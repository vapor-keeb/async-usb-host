#[cfg_attr(target_endian = "little", repr(C, packed))]
#[derive(Default, Clone)]
pub struct HubDescriptor {
    pub length: u8,
    pub descriptor_type: u8,

    /// bNbrPorts
    pub number_of_ports: u8,

    /// wHubCharacteristics
    pub hub_characteristics: u16,

    /// Time in 2ms interval for power on to power good
    pub power_on_to_power_good_time: u8,

    pub hub_controller_current: u8,
}

#[cfg(feature = "defmt")]
impl defmt::Format for HubDescriptor {
    fn format(&self, f: defmt::Formatter) {
        defmt::write!(
            f,
            "HubDescriptor {{ length: {}, type: {}, ports: {}, chars: {:#x}, power_time: {}ms, current: {}mA }}",
            self.length,
            self.descriptor_type,
            self.number_of_ports,
            { self.hub_characteristics },
            self.power_on_to_power_good_time * 2,
            self.hub_controller_current
        )
    }
}
