/// USB Hub class driver, private because it is only used by the main driver.
///
use core::{future::Future, pin::Pin};

use crate::{
    descriptor::DeviceDescriptor, driver::kbd::HidKbd, errors::UsbHostError, futures::select_pin_array, pipe::USBHostPipe, DeviceHandle, HostDriver
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
        let mut device_futures: [Option<Fut>; MAX_DEVICES] = core::array::from_fn(|_| None);
        let mut pinned: Pin<&mut [Option<Fut>; MAX_DEVICES]> =
            unsafe { Pin::new_unchecked(&mut device_futures) };

        loop {
            let new_dev_fut = self.new_dev.receive();
            match select(new_dev_fut, select_pin_array(&mut pinned)).await {
                Either::First((device, descriptor)) => {
                    let kbd = HidKbd::try_attach(self.pipe, device, descriptor).await;
                    match kbd {
                        Ok(kbd) => {
                            // Find an empty slot for the new device
                            if let Some(empty_slot) = unsafe { pinned.as_mut().get_unchecked_mut() }
                                .iter_mut()
                                .position(|slot| slot.is_none())
                            {
                                unsafe {
                                    pinned.as_mut().get_unchecked_mut()[empty_slot] =
                                        Some(f(kbd, self.pipe));
                                }
                                trace!("Device added to slot {}", empty_slot);
                            } else {
                                error!("No empty slots available for new device");
                            }
                        }
                        Err(e) => {
                            error!("Failed to attach keyboard: {}", e);
                        }
                    }
                }
                Either::Second((result, idx)) => {
                    match result {
                        Ok(_) => trace!("Device at slot {} completed successfully", idx),
                        Err(e) => error!("Device error at slot {}: {}", idx, e),
                    }
                    // The slot is already cleared by the select_pin_array implementation
                }
            }
        }
    }
}
