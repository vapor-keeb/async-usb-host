#![no_std]
use arrayvec::ArrayVec;
use bus::BusWrap;
use consts::UsbBaseClass;
use core::{error, marker::PhantomData, task::Poll};
use descriptor::DeviceDescriptor;
use device_addr::DeviceAddressAllocator;
use driver::hub::Hub;
use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Timer};
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

pub(crate) enum HostState<const NR_HUBS: usize> {
    Disconnected,
    EnumerateRoot,
    DeviceAttached {
        hubs: ArrayVec<driver::hub::Hub, NR_HUBS>,
    },
    Suspended,
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
enum HostInternalEvent {
    BusEvent(Event),
    HostEvent(HostEvent),
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum HostEvent {
    NewDevice {
        descriptor: DeviceDescriptor,
        handle: DeviceHandle,
    },
    DeviceDetach,
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

pub struct Host<'a, D: Driver, const NR_HUBS: usize> {
    phantom: PhantomData<D>,
    bus: BusWrap<D>,
    pipe: &'a USBHostPipe<D>,
    state: HostState<NR_HUBS>,
}

impl<'a, D: Driver, const NR_HUBS: usize>
    Host<'a, D, NR_HUBS>
{
    pub fn new(bus: D::Bus, pipe: &'a USBHostPipe<D>) -> Self {
        Host {
            bus: BusWrap::new(bus),
            pipe,
            state: HostState::Disconnected,
            phantom: PhantomData,
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
                    state: HostState::EnumerateRoot,
                    ..
                } => {
                    let msg = self.enumerate_root().await;
                    if let Some(msg) = msg {
                        return msg;
                    }
                }
                Host {
                    state: HostState::DeviceAttached { hubs },
                    pipe,
                    bus,
                    ..
                } => {
                    let (event,state) = Self::run_device_attached(pipe, bus, hubs).await;
                    if let Some(state) = state {
                        self.state = state;
                    }
                    if let Some(event) = event {
                        return event;
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
        hubs: &mut ArrayVec<Hub, NR_HUBS>,
    ) -> (Option<HostEvent>, Option<HostState<NR_HUBS>>) {
        match Self::run_device_attached_inner(pipe, bus, hubs).await {
            Ok(Some(HostInternalEvent::BusEvent(event))) => {
                match event {
                    Event::DeviceAttach => {
                        warn!("device attached while device already attached");
                        (None, Some(HostState::EnumerateRoot))
                    },
                    Event::DeviceDetach => (Some(HostEvent::DeviceDetach), Some(HostState::Disconnected)),
                    Event::Suspend => (None, Some(HostState::Suspended)),
                    Event::Resume => (None, Some(HostState::Disconnected)),
                }
            },
            Ok(Some(HostInternalEvent::HostEvent(event))) => {
                (Some(event), None)
            },
            Ok(None) => (None, None),
            Err(e) => {
                error!("{}", e);
                (None, Some(HostState::Disconnected))
            },
        }
    }

    async fn run_device_attached_inner(
        pipe: &USBHostPipe<D>,
        bus: &mut BusWrap<D>,
        hubs: &mut ArrayVec<Hub, NR_HUBS>,
    ) -> Result<Option<HostInternalEvent>, UsbHostError> {
        let bus_fut = bus.poll();
        let mut hubs_fut = async || {
            for hub in hubs.iter_mut() {
                match hub.poll(pipe).await {
                    Ok(Some(event)) => {
                        return Some(event);
                    },
                    Ok(None) => (),
                    // whomp whomp
                    Err(_) => (),
                }
            }
            Timer::after(Duration::from_millis(1)).await;
            None
        };
        let hubs_fut = hubs_fut();

        match select(hubs_fut, bus_fut).await {
            Either::First(Some(event)) => {
                match event {
                    driver::hub::HubEvent::DeviceAttach { port } => {
                        Self::enumerate_device(pipe, bus, hubs).await.map(|opt| {
                            opt.map(|(descriptor, handle)|
                                HostInternalEvent::HostEvent(HostEvent::NewDevice { descriptor, handle })
                            )
                        })
                    },
                    driver::hub::HubEvent::DeviceDetach { port } => {
                        trace!("device detached from port {}", port);
                        Ok(Some(HostInternalEvent::HostEvent(HostEvent::DeviceDetach)))
                    },
                }
            },
            Either::First(None) => Ok(None),
            Either::Second(event) => Ok(Some(HostInternalEvent::BusEvent(event))),
        }
    }

    async fn run_disconnected(&mut self) {
        // TODO free all addresses.
        self.state = match self.bus.poll().await {
            Event::DeviceAttach => HostState::EnumerateRoot,
            Event::DeviceDetach => {
                trace!("root device detached when disconnected");
                HostState::Disconnected
            },
            Event::Suspend => {
                trace!("host suspended");
                HostState::Suspended
            },
            Event::Resume => {
                trace!("host resumed");
                HostState::Disconnected
            },
        };
    }

    async fn enumerate_root(&mut self) -> Option<HostEvent> {
        let mut hubs = ArrayVec::new();
        match Self::enumerate_device(&self.pipe, &mut self.bus, &mut hubs).await {
            Ok(event) => {
                self.state = HostState::DeviceAttached { hubs };
                event.map(|(descriptor, handle)| HostEvent::NewDevice { descriptor, handle })
            }
            Err(e) => {
                error!("{}", e);
                self.state = HostState::Disconnected;
                Some(HostEvent::Suspended)
            }
        }
    }

    /// Ok(None) if the device is a hub
    /// Ok(Some((descriptor, handle))) if the device is not a hub
    /// Err if there is an error
    async fn enumerate_device(
        pipe: &USBHostPipe<D>,
        bus: &mut BusWrap<D>,
        hubs: &mut ArrayVec<Hub, NR_HUBS>,
    ) -> Result<Option<(DeviceDescriptor, DeviceHandle)>, UsbHostError> {
        let pipe_future = pipe.dev_attach();
        let bus_future = bus.wait_until_detach();

        let (descriptor, handle) = poll_select(pipe_future, bus_future, |either| match either {
            futures::Either::First(device_result) => match device_result {
                Ok((descriptor, handle)) => Poll::Ready(Ok((descriptor, handle))),
                Err(e) => Poll::Ready(Err(e)),
            },
            futures::Either::Second(_) => Poll::Ready(Err(UsbHostError::Detached)),
        })
        .await?;

        if descriptor.device_class == UsbBaseClass::Hub.into() {
            let hub = driver::hub::Hub::new(pipe, handle, descriptor).await?;
            hubs.try_push(hub).map_err(|_| UsbHostError::HubCapacity)?;

            Ok(None)
        } else {
            Ok(Some((descriptor, handle)))
        }
    }
}
