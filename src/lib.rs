#![no_std]
use core::{array, future::Future, marker::PhantomData, mem::transmute, task::Poll};

use consts::UsbBaseClass;
use descriptor::{
    hub::HubDescriptor, parse_descriptor, ConfigurationDescriptor, Descriptor, DescriptorType,
    DeviceDescriptor,
};
use embassy_futures::select::{select, select_array, Either};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::{Duration, Timer};
use errors::UsbHostError;
use futures::poll_select;
use request::{Request, RequestType, StandardDeviceRequest};

pub mod consts;
pub mod descriptor;
pub mod errors;
mod futures;
mod hot_potato;
mod macros;
pub mod request;
pub mod types;

const TRANSFER_TIMEOUT: Duration = Duration::from_millis(500);

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Event {
    DeviceAttach,
    DeviceDetach,
    Suspend,
    Resume,
}

// not Send anyways
#[allow(async_fn_in_trait)]
pub trait Bus {
    async fn reset(&mut self);
    /// must be able to resume after completion
    /// aka poll after returning Poll::Ready(_)
    /// the built-in async keyword does not allow this
    fn poll(&mut self) -> impl Future<Output = Event>;
}

// not Send anyways
#[allow(async_fn_in_trait)]
pub trait Pipe {
    fn set_addr(&mut self, addr: u8);
    /// When setup is called, it should send a setup request, also setup the
    /// hardware to send / expect DATA1 packets on subsequent data_in / data_out
    async fn setup(&mut self, buf: &[u8; 8]) -> Result<(), UsbHostError>;
    async fn data_in(&mut self, endpoint: u8, buf: &mut [u8]) -> Result<usize, UsbHostError>;
    async fn data_out(&mut self, endpoint: u8, buf: &[u8]) -> Result<(), UsbHostError>;
}

pub trait Driver {
    type Bus: Bus;
    type Pipe: Pipe;

    fn start(self) -> (Self::Bus, Self::Pipe);
}

pub enum HostState {
    Initializing,
    Disconnected,
    DeviceEnumerate,
    DeviceAttached {
        handle: DeviceHandle,
        descriptor: DeviceDescriptor,
    },
    Idle,
    Suspended,
}

pub struct HostControl {
    signal: Signal<CriticalSectionRawMutex, bool>,
}

impl HostControl {
    pub const fn new() -> Self {
        HostControl {
            signal: Signal::new(),
        }
    }
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Client2HostMessage {
    ClientReady,
    ControlTransfer {
        dev_handle: DeviceHandle,
        request: Request,
        buffer: &'static mut [u8],
    },
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Host2ClientMessage {
    NewDevice {
        descriptor: DeviceDescriptor,
        handle: DeviceHandle,
    },
    ControlTransferResponse {
        result: Result<usize, UsbHostError>,
        buffer: &'static mut [u8],
    },
}

pub struct HostHandle {
    host2client: Channel<CriticalSectionRawMutex, Host2ClientMessage, 1>,
    client2host: Channel<CriticalSectionRawMutex, Client2HostMessage, 1>,
    accept_device: fn(desc: &DeviceDescriptor) -> bool,
}

impl HostHandle {
    pub const fn new(accept_device: fn(desc: &DeviceDescriptor) -> bool) -> Self {
        HostHandle {
            host2client: Channel::new(),
            client2host: Channel::new(),
            accept_device,
        }
    }

    pub async fn register(&self) {
        self.client2host.send(Client2HostMessage::ClientReady).await;
    }

    pub async fn recv(&self) -> Host2ClientMessage {
        self.host2client.receive().await
    }

    pub async fn control_transfer(
        &self,
        dev_handle: DeviceHandle,
        request: Request,
        buffer: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        hot_potato::toss_potato_async(buffer, async |potate| {
            self.client2host
                .send(Client2HostMessage::ControlTransfer {
                    dev_handle,
                    request,
                    buffer: potate,
                })
                .await;

            let result = self.host2client.receive().await;
            match result {
                Host2ClientMessage::ControlTransferResponse { result, buffer } => (buffer, result),
                _ => panic!(),
            }
        })
        .await
    }
}

pub struct Host<'a, D: Driver, const NR_CLIENTS: usize> {
    phantom: PhantomData<D>,
    host_control: &'a HostControl,
    clients: [&'a HostHandle; NR_CLIENTS],
    bus: BusWrap<D>,
    pipe: PipeWrap<D>,
    state: HostState,
    address_alloc: DeviceAddressAllocator,
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct DeviceHandle {
    address: u8,
    max_packet_size: u8,
}

struct PipeWrap<D: Driver>(D::Pipe);

impl<D: Driver> PipeWrap<D> {
    async fn setup(&mut self, req: &Request) -> Result<(), UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        let setup_fut = self.0.setup(unsafe { transmute(req) });
        match select(timeout_fut, setup_fut).await {
            Either::First(_) => Err(UsbHostError::TransferTimeout),
            Either::Second(r) => r,
        }
    }

    async fn data_in(&mut self, endpoint: u8, buf: &mut [u8]) -> Result<usize, UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        let mut data_in_with_retry = async || loop {
            match self.0.data_in(endpoint, buf).await {
                Ok(size) => return Ok(size),
                Err(UsbHostError::NAK) => {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        };
        let data_in_fut = data_in_with_retry();
        match select(timeout_fut, data_in_fut).await {
            Either::First(_) => Err(UsbHostError::TransferTimeout),
            Either::Second(r) => r,
        }
    }

    async fn data_out(&mut self, endpoint: u8, buf: &[u8]) -> Result<(), UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        // TODO retry like data_in
        let data_fut = self.0.data_out(endpoint, buf);
        match select(timeout_fut, data_fut).await {
            Either::First(_) => Err(UsbHostError::TransferTimeout),
            Either::Second(r) => r,
        }
    }

    fn set_addr(&mut self, addr: u8) {
        self.0.set_addr(addr);
    }

    async fn assign_device_address(&mut self, addr: u8) -> Result<(), UsbHostError> {
        let request = Request {
            request_type: {
                use request::*;
                let mut t = RequestType::default();
                t.set_data_direction(RequestTypeDirection::HostToDevice);
                t.set_recipient(RequestTypeRecipient::Device);
                t
            },
            request: StandardDeviceRequest::SetAddress as u8,
            value: addr as u16,
            index: 0,
            length: 0,
        };

        // Setup stage
        self.setup(&request).await?;
        // Status stage (no data)
        self.data_in(0, &mut []).await?;

        Ok(())
    }

    async fn get_device_descriptor<'a>(
        &mut self,
        buf: &'a mut [u8],
    ) -> Result<&'a DeviceDescriptor, UsbHostError> {
        debug_assert!(buf.len() >= 18);
        // Setup Stage
        let request = Request {
            request_type: {
                use request::*;
                let mut rt = RequestType::default();
                rt.set_data_direction(RequestTypeDirection::DeviceToHost);
                rt.set_type(RequestTypeType::Standard);
                rt.set_recipient(RequestTypeRecipient::Device);
                rt
            },
            request: StandardDeviceRequest::GetDescriptor as u8,
            value: (1 << 8) | 0, // DescriptorType: 1(Device), Index 0
            index: 0,
            length: 64,
        };
        self.setup(&request).await?;
        trace!("setup finished");

        // Data stage
        let mut bytes_read = 0usize;
        let in_result = self.data_in(0, buf).await?;
        bytes_read += in_result;

        while bytes_read < core::mem::size_of::<DeviceDescriptor>() {
            let chopped_off_buf = &buf[bytes_read..];
            // SAFETY:
            // If the return Ok(desc); statement within the match block was executed,
            // the borrow is no longer in effect. Therefore, the unsafe transmute
            // is safe because there are no other outstanding immutable borrows of
            // the memory region being modified.
            let in_result = self
                .data_in(0, unsafe {
                    core::slice::from_raw_parts_mut(
                        chopped_off_buf.as_ptr() as *mut u8,
                        chopped_off_buf.len(),
                    )
                })
                .await?;
            bytes_read += in_result;
        }

        // Status stage
        self.data_out(0, &[]).await?;

        debug_assert!(bytes_read == core::mem::size_of::<DeviceDescriptor>());
        match parse_descriptor(buf) {
            Ok(desc) => {
                if let descriptor::Descriptor::Device(dev_desc) = desc {
                    Ok(dev_desc)
                } else {
                    Err(UsbHostError::Unknown)
                }
            }
            Err(e) => Err(UsbHostError::ParsingError(e)),
        }
    }

    async fn control_transfer(
        &mut self,
        request: &Request,
        buffer: &mut [u8],
        max_packet_size: u8,
    ) -> Result<usize, UsbHostError> {
        use request::RequestTypeDirection;
        let dir = request.request_type.data_direction();
        let mut bytes_received = 0usize;

        debug_assert!(buffer.len() >= request.length as usize);

        // Setup stage
        self.setup(request).await?;

        // (Optional) data stage
        if request.length > 0 {
            match dir {
                RequestTypeDirection::HostToDevice => todo!(),
                RequestTypeDirection::DeviceToHost => loop {
                    let len = self.data_in(0, &mut buffer[bytes_received..]).await?;
                    bytes_received += len;
                    if len < max_packet_size as usize {
                        break;
                    }
                },
            }
        }

        // Status stage
        match dir {
            RequestTypeDirection::HostToDevice => {
                self.data_in(0, &mut []).await?;
            }
            RequestTypeDirection::DeviceToHost => {
                self.data_out(0, &[]).await?;
            }
        }

        Ok(bytes_received)
    }

    async fn dev_attach(
        &mut self,
        address_alloc: &mut DeviceAddressAllocator,
    ) -> Result<(DeviceDescriptor, DeviceHandle), UsbHostError> {
        let mut buffer: [u8; 18] = [0u8; 18];
        let d = self.get_device_descriptor(&mut buffer).await?;
        let max_packet_size = d.max_packet_size;
        let addr = address_alloc.alloc_device_address();
        trace!("DeviceDescriptor: {}", d);

        self.assign_device_address(addr).await?;
        trace!("Device addressed {}", addr);

        Ok((
            d.clone(),
            DeviceHandle {
                address: addr,
                max_packet_size,
            },
        ))
    }
}

struct BusWrap<D: Driver>(D::Bus);

struct DeviceAddressAllocator([u8; 16]);

impl DeviceAddressAllocator {
    // Construct an allocator with all addresses except 0 occupied.
    pub fn new() -> Self {
        let mut alloc = DeviceAddressAllocator([0; 16]);
        // Address 0 is always used;
        alloc.0[0] = 1;
        alloc
    }

    fn set_addr(&mut self, addr: u8, used: bool) {
        debug_assert_ne!(addr, 0);
        let nth_byte = addr / 8;
        let bit_offset = addr % 8;
        let rest = self.0[nth_byte as usize] & !(1u8 << bit_offset);
        self.0[nth_byte as usize] = rest | ((if used { 1 } else { 0 }) << bit_offset);
    }

    pub fn alloc_device_address(&mut self) -> u8 {
        let mut addr: Option<u8> = None;

        'outer: for nth_byte in 0..8usize {
            // has at least one 0 bit
            if self.0[nth_byte] != 0xFF {
                let byte = self.0[nth_byte];
                for bit_offset in 0..8 {
                    if (byte & (1 << bit_offset)) == 0 {
                        addr.replace(nth_byte as u8 * 8 + bit_offset);
                        break 'outer;
                    }
                }
            }
        }

        assert!(addr.is_some(), "Ran out of address");

        let addr = addr.unwrap();
        debug_assert_ne!(addr, 0);
        // Mark address as used
        self.set_addr(addr, true);

        return addr;
    }

    pub fn free_address(&mut self, addr: u8) {
        self.set_addr(addr, false);
    }
}

impl<'a, D: Driver, const NR_CLIENTS: usize> Host<'a, D, NR_CLIENTS> {
    pub fn new(
        driver: D,
        host_control: &'a HostControl,
        clients: [&'a HostHandle; NR_CLIENTS],
    ) -> Self {
        let (bus, pipe) = driver.start();

        Host {
            bus: BusWrap(bus),
            pipe: PipeWrap(pipe),
            address_alloc: DeviceAddressAllocator::new(),
            host_control,
            state: HostState::Initializing,
            clients,
            phantom: PhantomData,
        }
    }

    async fn register_hub(
        &mut self,
        handle: DeviceHandle,
        descriptor: DeviceDescriptor,
    ) -> Result<(), UsbHostError> {
        let mut hub_desc = HubDescriptor::default();
        let buf = unsafe {
            core::slice::from_raw_parts_mut(
                &raw mut hub_desc as *mut u8,
                core::mem::size_of::<HubDescriptor>(),
            )
        };
        // TODO maybe control_transfer should just take an address
        self.pipe.set_addr(handle.address);
        self.pipe
            .control_transfer(
                &Request::get_descriptor(
                    0x29, // Hub Descriptor
                    request::RequestTypeType::Class,
                    0,
                    0,
                    buf.len() as u16,
                ),
                buf,
                handle.max_packet_size,
            )
            .await?;

        debug!("hub descriptor: {:?}", hub_desc);

        let mut buf: [u8; 255] = [0; 255];
        let len = unwrap!(
            self.pipe
                .control_transfer(
                    &Request::get_configuration_descriptor(
                        0,
                        core::mem::size_of::<ConfigurationDescriptor>() as u16
                    ),
                    &mut buf,
                    handle.max_packet_size,
                )
                .await
        );
        let cfg = parse_descriptor(&buf[..len]).map_err(|e| UsbHostError::ParsingError(e))?;

        trace!("configuration recv {} bytes: {:?}", len, cfg);
        if let Descriptor::Configuration(cfg) = cfg {
            self.pipe
                .control_transfer(
                    &Request::set_configuration(cfg.value),
                    &mut [],
                    handle.max_packet_size,
                )
                .await?;
            // get configuration descriptor again with the proper len
            let len = unwrap!(
                self.pipe
                    .control_transfer(
                        &Request::get_configuration_descriptor(0, cfg.total_length),
                        &mut buf,
                        handle.max_packet_size,
                    )
                    .await
            );
            // TODO should probably iterate through the descriptor
            let mut cfg_buf = &buf[..len];
            while !cfg_buf.is_empty() {
                trace!("{}", cfg_buf);
                let desc = parse_descriptor(cfg_buf).map_err(|e| UsbHostError::ParsingError(e))?;
                let len = match desc {
                    Descriptor::Device(device_descriptor) => device_descriptor.length,
                    Descriptor::Configuration(configuration_descriptor) => {
                        configuration_descriptor.length
                    }
                    Descriptor::Endpoint(endpoint_descriptor) => {
                        // interview quality code
                        // TODO needs to go into a separate polling task
                        // for each port
                        // GET_STATUS
                        // SET_FEATURE
                        loop {
                            let mut in_buf: [u8; 64] = [0; 64];
                            let in_buf_len = self
                                .pipe
                                .data_in(endpoint_descriptor.b_endpoint_address, &mut in_buf)
                                .await;

                            if let Ok(in_buf_len) = in_buf_len {
                                trace!("{}", in_buf[..in_buf_len]);
                                break;
                            }
                        }

                        endpoint_descriptor.b_length
                    }
                    Descriptor::Interface(interface_descriptor) => interface_descriptor.b_length,
                } as usize;

                cfg_buf = &cfg_buf[len..];
            }

            // enable ports

            Ok(())
        } else {
            Err(UsbHostError::InvalidState)
        }
    }

    pub async fn run_until_suspend(&mut self) {
        loop {
            let state = core::mem::replace(&mut self.state, HostState::Disconnected);
            self.state = match state {
                HostState::Initializing => {
                    for client in self.clients.iter() {
                        let packet = client.client2host.receive().await;
                        if let Client2HostMessage::ClientReady = packet {
                        } else {
                            panic!("???");
                        }
                    }
                    info!("Driver ready!");
                    HostState::Disconnected
                }
                HostState::Disconnected => Self::run_disconnected(&mut self.bus).await,
                HostState::DeviceEnumerate => self.run_enumerate().await,
                HostState::DeviceAttached { handle, descriptor } => {
                    if descriptor.device_class == UsbBaseClass::Hub.into() {
                        unwrap!(self.register_hub(handle, descriptor).await);
                    } else {
                        let mut accepted = false;

                        for client in self.clients {
                            if (client.accept_device)(&descriptor) {
                                client
                                    .host2client
                                    .send(Host2ClientMessage::NewDevice {
                                        descriptor: descriptor,
                                        handle: handle,
                                    })
                                    .await;
                                accepted = true;
                                break;
                            }
                        }
                        trace!("device accepted?: {}", accepted);
                    }

                    HostState::Idle
                }
                HostState::Idle => self.run_idle().await,
                HostState::Suspended => {
                    self.state = HostState::Disconnected;
                    break;
                }
            }
        }
    }

    async fn run_idle(&mut self) -> HostState {
        let futures: [_; NR_CLIENTS] = array::from_fn(|i| self.clients[i].client2host.receive());

        let client_request_fut = select_array(futures);
        let bus_fut = self.bus.0.poll();
        match select(client_request_fut, bus_fut).await {
            Either::First((client_request, client_id)) => {
                trace!("got request: {}, {:?}", client_id, client_request);
                match client_request {
                    Client2HostMessage::ClientReady => warn!("client ready"),
                    Client2HostMessage::ControlTransfer {
                        dev_handle,
                        request,
                        buffer,
                    } => {
                        self.pipe.set_addr(dev_handle.address);
                        let result = self
                            .pipe
                            .control_transfer(&request, buffer, dev_handle.max_packet_size)
                            .await;
                        self.clients[client_id]
                            .host2client
                            .send(Host2ClientMessage::ControlTransferResponse { result, buffer })
                            .await;
                    }
                }
                HostState::Idle
            }
            Either::Second(bus_event) => Self::handle_bus_event(&mut self.bus, bus_event).await,
        }
    }

    async fn run_disconnected(bus: &mut BusWrap<D>) -> HostState {
        let event = bus.0.poll().await;
        Self::handle_bus_event(bus, event).await
    }

    async fn handle_bus_event(bus: &mut BusWrap<D>, event: Event) -> HostState {
        info!("event: {}", event);
        match event {
            Event::DeviceAttach => {
                bus.0.reset().await;
                #[cfg(feature = "embassy")]
                embassy_time::Timer::after_millis(500).await;

                HostState::DeviceEnumerate
            }
            Event::DeviceDetach => HostState::Suspended,
            Event::Suspend => HostState::Suspended,
            Event::Resume => HostState::Disconnected,
        }
    }

    async fn run_enumerate(&mut self) -> HostState {
        let pipe_future = self.pipe.dev_attach(&mut self.address_alloc);
        let bus_future = self.bus.0.poll();

        poll_select(pipe_future, bus_future, |either| match either {
            futures::Either::First(device_result) => Poll::Ready(match device_result {
                Ok((desc, dev_handle)) => {
                    trace!("device attached!");
                    HostState::DeviceAttached {
                        handle: dev_handle,
                        descriptor: desc,
                    }
                }
                Err(e) => {
                    debug!("{}", e);
                    // TODO: restore to "previous state"
                    HostState::Disconnected
                }
            }),
            futures::Either::Second(event) => {
                info!("event: {}", event);
                match event {
                    Event::DeviceDetach => Poll::Ready(HostState::Disconnected),
                    _ => Poll::Pending,
                }
            }
        })
        .await
    }
}
