#![no_std]
use core::{borrow::Borrow, marker::PhantomData};

use descriptor::{parse_descriptor, DeviceDescriptor};
use errors::UsbHostError;

pub mod descriptor;
pub mod errors;
mod macros;
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
    async fn poll(&mut self) -> Event;
    async fn setup(&mut self, buf: &[u8; 8]) -> Result<(), UsbHostError>;
    async fn data_in(&mut self, buf: &mut [u8]) -> Result<usize, UsbHostError>;
    async fn data_out(&mut self, buf: &[u8]) -> Result<(), UsbHostError>;
}

pub trait Driver {
    type Bus: Bus;

    fn start(self) -> Self::Bus;
}

pub struct Host<D: Driver> {
    phantom: PhantomData<D>,
    bus: D::Bus,
}

impl<D: Driver> Host<D> {
    pub fn new(driver: D) -> Self {
        let bus = driver.start();

        Host {
            bus,
            phantom: PhantomData,
        }
    }
}

impl<D: Driver> Host<D> {
    async fn get_device_descriptor<'a>(
        &mut self,
        buf: &'a mut [u8; 18],
    ) -> Result<&'a DeviceDescriptor, UsbHostError> {
        let mut bytes_read = 0usize;
        let in_result = self.bus.data_in(buf).await?;
        bytes_read += in_result;

        let max_packet_size = match parse_descriptor(&buf[..bytes_read]) {
            Ok(desc) => match desc {
                descriptor::Descriptor::DeviceDescriptor(desc) => {
                    return Ok(desc);
                }
            },
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
                .bus
                .data_in(unsafe {
                    core::slice::from_raw_parts_mut(
                        chopped_off_buf.as_ptr() as *mut u8,
                        chopped_off_buf.len(),
                    )
                })
                .await?;
            bytes_read += in_result;
        }

        debug_assert!(bytes_read == 18);

        match parse_descriptor(buf) {
            Ok(desc) => match desc {
                descriptor::Descriptor::DeviceDescriptor(desc) => Ok(desc),
            },
            Err(e) => Err(UsbHostError::ParsingError(e)),
        }
    }

    pub async fn run(mut self) {
        loop {
            let event = self.bus.poll().await;
            match event {
                Event::DeviceAttach => {
                    self.bus.reset().await;
                    let buf = [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x40, 0x00];
                    unwrap!(self.bus.setup(&buf).await);
                    let mut buffer: [u8; 18] = [0u8; 18];
                    match self.get_device_descriptor(&mut buffer).await {
                        Ok(d) => {
                            trace!("res: {}", d);
                        }
                        Err(e) => debug!("{}", e),
                    }
                    match self.bus.data_out(&[]).await {
                        Ok(_) => {}
                        Err(e) => debug!("{}", e),
                    }
                }
                Event::DeviceDetach => {}
                Event::Suspend => {}
                Event::Resume => {}
            }
        }
    }
}
