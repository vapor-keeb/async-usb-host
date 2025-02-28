use crate::{descriptor::ParsingError, types::InterruptChannel};

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub enum UsbHostError {
    Unknown,
    NAK,
    WrongTog,
    STALL,
    BufferOverflow,
    ParsingError(ParsingError),
    TransferTimeout,
    InvalidState,
    InvalidResponse,
    InterruptTransferCapacity(InterruptChannel),
}
