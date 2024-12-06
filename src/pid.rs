/// USB Protocol Types

#[repr(u8)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PID {
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

impl TryFrom<u8> for PID {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0b0001 => Ok(PID::OUT),
            0b1001 => Ok(PID::IN),
            0b0101 => Ok(PID::SOF),
            0b1101 => Ok(PID::SETUP),

            0b0011 => Ok(PID::DATA0),
            0b1011 => Ok(PID::DATA1),
            0b0111 => Ok(PID::DATA2),
            0b1111 => Ok(PID::MDATA),

            0b0010 => Ok(PID::ACK),
            0b1010 => Ok(PID::NAK),
            0b1110 => Ok(PID::STALL),
            0b0110 => Ok(PID::NYET),

            0b1000 => Ok(PID::SPLIT),
            0b0100 => Ok(PID::PING),
            0b0000 => Ok(PID::Rsvd),

            _ => Err(()),
        }
    }
}
