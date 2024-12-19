#![no_std]
use core::{future::Future, marker::PhantomData, mem::transmute, task::Poll};

use descriptor::{parse_descriptor, ConfigurationDescriptor, DeviceDescriptor};
use embassy_futures::select::{select, Either};
use errors::UsbHostError;
use futures::poll_select;
use request::{Request, StandardDeviceRequest};

pub mod descriptor;
pub mod errors;
mod futures;
mod macros;
pub mod request;
pub mod types;

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
    async fn data_in(&mut self, buf: &mut [u8]) -> Result<usize, UsbHostError>;
    async fn data_out(&mut self, buf: &[u8]) -> Result<(), UsbHostError>;
}

pub trait Driver {
    type Bus: Bus;
    type Pipe: Pipe;

    fn start(self) -> (Self::Bus, Self::Pipe);
}

pub enum HostState {
    Idle,
    DeviceEnumerate,
    DeviceAttached(DeviceHandle),
    Suspended,
}

pub struct Host<D: Driver> {
    phantom: PhantomData<D>,
    bus: BusWrap<D>,
    pipe: PipeWrap<D>,
    address_alloc: DeviceAddressAllocator,
}

impl<D: Driver> Host<D> {
    pub fn new(driver: D) -> Self {
        let (bus, pipe) = driver.start();

        Host {
            bus: BusWrap(bus),
            pipe: PipeWrap(pipe),
            address_alloc: DeviceAddressAllocator(1),
            phantom: PhantomData,
        }
    }
}

pub struct DeviceHandle {
    address: u8,
}

struct PipeWrap<D: Driver>(D::Pipe);

impl<D: Driver> PipeWrap<D> {
    async fn setup(&mut self, req: &Request) -> Result<(), UsbHostError> {
        self.0.setup(unsafe { transmute(req) }).await
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
        self.0.data_in(&mut []).await?;

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
        let in_result = self.0.data_in(buf).await?;
        bytes_read += in_result;

        let max_packet_size = match parse_descriptor(&buf[..bytes_read]) {
            Ok(desc) => {
                if let descriptor::Descriptor::Device(dev_desc) = desc {
                    return Ok(dev_desc);
                }
                return Err(UsbHostError::Unknown);
            }
            Err(descriptor::ParsingError::IncompleteDeviceDescriptor { max_packet_size }) => {
                max_packet_size
            }
            Err(e) => return Err(UsbHostError::ParsingError(e)),
        };

        debug_assert!(max_packet_size % 8 == 0);

        while bytes_read < 18 {
            let chopped_off_buf = &buf[bytes_read..];
            // SAFETY:
            // If the return Ok(desc); statement within the match block was executed,
            // the borrow is no longer in effect. Therefore, the unsafe transmute
            // is safe because there are no other outstanding immutable borrows of
            // the memory region being modified.
            let in_result = self
                .0
                .data_in(unsafe {
                    core::slice::from_raw_parts_mut(
                        chopped_off_buf.as_ptr() as *mut u8,
                        chopped_off_buf.len(),
                    )
                })
                .await?;
            bytes_read += in_result;
        }

        // Status stage
        self.0.data_out(&[]).await?;

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

    pub async fn control_transfer(
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
                    let len = self.0.data_in(&mut buffer[bytes_received..]).await?;
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
                self.0.data_in(&mut []).await?;
            }
            RequestTypeDirection::DeviceToHost => {
                self.0.data_out(&[]).await?;
            }
        }

        Ok(bytes_received)
    }

    async fn dev_attach(
        &mut self,
        address_alloc: &mut DeviceAddressAllocator,
    ) -> Result<DeviceHandle, UsbHostError> {
        let mut buffer: [u8; 18] = [0u8; 18];
        let d = self.get_device_descriptor(&mut buffer).await?;
        let max_packet_size = d.max_packet_size;
        let addr = address_alloc.alloc_device_address();
        trace!("DeviceDescriptor: {}", d);

        self.assign_device_address(addr).await?;
        trace!("Device addressed {}", addr);
        self.0.set_addr(addr);
        let mut buf: [u8; 255] = [0; 255];
        let len = unwrap!(
            self.control_transfer(
                &Request::get_configuration_descriptor(
                    0,
                    core::mem::size_of::<ConfigurationDescriptor>() as u16
                ),
                &mut buf,
                max_packet_size,
            )
            .await
        );
        let cfg = parse_descriptor(&buf[..len]);
        trace!("configuration recv {} bytes: {:?}", len, cfg);
        self.control_transfer(&Request::set_configuration(1), &mut [], max_packet_size)
            .await?;
        Ok(DeviceHandle { address: addr })
    }
}

struct BusWrap<D: Driver>(D::Bus);

struct DeviceAddressAllocator(u8);

impl DeviceAddressAllocator {
    fn alloc_device_address(&mut self) -> u8 {
        let addr = self.0;
        // TODO allocate and free addresses properly
        assert!(addr <= 127, "out of addr");
        self.0 += 1;
        return addr;
    }
}

impl<D: Driver> Host<D> {
    pub async fn run_until_suspend(mut self) -> (Self, Option<DeviceHandle>) {
        let mut state = HostState::Idle;
        let mut handle = None;

        let Host {
            phantom: _,
            mut bus,
            mut pipe,
            mut address_alloc,
        } = self;

        loop {
            match state {
                HostState::Idle => {
                    state = Self::run_idle(&mut bus).await;
                }
                HostState::DeviceEnumerate => {
                    state = Self::run_enumerate(&mut bus, &mut pipe, &mut address_alloc).await;
                }
                HostState::Suspended => break,
                HostState::DeviceAttached(d) => {
                    handle.replace(d);
                    break;
                }
            }
        }

        (
            Host {
                phantom: PhantomData,
                bus,
                pipe,
                address_alloc,
            },
            handle,
        )
    }

    async fn run_idle(bus: &mut BusWrap<D>) -> HostState {
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
            Event::Resume => HostState::Idle,
        }
    }

    async fn run_enumerate(
        bus: &mut BusWrap<D>,
        pipe: &mut PipeWrap<D>,
        address_alloc: &mut DeviceAddressAllocator,
    ) -> HostState {
        let pipe_future = pipe.dev_attach(address_alloc);
        let bus_future = bus.0.poll();

        poll_select(pipe_future, bus_future, |either| match either {
            futures::Either::First(device_result) => Poll::Ready(match device_result {
                Ok(dev) => HostState::DeviceAttached(dev),
                Err(e) => {
                    debug!("{}", e);
                    HostState::Idle
                }
            }),
            futures::Either::Second(event) => {
                // Self::handle_bus_event(bus, event);
                info!("event: {}", event);
                match event {
                    Event::DeviceDetach => Poll::Ready(HostState::Idle),
                    _ => Poll::Pending,
                }
            }
        })
        .await
    }
}
