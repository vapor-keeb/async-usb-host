use core::{future::poll_fn, pin::pin};

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

use crate::{
    descriptor::DeviceDescriptor, errors::UsbHostError, pipe::USBHostPipe, DeviceHandle, Driver,
};

/// USB Hub class driver, private because it is only used by the main driver.
pub(crate) mod hub;
pub mod kbd;