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
use types::{DataTog, EndpointAddress, Pid};

pub mod consts;
pub mod descriptor;
pub mod errors;
mod futures;
mod hot_potato;
mod macros;
pub mod request;
pub mod types;
pub mod driver;

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
    async fn data_in(
        &mut self,
        endpoint: u8,
        tog: DataTog,
        buf: &mut [u8],
    ) -> Result<usize, UsbHostError>;
    async fn data_out(
        &mut self,
        endpoint: u8,
        tog: DataTog,
        buf: &[u8],
    ) -> Result<(), UsbHostError>;
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
    InterruptTransfer {
        dev_handle: DeviceHandle,
        endpoint_address: EndpointAddress,
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
    InterruptTransferResponse {
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

    pub async fn interrupt_transfer(
        &self,
        dev_handle: DeviceHandle,
        endpoint_address: EndpointAddress,
        buffer: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        hot_potato::toss_potato_async(buffer, async |potate| {
            self.client2host
                .send(Client2HostMessage::InterruptTransfer {
                    dev_handle,
                    endpoint_address,
                    buffer: potate,
                })
                .await;

            let result = self.host2client.receive().await;
            match result {
                Host2ClientMessage::InterruptTransferResponse { result, buffer } => {
                    (buffer, result)
                }
                _ => panic!(),
            }
        })
        .await
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
#[derive(Clone, Copy)]
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

    async fn data_in(
        &mut self,
        endpoint: u8,
        tog: DataTog,
        buf: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        let mut data_in_with_retry = async || loop {
            match self.0.data_in(endpoint, tog, buf).await {
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

    async fn data_out(
        &mut self,
        endpoint: u8,
        tog: DataTog,
        buf: &[u8],
    ) -> Result<(), UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        // TODO retry like data_in
        let data_fut = self.0.data_out(endpoint, tog, buf);
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
        self.data_in(0, DataTog::DATA1, &mut []).await?;

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

        let mut tog = DataTog::DATA1;
        // Data stage
        let mut bytes_read = 0usize;
        let in_result = self.data_in(0, tog, buf).await?;
        tog.next();
        bytes_read += in_result;

        while bytes_read < core::mem::size_of::<DeviceDescriptor>() {
            let chopped_off_buf = &buf[bytes_read..];
            // SAFETY:
            // If the return Ok(desc); statement within the match block was executed,
            // the borrow is no longer in effect. Therefore, the unsafe transmute
            // is safe because there are no other outstanding immutable borrows of
            // the memory region being modified.
            let in_result = self
                .data_in(0, tog, unsafe {
                    core::slice::from_raw_parts_mut(
                        chopped_off_buf.as_ptr() as *mut u8,
                        chopped_off_buf.len(),
                    )
                })
                .await?;
            tog.next();
            bytes_read += in_result;
        }

        // Status stage
        self.data_out(0, DataTog::DATA1, &[]).await?;

        debug_assert!(bytes_read == core::mem::size_of::<DeviceDescriptor>());
        let dev_desc = parse_descriptor(buf)
            .and_then(|desc| desc.device().ok_or(UsbHostError::InvalidResponse))?;
        Ok(dev_desc)
    }

    async fn control_transfer(
        &mut self,
        device_handle: DeviceHandle,
        request: &Request,
        buffer: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        use request::RequestTypeDirection;
        let dir = request.request_type.data_direction();
        let mut bytes_received = 0usize;

        debug_assert!(buffer.len() >= request.length as usize);

        self.set_addr(device_handle.address);

        // Setup stage
        self.setup(request).await?;

        // (Optional) data stage
        if request.length > 0 {
            match dir {
                RequestTypeDirection::HostToDevice => todo!(),
                RequestTypeDirection::DeviceToHost => {
                    let mut tog = DataTog::DATA1;
                    loop {
                        let len = self.data_in(0, tog, &mut buffer[bytes_received..]).await?;
                        tog.next();
                        bytes_received += len;
                        if len < device_handle.max_packet_size as usize {
                            break;
                        }
                    }
                }
            }
        }

        // Status stage
        match dir {
            RequestTypeDirection::HostToDevice => {
                self.data_in(0, DataTog::DATA1, &mut []).await?;
            }
            RequestTypeDirection::DeviceToHost => {
                self.data_out(0, DataTog::DATA1, &[]).await?;
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
                        unwrap!(driver::hub::register_hub(self, handle, descriptor).await);
                    } else {
                        let mut accepted = false;

                        for client in self.clients {
                            if (client.accept_device)(&descriptor) {
                                client
                                    .host2client
                                    .send(Host2ClientMessage::NewDevice { descriptor, handle })
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
                            .control_transfer(dev_handle, &request, buffer)
                            .await;
                        self.clients[client_id]
                            .host2client
                            .send(Host2ClientMessage::ControlTransferResponse { result, buffer })
                            .await;
                    }
                    Client2HostMessage::InterruptTransfer {
                        dev_handle,
                        endpoint_address,
                        buffer,
                    } => todo!(),
                }
                HostState::Idle
            }
            Either::Second(bus_event) => Self::handle_bus_event(&mut self.bus, bus_event).await,
        }
    }

    async fn run_disconnected(bus: &mut BusWrap<D>) -> HostState {
        // TODO free all addresses.
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
