use crate::types::Bcd16;

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
#[derive(Copy, Clone)]
#[repr(u8)]
pub enum DescriptorType {
    Device = 1,
    Configuration = 2,
    String = 3,
    Interface = 4,
    Endpoint = 5,
}

impl TryFrom<u8> for DescriptorType {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, ()> {
        match value {
            1 => Ok(Self::Device),
            2 => Ok(Self::Configuration),
            3 => Ok(Self::String),
            4 => Ok(Self::Interface),
            5 => Ok(Self::Endpoint),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Descriptor<'d> {
    DeviceDescriptor(&'d DeviceDescriptor),
}

#[derive(Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub enum ParsingError {
    IncompleteDeviceDescriptor { max_packet_size: u8 },
    Incomplete,
    InvalidLength,
    UnknownType(u8),
}

pub fn parse_descriptor<'a>(buf: &'a [u8]) -> Result<Descriptor<'a>, ParsingError> {
    if buf.len() < core::mem::size_of::<DescriptorHeader>() {
        return Err(ParsingError::Incomplete);
    }
    // SAFETY: [`DescriptorHeader`] is packed, does not require alignment,
    // size is checked above
    let header: &'a DescriptorHeader = unsafe { core::mem::transmute(buf.as_ptr()) };
    let desc_type = DescriptorType::try_from(header.descriptor_type)
        .map_err(|_| ParsingError::UnknownType(header.descriptor_type))?;
    match desc_type {
        DescriptorType::Device => {
            if header.length as usize != core::mem::size_of::<DeviceDescriptor>() {
                Err(ParsingError::InvalidLength)
            } else if buf.len()
                < core::mem::offset_of!(DeviceDescriptor, max_packet_size)
                    + core::mem::size_of::<u8>()
            {
                Err(ParsingError::Incomplete)
            } else {
                // SAFETY: the transmute itself is unsafe. But in the `if` branch
                // we are guaranteed that DeviceDescriptor::max_packet_size is at least
                // within bound, because of the above check.
                // In the else branch we know that the buffer is large enough
                unsafe {
                    let dev_desc: &'a DeviceDescriptor = core::mem::transmute(buf.as_ptr());
                    if buf.len() < header.length as usize {
                        Err(ParsingError::IncompleteDeviceDescriptor {
                            max_packet_size: dev_desc.max_packet_size,
                        })
                    } else {
                        debug_assert!(
                            header.length as usize == core::mem::size_of::<DeviceDescriptor>()
                        );
                        Ok(Descriptor::DeviceDescriptor(dev_desc))
                    }
                }
            }
        }
        DescriptorType::Configuration => panic!(),
        DescriptorType::String => panic!(),
        DescriptorType::Interface => panic!(),
        DescriptorType::Endpoint => panic!(),
    }
}

#[repr(C, packed)]
struct DescriptorHeader {
    length: u8,
    descriptor_type: u8,
}

/// A device descriptor describes general information about a USB device. It includes information that applies
/// globally to the device and all of the device’s configurations. A USB device has only one device descriptor.
// #[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
#[repr(C, packed)]
pub struct DeviceDescriptor {
    pub length: u8,
    pub descriptor_type: DescriptorType,
    /// USB Specification Release Number in Binary-Coded Decimal (i.e., 2.10 is 210H).
    ///
    /// This field identifies the release of the USB Specification with which the device and its descriptors are compliant.
    pub usb_release: Bcd16,

    /// Class code (assigned by the USB-IF).
    ///
    /// If this field is reset to zero, each interface within a configuration specifies its own
    /// class information and the various interfaces operate independently.
    ///
    /// If this field is set to a value between 1 and FEH, the device supports different class
    /// specifications on different interfaces and the interfaces may not operate independently.
    ///
    /// This value identifies the class definition used for the aggregate interfaces.
    /// If this field is set to FFH, the device class is vendor-specific.
    pub device_class: u8,

    /// Subclass code (assigned by the USB-IF).
    ///
    /// These codes are qualified by the value of the bDeviceClass field.
    /// If the bDeviceClass field is reset to zero, this field must also be reset to zero.
    /// If the bDeviceClass field is not set to FFH, all values are reserved for assignment by the USB-IF.
    pub device_sub_class: u8,

    /// Protocol code (assigned by the USB-IF).
    /// These codes are qualified by the value of the bDeviceClass and the bDeviceSubClass fields.
    ///
    /// If a device supports class-specific protocols on a device basis as opposed to an interface
    /// basis, this code identifies the protocols that the device uses as defined by the
    /// specification of the device class.
    /// If this field is reset to zero, the device does not use class-specific protocols on a
    /// device basis. However, it may use class- specific protocols on an interface basis.
    /// If this field is set to FFH, the device uses a vendor-specific protocol on a device basis.
    pub device_protocol: u8,

    /// Maximum packet size for endpoint zero
    ///
    /// (only 8, 16, 32, or 64 are valid)
    pub max_packet_size: u8,

    /// Vendor ID (assigned by the USB-IF)
    pub id_vendor: u16,

    /// Product ID (assigned by the manufacturer)
    pub id_product: u16,

    /// Device release number in binary-coded decimal
    pub device_release: Bcd16,

    /// Index of string descriptor describing manufacturer
    pub manufacturer_index: u8,

    /// Index of string descriptor describing product
    pub product_index: u8,

    /// Index of string descriptor describing the device's serial number
    pub serial_number_index: u8,

    /// Number of possible configurations
    pub num_configurations: u8,
}

#[cfg(feature = "defmt")]
impl defmt::Format for DeviceDescriptor {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(
            fmt,
            "USB Device Descriptor {{
\tusb_release: {},
\tdevice_class: {=u8:#x},
\tdevice_sub_class: {=u8:#x},
\tdevice_protocol: {=u8:#x},
\tmax_packet_size: {},
\tid_vendor: {=u16:#x},
\tid_product: {=u16:#x},
\tdevice_release: {},
\tmanufacturer_index: {},
\tproduct_index: {},
\tserial_number_index: {},
\tnum_configurations: {}
}}",
            { self.usb_release },
            self.device_class,
            self.device_sub_class,
            self.device_protocol,
            self.max_packet_size,
            { self.id_vendor },
            { self.id_product },
            { self.device_release },
            self.manufacturer_index.clone(),
            self.product_index.clone(),
            self.serial_number_index.clone(),
            self.num_configurations.clone(),
        )
    }
}

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct ConfigurationAttributes(u8);

/// Part of the [`ConfigurationDescriptor`]
impl ConfigurationAttributes {
    /// A device configuration reports whether the configuration is bus-powered or self-powered.
    ///
    /// Device status reports whether the device is currently self-powered. If a device is
    /// disconnected from its external power source, it updates device status to indicate that
    /// it is no longer self-powered.
    pub fn self_powered(&self) -> bool {
        (self.0 >> 6) & 1 == 1
    }

    /// Device supports remote wakeup
    pub fn remote_wakeup(&self) -> bool {
        (self.0 >> 5) & 1 == 1
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for ConfigurationAttributes {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(
            fmt,
            "ConfigurationAttributes {{ self_powered: {=bool}, remote_wakeup: {=bool} }}",
            self.self_powered(),
            self.remote_wakeup()
        );
    }
}

/// The configuration descriptor describes information about a specific device configuration.
///
/// The descriptor contains a bConfigurationValue field with a value that, when used as a parameter
/// to the SetConfiguration() request, causes the device to assume the described configuration.
#[derive(Clone)]
#[repr(C, packed)]
pub struct ConfigurationDescriptor {
    /// Total length of data returned for this configuration.
    ///
    /// Includes the combined length of all descriptors (configuration, interface,
    /// endpoint, and class- or vendor-specific) returned for this configuration.
    pub total_length: u16,

    /// Number of interfaces supported by this configuration
    pub num_interfaces: u8,

    /// Value to use as an argument to the SetConfiguration() request to select this configuration
    pub value: u8,

    /// Index of string descriptor describing this configuration
    pub index: u8,

    /// Configuration characteristics
    pub attributes: ConfigurationAttributes,

    /// Maximum power consumption of the USB device from the bus in this specific configuration when the device is fully operational.
    ///
    /// Expressed in 2 mA units (i.e., 50 = 100 mA).
    pub max_power: u8,
}

#[cfg(feature = "defmt")]
impl defmt::Format for ConfigurationDescriptor {
    fn format(&self, f: defmt::Formatter) {
        defmt::write!(
            f,
            "ConfigurationDescriptor {{ 
\ttotal_length: {}, 
\tnum_interfaces: {}, 
\tvalue: {}, 
\tindex: {}, 
\tattributes: {}, 
\tmax_power: {} mA 
}}",
            { self.total_length },
            self.num_interfaces,
            self.value,
            self.index,
            self.attributes,
            self.max_power * 2
        )
    }
}
