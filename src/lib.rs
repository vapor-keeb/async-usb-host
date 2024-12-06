#![no_std]
use core::marker::PhantomData;

use defmt::Format;

#[cfg_attr(feature = "defmt", derive(Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub enum UsbHostError {
    NAK,
    WrongTog,
    STALL,
    Unknown,
    BufferOverflow,
}

trait Bus {
    fn reset(&mut self);

    fn setup(&mut self, buf: &[u8; 8]) -> Result<(), UsbHostError>;
}

trait Driver {
    type Bus: Bus;

    fn start(self) -> Self::Bus;
}

struct Host<D: Driver> {
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
