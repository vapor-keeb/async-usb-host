use core::future::Future;

use crate::Driver;

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
}

pub(crate) struct BusWrap<D: Driver>(D::Bus);

impl<D: Driver> BusWrap<D> {
    pub fn new(bus: D::Bus) -> Self {
        Self(bus)
    }

    pub async fn poll(&mut self) -> Event {
        match self.0.poll().await {
            Event::DeviceAttach => {
                self.0.reset().await;
                embassy_time::Timer::after_millis(500).await;

                Event::DeviceAttach
            }
            e => e,
        }
    }

    pub async fn wait_until_detach(&mut self) {
        loop {
            match self.0.poll().await {
                Event::DeviceDetach => return,
                _ => {}
            }
        }
    }
}
