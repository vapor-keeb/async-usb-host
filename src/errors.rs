use crate::descriptor::ParsingError;

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
#[derive(Clone, Copy)]
pub enum UsbHostError {
    Unknown,
    // USB Device packet level errors
    NAK,
    NYET, // Used for split transactions
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
