use crate::{descriptor::EndpointDescriptor, DeviceHandle};

/// Represents a 16-bit binary-coded-decimal value
///
/// A 16-bit BCD represents 4 decimal digits (0-9).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
#[repr(transparent)]
pub struct Bcd16(pub u16);

impl Bcd16 {
    /// Returns the four contained digits as separate numbers
    ///
    /// Each of the returned numbers is in the 0-9 range.
    pub fn to_digits(self) -> [u8; 4] {
        [
            ((self.0 >> 12) & 0xF) as u8,
            ((self.0 >> 8) & 0xF) as u8,
            ((self.0 >> 4) & 0xF) as u8,
            (self.0 & 0xF) as u8,
        ]
    }

    pub fn is_valid(value: u16) -> bool {
        (value >> 12 & 0xF) < 10
            && (value >> 8 & 0xF) < 10
            && (value >> 4 & 0xF) < 10
            && (value & 0xF) < 10
    }

    pub(crate) fn from_le_bytes(data: [u8; 2]) -> Bcd16 {
        Bcd16(u16::from_le_bytes(data))
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for Bcd16 {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(
            fmt,
            "{}{}{}{}",
            (self.0 >> 12) & 0xF,
            (self.0 >> 8) & 0xF,
            (self.0 >> 4) & 0xF,
            self.0 & 0xF,
        )
    }
}

#[repr(u8)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pid {
    OUT = 0b0001,
    IN = 0b1001,
    SOF = 0b0101,
    SETUP = 0b1101,

    DATA0 = 0b0011,
    DATA1 = 0b1011,
    DATA2 = 0b0111,
    MDATA = 0b1111,

    ACK = 0b0010,
    NAK = 0b1010,
    STALL = 0b1110,
    NYET = 0b0110,

    // PRE and ERR are both 0b1100
    // Ignoring for now
    SPLIT = 0b1000,
    PING = 0b0100,
    Rsvd = 0b0000,
}

impl TryFrom<u8> for Pid {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0b0001 => Ok(Pid::OUT),
            0b1001 => Ok(Pid::IN),
            0b0101 => Ok(Pid::SOF),
            0b1101 => Ok(Pid::SETUP),

            0b0011 => Ok(Pid::DATA0),
            0b1011 => Ok(Pid::DATA1),
            0b0111 => Ok(Pid::DATA2),
            0b1111 => Ok(Pid::MDATA),

            0b0010 => Ok(Pid::ACK),
            0b1010 => Ok(Pid::NAK),
            0b1110 => Ok(Pid::STALL),
            0b0110 => Ok(Pid::NYET),

            0b1000 => Ok(Pid::SPLIT),
            0b0100 => Ok(Pid::PING),
            0b0000 => Ok(Pid::Rsvd),

            _ => Err(()),
        }
    }
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UsbSpeed {
    LowSpeed,
    FullSpeed,
    HighSpeed,
}

impl UsbSpeed {
    pub fn is_classic(&self) -> bool {
        matches!(self, UsbSpeed::LowSpeed | UsbSpeed::FullSpeed)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub enum EndpointDirection {
    In,
    Out,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub struct EndpointAddress {
    pub number: u8,
    pub direction: EndpointDirection,
}

impl From<&EndpointDescriptor> for EndpointAddress {
    fn from(value: &EndpointDescriptor) -> Self {
        let number = value.b_endpoint_address & 0xF;
        let direction = if value.b_endpoint_address & 0x80 == 0 {
            EndpointDirection::Out
        } else {
            EndpointDirection::In
        };
        EndpointAddress { number, direction }
    }
}

#[derive(Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub struct InterruptChannel {
    pub(crate) device_handle: DeviceHandle,
    pub(crate) endpoint_address: EndpointAddress,
    pub(crate) tog: DataTog,
}

impl InterruptChannel {
    pub fn new(device_handle: DeviceHandle, endpoint_address: EndpointAddress) -> Self {
        Self {
            device_handle,
            endpoint_address,
            tog: DataTog::DATA0,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub enum DataTog {
    DATA0,
    DATA1,
}

impl DataTog {
    pub fn next(&mut self) {
        *self = match self {
            DataTog::DATA0 => DataTog::DATA1,
            DataTog::DATA1 => DataTog::DATA0,
        };
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub struct PortInfo {
    /// 7-bit USB address and the highest bit being a "valid" flag
    valid_parent_addr: u8,
    port_on_parent: u8,
}

impl PortInfo {
    pub fn invalid() -> Self {
        PortInfo {
            valid_parent_addr: 0,
            port_on_parent: 0,
        }
    }

    // hub constructs PortInfo on detach
    pub(crate) fn new(valid_parent_addr: u8, port_on_parent: u8) -> Self {
        PortInfo {
            valid_parent_addr,
            port_on_parent,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.valid_parent_addr & 0x80 == 0
    }

    pub fn parent_addr(&self) -> Option<u8> {
        if self.is_empty() {
            None
        } else {
            Some(self.valid_parent_addr & 0x7F)
        }
    }

    pub fn port(&self) -> u8 {
        self.port_on_parent
    }
}

#[derive(Copy, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub struct DevInfo {
    port: PortInfo,
    // tt addr, port
    // TODO: clean up
    transaction_translator: Option<(u8, u8)>,
    speed: UsbSpeed,
}

impl DevInfo {
    pub fn empty() -> Self {
        DevInfo {
            port: PortInfo::invalid(),
            transaction_translator: None,
            speed: UsbSpeed::LowSpeed,
        }
    }

    pub fn root_device(speed: UsbSpeed) -> Self {
        DevInfo {
            port: PortInfo::new(0x80, 0),
            transaction_translator: None,
            speed,
        }
    }

    pub fn new(
        addr: u8,
        port: u8,
        transaction_translator: Option<(u8, u8)>,
        speed: UsbSpeed,
    ) -> Self {
        assert!((addr & 0x7F) != 0);
        DevInfo {
            port: PortInfo::new(0x80 | addr, port),
            transaction_translator,
            speed,
        }
    }

    pub fn port(&self) -> PortInfo {
        self.port
    }

    pub fn transaction_translator(&self) -> Option<(u8, u8)> {
        self.transaction_translator
    }

    pub fn speed(&self) -> UsbSpeed {
        self.speed
    }
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EndpointType {
    Control = 0x0,
    Isochronous = 0x1,
    Bulk = 0x2,
    Interrupt = 0x3,
}
