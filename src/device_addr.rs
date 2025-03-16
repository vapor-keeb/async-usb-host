use crate::types::DevInfo;

#[derive(Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub struct DeviceHandle {
    address: u8,
    max_packet_size: u16,
    parent: DevInfo,
}

impl DeviceHandle {
    pub fn address(&self) -> u8 {
        self.address
    }

    pub fn max_packet_size(&self) -> u16 {
        self.max_packet_size
    }
}

pub(crate) struct DeviceAddressManager<const NR_DEVICES: usize> {
    // 1-based indexing
    info: [DevInfo; NR_DEVICES],
}

impl<const NR_DEVICES: usize> DeviceAddressManager<NR_DEVICES> {
    // Construct an allocator with all addresses except 0 occupied.
    pub fn new() -> Self {
        Self {
            info: [DevInfo::empty(); NR_DEVICES],
        }
    }

    pub fn alloc_device_address(&mut self, max_packet_size: u16, parent: DevInfo) -> DeviceHandle {
        debug_assert!(!parent.is_empty());
        for i in 0..NR_DEVICES {
            if self.info[i].is_empty() {
                self.info[i] = parent;
                return DeviceHandle {
                    address: i as u8 + 1,
                    max_packet_size,
                    parent,
                };
            }
        }
        panic!("No address available");
    }

    pub fn free_address(&mut self, device_handle: DeviceHandle) {
        debug_assert!(!self.info[device_handle.address as usize - 1].is_empty());
        self.info[device_handle.address as usize - 1] = DevInfo::empty();
    }
}
