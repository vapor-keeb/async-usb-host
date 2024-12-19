
#[cfg_attr(target_endian="little", repr(C, packed))]
pub struct HubDescriptor {
    length: u8,
    descriptor_type: u8,
    
    /// bNbrPorts
    number_of_ports: u8,

    /// wHubCharacteristics
    hub_characteristics: u16,

    /// Time in 2ms interval for power on to power good
    power_on_to_power_good_time: u8,

    hub_controller_current: u8,
}