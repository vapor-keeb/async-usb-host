#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UsbBaseClass {
    /// Unspecified class – use interface descriptors to determine functionality
    Unspecified           = 0x00,
    /// Audio Devices (e.g., speakers, microphones, etc.)
    Audio                 = 0x01,
    /// Communications and CDC Control (e.g., modems, USB-to-Ethernet adapters)
    CDCControl            = 0x02,
    /// Human Interface Devices (e.g., keyboards, mice)
    HID                   = 0x03,
    /// Physical Interface Devices (e.g., force-feedback controllers)
    Physical              = 0x05,
    /// Still Imaging Devices (e.g., digital cameras, scanners)
    StillImaging          = 0x06,
    /// Printer Devices
    Printer               = 0x07,
    /// Mass Storage Devices (e.g., USB flash drives)
    MassStorage           = 0x08,
    /// USB Hub Devices
    Hub                   = 0x09,
    /// CDC Data (used together with CDC Control for networking functionality)
    CDCData               = 0x0A,
    /// Smart Card Devices (e.g., USB smart card readers)
    SmartCard             = 0x0B,
    /// Content Security Devices (e.g., fingerprint readers)
    ContentSecurity       = 0x0D,
    /// Video Devices (e.g., webcams)
    Video                 = 0x0E,
    /// Personal Healthcare Devices (PHDC) (e.g., pulse monitors)
    PersonalHealthcare    = 0x0F,
    /// Audio/Video Devices (used for combined A/V functionalities)
    AudioVideo            = 0x10,
    /// Billboard devices (for describing alternate modes in USB Type-C)
    Billboard             = 0x11,
    /// USB Type-C Bridge Class (Interface)
    TypeCBridge           = 0x12,
    /// USB Bulk Display Protocol Device Class (Interface)
    BulkDisplay           = 0x13,
    /// MCTP over USB Protocol Endpoint Device Class (Interface)
    MCTP                  = 0x14,
    /// I3C Device Class (Interface)
    I3C                   = 0x3C,
    /// Diagnostic Devices (used for testing or compliance)
    Diagnostic            = 0xDC,
    /// Wireless Controller Devices (e.g., Bluetooth adapters)
    WirelessController    = 0xE0,
    /// Miscellaneous devices that do not fit in other categories
    Miscellaneous         = 0xEF,
    /// Application-specific devices
    ApplicationSpecific   = 0xFE,
    /// Vendor-specific devices – require vendor-supplied drivers
    VendorSpecific        = 0xFF,
}

impl TryFrom<u8> for UsbBaseClass {
    type Error = ();

    fn try_from(code: u8) -> Result<Self, Self::Error> {
        match code {
            0x00 => Ok(UsbBaseClass::Unspecified),
            0x01 => Ok(UsbBaseClass::Audio),
            0x02 => Ok(UsbBaseClass::CDCControl),
            0x03 => Ok(UsbBaseClass::HID),
            0x05 => Ok(UsbBaseClass::Physical),
            0x06 => Ok(UsbBaseClass::StillImaging),
            0x07 => Ok(UsbBaseClass::Printer),
            0x08 => Ok(UsbBaseClass::MassStorage),
            0x09 => Ok(UsbBaseClass::Hub),
            0x0A => Ok(UsbBaseClass::CDCData),
            0x0B => Ok(UsbBaseClass::SmartCard),
            0x0D => Ok(UsbBaseClass::ContentSecurity),
            0x0E => Ok(UsbBaseClass::Video),
            0x0F => Ok(UsbBaseClass::PersonalHealthcare),
            0x10 => Ok(UsbBaseClass::AudioVideo),
            0x11 => Ok(UsbBaseClass::Billboard),
            0x12 => Ok(UsbBaseClass::TypeCBridge),
            0x13 => Ok(UsbBaseClass::BulkDisplay),
            0x14 => Ok(UsbBaseClass::MCTP),
            0x3C => Ok(UsbBaseClass::I3C),
            0xDC => Ok(UsbBaseClass::Diagnostic),
            0xE0 => Ok(UsbBaseClass::WirelessController),
            0xEF => Ok(UsbBaseClass::Miscellaneous),
            0xFE => Ok(UsbBaseClass::ApplicationSpecific),
            0xFF => Ok(UsbBaseClass::VendorSpecific),
            _    => Err(()),
        }
    }
}

impl From<UsbBaseClass> for u8 {
    fn from(class: UsbBaseClass) -> Self {
        class as u8
    }
}
