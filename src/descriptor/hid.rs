use crate::types::Bcd16;

#[repr(C, packed)]
pub struct HIDDescriptor {
    pub total_length: u8,
    pub descriptor_type: u8,
    pub bcd_hid: Bcd16,
    pub country_code: u8,
    pub flags: u8,
    pub num_descriptors: u8,
}

// HID descriptor type constant
pub const HID_DESCRIPTOR_TYPE: u8 = 0x21;

impl HIDDescriptor {
    pub fn parse(data: &[u8]) -> Option<Self> {
        // USB uses little-endian, so ensure we're compiling for a compatible target
        #[cfg(target_endian = "big")]
        compile_error!("USB protocol uses little-endian byte order, but compiling for big-endian target");

        if data.len() < core::mem::size_of::<Self>() {
            return None;
        }
        
        // Safety: We've checked the length and the struct is #[repr(C, packed)]
        let descriptor = unsafe {
            core::ptr::read_unaligned(data.as_ptr() as *const Self)
        };
        
        Some(descriptor)
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for HIDDescriptor {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(
            fmt,
            "HIDDescriptor {{
                total_length: {},
                descriptor_type: {},
                bcd_hid: {},
                country_code: {},
                flags: {},
                num_descriptors: {}
            }}",
            self.total_length,
            self.descriptor_type,
            { self.bcd_hid },
            self.country_code,
            self.flags,
            self.num_descriptors
        );
    }
}
