/// USB Hub class driver, private because it is only used by the main driver.
///
use core::{
    future::Future,
    pin::{pin, Pin},
    result,
};

use crate::{
    descriptor::DeviceDescriptor,
    driver::kbd::HidKbd,
    errors::UsbHostError,
    futures::{select_pin_array, StaticUnpinPoller},
    pipe::USBHostPipe,
    DeviceHandle, HostDriver,
};
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

pub(crate) mod hub;
pub mod kbd;

pub type DeviceChannel = Channel<CriticalSectionRawMutex, (DeviceHandle, DeviceDescriptor), 1>;

pub struct USBHostDriver<'a, D: HostDriver, const MAX_DEVICES: usize = 4> {
    pipe: &'a USBHostPipe<D, 16>,
    new_dev: &'a DeviceChannel,
}

impl<'a, D: HostDriver, const MAX_DEVICES: usize> USBHostDriver<'a, D, MAX_DEVICES> {
    pub fn new(pipe: &'a USBHostPipe<D, 16>, new_dev: &'a DeviceChannel) -> Self {
        Self { pipe, new_dev }
    }

    pub async fn run<
        Fut: Future<Output = Result<(), UsbHostError>>,
        F: Fn(HidKbd, &'a USBHostPipe<D, 16>) -> Fut,
    >(
        &mut self,
        f: F,
    ) {
        let poller = StaticUnpinPoller::<Fut, MAX_DEVICES>::new();
        let mut poller = pin!(poller);

        loop {
            let new_dev_fut = self.new_dev.receive();
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
                        // The slot is already cleared by the select_pin_array implementation
                    }
                    Either::Second(None) => {
                        continue;
                    }
                }
            };
            let kbd = HidKbd::try_attach(self.pipe, device, descriptor).await;
            match kbd {
                Ok(kbd) => {
                    // Find an empty slot for the new device
                    if let Err(e) = poller.as_mut().insert(f(kbd, self.pipe)) {
                        error!("No empty slots available for new device: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to attach keyboard: {}", e);
                }
            }
        }
    }
}
