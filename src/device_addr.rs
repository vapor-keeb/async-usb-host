#[derive(Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub struct DeviceHandle {
    address: u8,
    max_packet_size: u8,
}

impl DeviceHandle {
    pub fn address(&self) -> u8 {
        self.address
    }

    pub fn max_packet_size(&self) -> u8 {
        self.max_packet_size
    }
}

pub(crate) struct DeviceAddressAllocator([u8; 16]);

impl DeviceAddressAllocator {
    // Construct an allocator with all addresses except 0 occupied.
    pub fn new() -> Self {
        let mut alloc = DeviceAddressAllocator([0; 16]);
        // Address 0 is always used;
        alloc.0[0] = 1;
        alloc
    }

    fn set_addr(&mut self, addr: u8, used: bool) {
        debug_assert_ne!(addr, 0);
        let nth_byte = addr / 8;
        let bit_offset = addr % 8;
        let rest = self.0[nth_byte as usize] & !(1u8 << bit_offset);
        self.0[nth_byte as usize] = rest | ((if used { 1 } else { 0 }) << bit_offset);
    }

    pub fn alloc_device_address(&mut self, max_packet_size: u8) -> DeviceHandle {
        let address = {
            let mut address: Option<u8> = None;

            'outer: for nth_byte in 0..8usize {
                // has at least one 0 bit
                if self.0[nth_byte] != 0xFF {
                    let byte = self.0[nth_byte];
                    for bit_offset in 0..8 {
                        if (byte & (1 << bit_offset)) == 0 {
                            address.replace(nth_byte as u8 * 8 + bit_offset);
                            break 'outer;
                        }
                    }
                }
            }
            if let Some(address) = address {
                address
            } else {
                // No address available
                panic!("Ran out of address");
            }
        };
        debug_assert_ne!(address, 0);
        // Mark address as used
        self.set_addr(address, true);

        return DeviceHandle {
            address,
            max_packet_size,
        };
    }

    pub fn free_address(&mut self, device_handle: DeviceHandle) {
        self.set_addr(device_handle.address, false);
    }
}
