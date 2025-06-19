/// USB Hub class driver, private because it is only used by the main driver.
///
use core::{future::Future, marker::PhantomData, pin::pin};

use crate::{
    descriptor::DeviceDescriptor, driver::kbd::HidKbd, errors::UsbHostError,
    futures::StaticUnpinPoller, pipe::USBHostPipe, DeviceHandle, HostDriver,
};
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

pub(crate) mod hub;
pub mod kbd;
pub mod dfu;

pub type DeviceChannel = Channel<CriticalSectionRawMutex, (DeviceHandle, DeviceDescriptor), 1>;

#[allow(async_fn_in_trait)]
pub trait USBHostDeviceDriver: Sized {
    async fn try_attach<D: HostDriver, const NR_DEVICES: usize>(
        pipe: &USBHostPipe<D, NR_DEVICES>,
        device: DeviceHandle,
        desc: DeviceDescriptor,
    ) -> Result<Self, UsbHostError>;

    async fn run<D: HostDriver, const NR_DEVICES: usize>(
        self,
        pipe: &USBHostPipe<D, NR_DEVICES>,
    ) -> Result<(), UsbHostError>;
}

pub struct USBDeviceDispatcher<
    'a,
    HDD: USBHostDeviceDriver,
    HD: HostDriver,
    const NR_DEVICES: usize,
> {
    pipe: &'a USBHostPipe<HD, NR_DEVICES>,
    new_dev: DeviceChannel,
    _phantom: PhantomData<HDD>,
}

impl<'a, HDD: USBHostDeviceDriver, HD: HostDriver, const NR_DEVICES: usize>
    USBDeviceDispatcher<'a, HDD, HD, NR_DEVICES>
{
    pub fn new(pipe: &'a USBHostPipe<HD, NR_DEVICES>) -> Self {
        Self {
            pipe,
            new_dev: DeviceChannel::new(),
            _phantom: PhantomData,
        }
    }

    pub fn run<'b>(&'b self) -> impl Future<Output = ()> + use<'a, 'b, HDD, HD, NR_DEVICES> {
        Self::run_inner(self.pipe, &self.new_dev)
    }

    pub async fn insert_new_device(&self, device: DeviceHandle, descriptor: DeviceDescriptor) {
        self.new_dev.send((device, descriptor)).await;
    }

    async fn run_inner<'b>(pipe: &'a USBHostPipe<HD, NR_DEVICES>, new_dev: &'b DeviceChannel) {
        let poller = StaticUnpinPoller::<_, NR_DEVICES>::new();
        let mut poller = pin!(poller);

        loop {
            let new_dev_fut = new_dev.receive();
            let (device, descriptor) = if poller.as_mut().is_empty() {
                new_dev_fut.await
            } else {
                match select(new_dev_fut, poller.as_mut()).await {
                    Either::First((device, descriptor)) => (device, descriptor),
                    Either::Second(Some((idx, result))) => {
                        match result {
                            Ok(_) => {
                                trace!("Device at slot {} completed successfully", idx);
                            }
                            Err(e) => error!("Device error at slot {}: {}", idx, e),
                        }
                        continue;
                    }
                    Either::Second(None) => {
                        continue;
                    }
                }
            };
            let hdd = HDD::try_attach(pipe, device, descriptor).await;
            match hdd {
                Ok(hdd) => {
                    // Find an empty slot for the new device
                    if let Err(e) = poller.as_mut().insert(hdd.run(pipe)) {
                        error!("No empty slots available for new device: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to attach device driver: {}", e);
                }
            }
        }
    }
}
