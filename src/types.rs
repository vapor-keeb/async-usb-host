/// Represents a 16-bit binary-coded-decimal value
///
/// A 16-bit BCD represents 4 decimal digits (0-9).
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature="defmt"), derive(Debug))]
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

#[cfg(feature="defmt")]
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