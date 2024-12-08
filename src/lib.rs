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

        match parse_descriptor(&buf[..bytes_read]) {
            Ok(desc) => match desc {
                descriptor::Descriptor::DeviceDescriptor(desc) => {
                    return Ok(desc);
                }
            },
            Err(descriptor::ParsingError::IncompleteDeviceDescriptor { max_packet_size }) => {}
            Err(e) => todo!(),
        }

        // SAFETY: This is sane as long as core::slice::as_slice implemntation is 
        // equal to core::slice::as_mut_slice. Which I can not foresee why it 
        // would ever change. 
        // Otherwise, since the buf is mut already, and the above immutable borrow
        // must've been release, otherwise this code is unreachable (see above return).
        let in_result = self
            .bus
            .data_in(unsafe { core::mem::transmute(buf.as_slice()) })
            .await?;

        todo!()
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
