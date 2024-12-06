/// Represents a 16-bit binary-coded-decimal value
///
/// A 16-bit BCD represents 4 decimal digits (0-9).
#[derive(Clone, Copy, PartialEq, Eq)]
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

    pub(crate) fn is_valid(value: u16) -> bool {
        (value >> 12 & 0xF) < 10
            && (value >> 8 & 0xF) < 10
            && (value >> 4 & 0xF) < 10
            && (value & 0xF) < 10
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
