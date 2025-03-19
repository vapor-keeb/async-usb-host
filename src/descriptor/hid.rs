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
