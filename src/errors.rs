use crate::descriptor::ParsingError;

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub enum UsbHostError {
    NAK,
    WrongTog,
    STALL,
    Unknown,
    BufferOverflow,
    ParsingError(ParsingError)
}