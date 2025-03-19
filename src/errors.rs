use crate::descriptor::ParsingError;

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub enum UsbHostError {
    Unknown,
    // USB Device packet level errors
    NAK,
    WrongTog,
    STALL,
    UnexpectedPID,

    // USB Host stack level errors
    BufferOverflow,
    ParsingError(ParsingError),
    TransferTimeout,
    InvalidState,
    InvalidResponse,
    UnexpectedDevice,
    HubCapacity,
    Detached,
}
