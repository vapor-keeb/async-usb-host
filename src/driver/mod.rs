/// USB Hub class driver, private because it is only used by the main driver.
///
use core::{error, future::Future, marker::PhantomData, pin::pin};

use crate::{
    descriptor::{Descriptor, DescriptorIterator, DeviceDescriptor},
    driver::kbd::HidKbd,
    errors::UsbHostError,
    futures::StaticUnpinPoller,
    pipe::USBHostPipe,
    DeviceHandle, HostDriver,
};
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

pub mod dfu;
pub(crate) mod hub;
pub mod kbd;

pub type DeviceChannel = Channel<CriticalSectionRawMutex, (DeviceHandle, DeviceDescriptor), 1>;

#[allow(async_fn_in_trait)]
pub trait USBHostDeviceDriver: Sized {
    const CLASS: Option<u8> = None;
    const SUBCLASS: Option<u8> = None;
    const VENDOR: Option<u16> = None;
    const PRODUCT: Option<u16> = None;

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

pub async fn get_configuration_descriptor<'a, HD: HostDriver, const NR_DEVICES: usize>(
    device_handle: DeviceHandle,
    buf: &'a mut [u8],
    pipe: &USBHostPipe<HD, NR_DEVICES>,
) -> Result<impl Iterator<Item = Result<Descriptor<'a>, UsbHostError>> + 'a, UsbHostError> {
    let len = pipe
        .control_transfer(
            device_handle,
            &crate::request::Request::get_configuration_descriptor(
                // TODO: take an index for configuration
                0,
                buf.len() as u16,
            ),
            buf,
        )
        .await?;
    let buf_len = buf.len();

    let mut iter = DescriptorIterator::new(&mut buf[..len]).peekable();
    match iter.peek() {
        Some(Ok(Descriptor::Configuration(c))) => {
            if c.total_length as usize == len {
                // If the total length matches, we can return the iterator
                Ok(iter)
            } else if buf_len < c.total_length as usize {
                Err(UsbHostError::BufferOverflow)
            } else {
                error!("Configuration descriptor length mismatch: expected {}, got {}", {c.total_length}, len);
                Ok(iter)
            }
        }
        Some(Ok(_)) => {
            Err(UsbHostError::InvalidResponse)
        }
        Some(Err(e)) => {
            Err(e.clone())
        }
        None => {
            Err(UsbHostError::InvalidResponse)
        }
    }
}
