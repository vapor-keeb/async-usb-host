#![no_std]
use core::{marker::PhantomData, task::Poll};

use arrayvec::ArrayVec;
use bus::BusWrap;
use consts::UsbBaseClass;
use descriptor::DeviceDescriptor;
use embassy_futures::select::{select, Either};
use embassy_time::Duration;
use errors::UsbHostError;
use futures::poll_select;
use pipe::PipeWrap;
use request::Request;
use types::{EndpointAddress, InterruptChannel};
use device_addr::DeviceAddressAllocator;

pub mod consts;
pub mod descriptor;
// pub mod driver;
mod device_addr;
pub mod errors;
mod futures;
mod hot_potato;
mod macros;
pub mod request;
pub mod types;

mod bus;
mod pipe;
pub use bus::{Bus, Event};
pub use device_addr::DeviceHandle;
pub use pipe::Pipe;

const TRANSFER_TIMEOUT: Duration = Duration::from_millis(500);

pub trait Driver {
    type Bus: Bus;
    type Pipe: Pipe;

    fn start(self) -> (Self::Bus, Self::Pipe);
}

struct InterruptTransfer<'a> {
    channel: InterruptChannel,
    buffer: &'a mut [u8],
}

pub(crate) enum HostState<'a, const NR_PENDING_TRANSFERS: usize> {
    Initializing,
    Disconnected,
    DeviceEnumerate,
    DeviceAttached {
        interrupt_transfers: ArrayVec<InterruptTransfer<'a>, NR_PENDING_TRANSFERS>,
    },
    Suspended,
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
    Suspended,
}

pub struct Host<'a, D: Driver, const NR_CLIENTS: usize, const NR_PENDING_TRANSFERS: usize> {
    phantom: PhantomData<D>,
    bus: BusWrap<D>,
    pipe: PipeWrap<D>,
    state: HostState<'a, NR_PENDING_TRANSFERS>,
    address_alloc: DeviceAddressAllocator,
}

impl<'a, D: Driver, const NR_CLIENTS: usize, const NR_PENDING_TRANSFERS: usize>
    Host<'a, D, NR_CLIENTS, NR_PENDING_TRANSFERS>
{
    pub fn new(driver: D) -> Self {
        let (bus, pipe) = driver.start();

        Host {
            bus: BusWrap::new(bus),
            pipe: PipeWrap::new(pipe),
            address_alloc: DeviceAddressAllocator::new(),
            state: HostState::Initializing,
            phantom: PhantomData,
        }
    }

    pub async fn control_transfer(
        &mut self,
        dev_handle: DeviceHandle,
        request: Request,
        buffer: &'a mut [u8],
    ) -> Result<usize, UsbHostError> {
        if let HostState::DeviceAttached {
            interrupt_transfers: _,
        } = self.state
        {
            self.pipe
                .control_transfer(dev_handle, &request, buffer)
                .await
        } else {
            return Err(UsbHostError::InvalidState);
        }
    }

    pub async fn interrupt_transfer(
        &mut self,
        interrupt_channel: InterruptChannel,
        buffer: &'a mut [u8],
    ) -> Result<(), UsbHostError> {
        if let HostState::DeviceAttached {
            ref mut interrupt_transfers,
        } = self.state
        {
            interrupt_transfers
                .try_push(InterruptTransfer {
                    channel: interrupt_channel,
                    buffer,
                })
                .map_err(|cap| UsbHostError::InterruptTransferCapacity(cap.element().channel))
        } else {
            return Err(UsbHostError::InvalidState);
        }
    }

    pub async fn run_until_event(&mut self) -> Host2ClientMessage {
        loop {
            let state = core::mem::replace(&mut self.state, HostState::Disconnected);
            match state {
                HostState::Initializing => {
                    info!("Driver ready!");
                    self.state = HostState::Disconnected;
                }
                HostState::Disconnected => self.run_disconnected().await,
                HostState::DeviceEnumerate => {
                    let msg = self.run_enumerate().await;
                    if let Some(msg) = msg {
                        return msg;
                    }
                }
                HostState::DeviceAttached {
                    interrupt_transfers,
                } => {
                    self.run_device_attached(interrupt_transfers).await;
                }
                HostState::Suspended => {
                    self.state = HostState::Disconnected;
                    return Host2ClientMessage::Suspended;
                }
            }
        }
    }

    async fn run_device_attached(
        &mut self,
        mut interrupt_xfer: ArrayVec<InterruptTransfer<'a>, NR_PENDING_TRANSFERS>,
    ) {
        let bus_fut = Self::handle_bus_event(&mut self.bus);
        let interrupt_xfer_fut = self.pipe.interrupt_transfer(&mut interrupt_xfer);
        self.state = match select(interrupt_xfer_fut, bus_fut).await {
            Either::First(xfer) => {
                info!("Interrupt xfer completed");
                HostState::DeviceAttached {
                    interrupt_transfers: interrupt_xfer,
                }
            }
            Either::Second(state) => state,
        };
    }

    async fn run_disconnected(&mut self) {
        // TODO free all addresses.
        self.state = Self::handle_bus_event(&mut self.bus).await;
    }

    async fn handle_bus_event(bus: &mut BusWrap<D>) -> HostState<'a, NR_PENDING_TRANSFERS> {
        let event = bus.poll().await;
        info!("event: {}", event);
        match event {
            Event::DeviceAttach => HostState::DeviceEnumerate,
            Event::DeviceDetach => HostState::Suspended,
            // TODO not implemented correctly
            Event::Suspend => HostState::Suspended,
            Event::Resume => HostState::Disconnected,
        }
    }

    async fn run_enumerate(&mut self) -> Option<Host2ClientMessage> {
        let pipe_future = self.pipe.dev_attach(&mut self.address_alloc);
        let bus_future = self.bus.poll();

        let (state, opt) = poll_select(pipe_future, bus_future, |either| match either {
            futures::Either::First(device_result) => Poll::Ready(match device_result {
                Ok((descriptor, handle)) => (
                    HostState::DeviceAttached {
                        interrupt_transfers: ArrayVec::new(),
                    },
                    Some((descriptor, handle)),
                ),
                Err(e) => {
                    debug!("{}", e);
                    // TODO: restore to "previous state"
                    (HostState::Disconnected, None)
                }
            }),
            futures::Either::Second(event) => {
                info!("event: {}", event);
                match event {
                    Event::DeviceDetach => Poll::Ready((HostState::Disconnected, None)),
                    _ => Poll::Pending,
                }
            }
        })
        .await;

        self.state = state;

        if let Some((descriptor, handle)) = opt {
            trace!("device attached!");
            if descriptor.device_class == UsbBaseClass::Hub.into() {
                // unwrap!(driver::hub::register_hub(self, handle, descriptor).await);
                None
            } else {
                Some(Host2ClientMessage::NewDevice { descriptor, handle })
            }
        } else {
            None
        }
    }
}
