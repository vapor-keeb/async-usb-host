#![no_std]
use arrayvec::ArrayVec;
use bus::BusWrap;
use consts::UsbBaseClass;
use core::{marker::PhantomData, task::Poll};
use descriptor::DeviceDescriptor;
use device_addr::DeviceAddressAllocator;
use embassy_futures::select::{select, Either};
use embassy_time::Duration;
use errors::UsbHostError;
use futures::poll_select;
use pipe::USBHostPipe;
use request::Request;
use types::{EndpointAddress, InterruptChannel};

#[macro_use]
mod macros;

pub mod consts;
pub mod descriptor;
mod device_addr;
pub mod driver;
pub mod errors;
mod futures;
mod hot_potato;
pub mod request;
pub mod types;

mod bus;
pub mod pipe;
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
    Disconnected,
    DeviceEnumerate,
    DeviceAttached {
        interrupt_transfers: ArrayVec<InterruptTransfer<'a>, NR_PENDING_TRANSFERS>,
    },
    Suspended,
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum HostEvent {
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
    pipe: &'a USBHostPipe<D>,
    state: HostState<'a, NR_PENDING_TRANSFERS>,
}

impl<'a, D: Driver, const NR_CLIENTS: usize, const NR_PENDING_TRANSFERS: usize>
    Host<'a, D, NR_CLIENTS, NR_PENDING_TRANSFERS>
{
    pub fn new(bus: D::Bus, pipe: &'a USBHostPipe<D>) -> Self {
        Host {
            bus: BusWrap::new(bus),
            pipe: pipe,
            state: HostState::Disconnected,
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

    pub async fn run_until_event(&mut self) -> HostEvent {
        loop {
            match self {
                Host {
                    state: HostState::Disconnected,
                    ..
                } => self.run_disconnected().await,
                Host {
                    state: HostState::DeviceEnumerate,
                    ..
                } => {
                    let msg = self.run_enumerate().await;
                    if let Some(msg) = msg {
                        return msg;
                    }
                }
                Host {
                    state:
                        HostState::DeviceAttached {
                            interrupt_transfers,
                        },
                    pipe,
                    bus,
                    ..
                } => {
                    if let Some(new_state) =
                        Self::run_device_attached(pipe, bus, interrupt_transfers).await
                    {
                        self.state = new_state;
                    }
                }
                Host {
                    state: HostState::Suspended,
                    ..
                } => {
                    self.state = HostState::Disconnected;
                    return HostEvent::Suspended;
                }
            }
        }
    }

    async fn run_device_attached(
        pipe: &USBHostPipe<D>,
        bus: &mut BusWrap<D>,
        interrupt_xfer: &mut ArrayVec<InterruptTransfer<'a>, NR_PENDING_TRANSFERS>,
    ) -> Option<HostState<'a, NR_PENDING_TRANSFERS>> {
        let bus_fut = Self::handle_bus_event(bus);
        let interrupt_xfer_fut = pipe.interrupt_transfer(interrupt_xfer);
        match select(interrupt_xfer_fut, bus_fut).await {
            Either::First(xfer) => None,
            Either::Second(state) => Some(state),
        }
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

    async fn run_enumerate(&mut self) -> Option<HostEvent> {
        let pipe_future = self.pipe.dev_attach();
        let bus_future = self.bus.wait_until_detach();

        let (state, opt) = poll_select(pipe_future, bus_future, |either| match either {
            futures::Either::First(device_result) => match device_result {
                Ok((descriptor, handle)) => Poll::Ready((
                    HostState::DeviceAttached {
                        interrupt_transfers: ArrayVec::new(),
                    },
                    Some((descriptor, handle)),
                )),
                Err(e) => {
                    debug!("{}", e);
                    // TODO: restore to "previous state"
                    Poll::Ready((HostState::Disconnected, None))
                }
            },
            futures::Either::Second(_) => {
                info!("device detached!");
                Poll::Ready((HostState::Disconnected, None))
            }
        })
        .await;

        self.state = state;

        if let Some((descriptor, handle)) = opt {
            trace!("device attached!");
            if descriptor.device_class == UsbBaseClass::Hub.into() {
                driver::hub::Hub::new(self.pipe, handle, descriptor)
                    .await
                    .map(|_| None)
                    .unwrap_or_else(|e| {
                        debug!("{}", e);
                        Some(HostEvent::Suspended)
                    })
            } else {
                Some(HostEvent::NewDevice { descriptor, handle })
            }
        } else {
            Some(HostEvent::Suspended)
        }
    }
}
