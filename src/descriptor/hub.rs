use crate::types::UsbSpeed;

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

#[derive(Default, Clone, Copy)]
pub struct HubPortStatus(u16);

impl HubPortStatus {
    pub fn connected(&self) -> bool {
        self.0 & 0x1 != 0
    }

    pub fn enabled(&self) -> bool {
        self.0 & 0x2 != 0
    }

    pub fn suspended(&self) -> bool {
        self.0 & 0x4 != 0
    }

    pub fn over_current(&self) -> bool {
        self.0 & 0x8 != 0
    }

    pub fn reset(&self) -> bool {
        self.0 & 0x10 != 0
    }

    pub fn power(&self) -> bool {
        self.0 & 0x100 != 0
    }

    pub fn low_speed(&self) -> bool {
        self.0 & 0x200 != 0
    }

    pub fn high_speed(&self) -> bool {
        self.0 & 0x400 != 0
    }

    pub fn speed(&self) -> UsbSpeed {
        if self.high_speed() {
            UsbSpeed::HighSpeed
        } else if self.low_speed() {
            UsbSpeed::LowSpeed
        } else {
            UsbSpeed::FullSpeed
        }
    }
    pub fn test_mode(&self) -> bool {
        self.0 & 0x800 != 0
    }

    pub fn indicator(&self) -> bool {
        self.0 & 0x1000 != 0
    }
}

impl From<u16> for HubPortStatus {
    fn from(val: u16) -> Self {
        HubPortStatus(val)
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for HubPortStatus {
    fn format(&self, f: defmt::Formatter) {
        defmt::write!(
            f,
            "HubPortStatus {{ connected: {}, enabled: {}, suspended: {}, over_current: {}, reset: {}, power: {}, low_speed: {}, high_speed: {}, test_mode: {}, indicator: {} }}",
            self.connected(),
            self.enabled(),
            self.suspended(),
            self.over_current(),
            self.reset(),
            self.power(),
            self.low_speed(),
            self.high_speed(),
            self.test_mode(),
            self.indicator()
        )
    }
}

#[derive(Clone, Copy)]
pub struct HubPortStatusChange(u16);

impl HubPortStatusChange {
    pub fn connection(&self) -> bool {
        self.0 & 0x1 != 0
    }

    pub fn enable(&self) -> bool {
        self.0 & 0x2 != 0
    }

    pub fn suspend(&self) -> bool {
        self.0 & 0x4 != 0
    }

    pub fn over_current(&self) -> bool {
        self.0 & 0x8 != 0
    }

    pub fn reset(&self) -> bool {
        self.0 & 0x10 != 0
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for HubPortStatusChange {
    fn format(&self, f: defmt::Formatter) {
        defmt::write!(
            f,
            "HubPortStatusChange {{ connection: {}, enable: {}, suspend: {}, over_current: {}, reset: {} }}",
            self.connection(),
            self.enable(),
            self.suspend(),
            self.over_current(),
            self.reset()
        )
    }
}

impl From<u16> for HubPortStatusChange {
    fn from(val: u16) -> Self {
        HubPortStatusChange(val)
    }
}


#[repr(u8)]
#[derive(Clone, Copy)]
#[cfg_attr(feature="defmt", derive(defmt::Format))]
pub enum HubPortFeature {
    Connect = 0,
    Enable = 1,
    Suspend = 2,
    OverCurrent = 3,
    Reset = 4,
    Power = 8,
    LowSpeed = 9,
    ChangeConnection = 16,
    ChangeEnable = 17,
    ChangeSuspend = 18,
    ChangeOverCurrent = 19,
    ChangeReset = 20,
    Test = 21,
    Indicator = 22,
}

impl Into<u8> for HubPortFeature {
    fn into(self) -> u8 {
        self as u8
    }
}
