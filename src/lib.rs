#![no_std]
use core::marker::PhantomData;

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
    async fn get_device_descriptor<'a, 'b>(buf: &'a mut [u8; 18]) -> Result<&'b DeviceDescriptor, UsbHostError> where 'a : 'b {
        let mut bytes_read = 0usize;
        loop {
                    // let in_result = self.bus.data_in(buf).await?;
                    buf[0] = 1u8;

                    // bytes_read += in_result;

                    let parse_result = parse_descriptor(&buf[..bytes_read]);
                    match &parse_result {
                        Ok(desc) => {
                            match desc {
                                descriptor::Descriptor::DeviceDescriptor(desc) => return Ok(desc),
                            }
                        },
                        Err(descriptor::ParsingError::IncompleteDeviceDescriptor {
                            max_packet_size,
                        }) => {

                        },
                        Err(e) => todo!(),
                    }
                    drop(parse_result);

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
                    trace!("res: {} & {:x}", in_result, buffer);
                }
                Event::DeviceDetach => {}
                Event::Suspend => {}
                Event::Resume => {}
            }
        }
    }
}
