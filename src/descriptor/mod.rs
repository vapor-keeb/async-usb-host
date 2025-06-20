use crate::{errors::UsbHostError, types::Bcd16};

pub mod hid;
pub mod hub;

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
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub enum Descriptor<'d> {
    Device(&'d DeviceDescriptor),
    Configuration(&'d ConfigurationDescriptor),
    Endpoint(&'d EndpointDescriptor),
    Interface(&'d InterfaceDescriptor),
    UnknownDescriptor {
        descriptor_type: u8,
        length: u8,
        data: &'d [u8],
    },
}

impl<'a> Descriptor<'a> {
    pub fn device(self) -> Option<&'a DeviceDescriptor> {
        match self {
            Descriptor::Device(dev) => Some(dev),
            _ => None,
        }
    }
    pub fn configuration(self) -> Option<&'a ConfigurationDescriptor> {
        match self {
            Descriptor::Configuration(conf) => Some(conf),
            _ => None,
        }
    }
    pub fn endpoint(self) -> Option<&'a EndpointDescriptor> {
        match self {
            Descriptor::Endpoint(ep) => Some(ep),
            _ => None,
        }
    }
    pub fn interface(self) -> Option<&'a InterfaceDescriptor> {
        match self {
            Descriptor::Interface(intf) => Some(intf),
            _ => None,
        }
    }
}

pub struct DescriptorIterator<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl<'a> DescriptorIterator<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, offset: 0 }
    }
}

impl<'a> Iterator for DescriptorIterator<'a> {
    type Item = Result<Descriptor<'a>, UsbHostError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.buf.len() {
            return None;
        }

        let desc = parse_descriptor(&self.buf[self.offset..]);

        match desc {
            Ok((descriptor, length)) => {
                self.offset += length;
                Some(Ok(descriptor))
            }
            Err(e) => {
                self.offset = self.buf.len();
                Some(Err(e))
            }
        }
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub enum ParsingError {
    IncompleteDeviceDescriptor { max_packet_size: u8 },
    Incomplete,
    InvalidLength,
    UnknownType { length: u8, descriptor_type: u8 },
}

impl Into<UsbHostError> for ParsingError {
    fn into(self) -> UsbHostError {
        UsbHostError::ParsingError(self)
    }
}

fn parse_descriptor<'a>(buf: &'a [u8]) -> Result<(Descriptor<'a>, usize), UsbHostError> {
    #[cfg(not(target_endian = "little"))]
    compile_error!("This function only works for little endian architechture");

    if buf.len() < core::mem::size_of::<DescriptorHeader>() {
        return Err(ParsingError::Incomplete.into());
    }
    // SAFETY: [`DescriptorHeader`] is packed, does not require alignment,
    // size is checked above
    let header: &'a DescriptorHeader = unsafe { core::mem::transmute(buf.as_ptr()) };
    let desc_type = match DescriptorType::try_from(header.descriptor_type) {
        Ok(desc_type) => desc_type,
        Err(_) => {
            return Ok((
                Descriptor::UnknownDescriptor {
                    descriptor_type: header.descriptor_type,
                    length: header.length,
                    data: &buf[..header.length as usize],
                },
                header.length as usize,
            ));
        }
    };
    match desc_type {
        DescriptorType::Device => {
            if header.length as usize != core::mem::size_of::<DeviceDescriptor>() {
                Err(ParsingError::InvalidLength.into())
            } else if buf.len()
                < core::mem::offset_of!(DeviceDescriptor, max_packet_size)
                    + core::mem::size_of::<u8>()
            {
                Err(ParsingError::Incomplete.into())
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
                        }
                        .into())
                    } else {
                        Ok((Descriptor::Device(dev_desc), header.length as usize))
                    }
                }
            }
        }
        DescriptorType::Configuration => {
            if buf.len() < core::mem::size_of::<ConfigurationDescriptor>() {
                Err(ParsingError::Incomplete.into())
            } else {
                Ok((
                    Descriptor::Configuration(unsafe { core::mem::transmute(buf.as_ptr()) }),
                    header.length as usize,
                ))
            }
        }
        DescriptorType::String => panic!(),
        DescriptorType::Interface => {
            if buf.len() < core::mem::size_of::<InterfaceDescriptor>() {
                Err(ParsingError::Incomplete.into())
            } else {
                Ok((
                    Descriptor::Interface(unsafe { core::mem::transmute(buf.as_ptr()) }),
                    header.length as usize,
                ))
            }
        }
        DescriptorType::Endpoint => {
            if buf.len() < core::mem::size_of::<EndpointDescriptor>() {
                Err(ParsingError::Incomplete.into())
            } else {
                Ok((
                    Descriptor::Endpoint(unsafe { core::mem::transmute(buf.as_ptr()) }),
                    header.length as usize,
                ))
            }
        }
    }
}

#[cfg_attr(target_endian = "little", repr(C, packed))]
struct DescriptorHeader {
    length: u8,
    descriptor_type: u8,
}

/// A device descriptor describes general information about a USB device. It includes information that applies
/// globally to the device and all of the device’s configurations. A USB device has only one device descriptor.
// #[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
#[cfg_attr(target_endian = "little", repr(C, packed))]
#[derive(Clone)]
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
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
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
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
#[cfg_attr(target_endian = "little", repr(C, packed))]
pub struct ConfigurationDescriptor {
    pub length: u8,
    pub descriptor_type: DescriptorType,
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

#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
#[repr(C, packed)]
pub struct EndpointDescriptor {
    /// bLength
    ///
    /// Size of this descriptor in bytes.
    pub b_length: u8,

    /// bDescriptorType
    ///
    /// Descriptor type. Always `DESCRIPTOR_TYPE_ENDPOINT (0x05)` for Endpoint Descriptors.
    pub b_descriptor_type: u8,

    /// bEndpointAddress
    ///
    /// The address of the endpoint on the USB device.
    ///
    /// - Bits 3..0: Endpoint Number.
    /// - Bit 7: Endpoint Direction.
    ///   - 0 = OUT endpoint (device-to-host)
    ///   - 1 = IN endpoint (host-to-device)
    pub b_endpoint_address: u8,

    /// bmAttributes
    ///
    /// Attributes of the endpoint.
    ///
    /// - Bits 1..0: Transfer Type
    ///   - 00 = Control
    ///   - 01 = Isochronous
    ///   - 10 = Bulk
    ///   - 11 = Interrupt
    /// - Bits 3..2: For Isochronous Endpoints only - Synchronization Type
    ///   - 00 = No Synchronization
    ///   - 01 = Asynchronous
    ///   - 10 = Adaptive
    ///   - 11 = Synchronous
    /// - Bits 5..4: For Isochronous Endpoints only - Usage Type
    ///   - 00 = Data Endpoint
    ///   - 01 = Feedback Endpoint
    ///   - 10 = Implicit Feedback Data Endpoint
    ///   - 11 = Reserved
    pub bm_attributes: u8,

    /// wMaxPacketSize
    ///
    /// Maximum packet size this endpoint is capable of sending or receiving.
    /// For high-speed and super-speed endpoints, bits 10..0 specify the maximum
    /// packet size. Bits 12..11 are used for high-speed isochronous endpoints
    /// to indicate the number of transactions per microframe (transactions per
    /// microframe = bits 12..11 + 1). For other endpoint types and speeds, these
    /// bits should be zero.
    pub w_max_packet_size: u16,

    /// bInterval
    ///
    /// For interrupt and isochronous endpoints, this value indicates the polling
    /// interval in frames (for full/low speed) or microframes (for high speed)
    /// for interrupt endpoints or the service interval for isochronous endpoints.
    /// The value ranges from 1 to 255.  Interpretation depends on endpoint speed.
    pub b_interval: u8,
    // /// bSynchAddress (Optional, present for Isochronous Endpoints with Synchronization Type)
    // ///
    // /// For isochronous endpoints using synchronization type, this field specifies
    // /// the endpoint address of the synchronization endpoint.  If not used, it's usually 0.
    // pub bSynchAddress: u8,

    // /// bRefreshRate (Optional, present for Isochronous Feedback Endpoints)
    // ///
    // /// For isochronous feedback endpoints, this field specifies the rate at which
    // /// feedback data is refreshed. If not used, it's usually 0.
    // pub bRefreshRate: u8,
}

/// NOT READ BY A HUMAN. 99% generated
#[cfg(feature = "defmt")]
impl defmt::Format for EndpointDescriptor {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "EndpointDescriptor {{");
        defmt::write!(fmt, ".b_length: {},", self.b_length);
        defmt::write!(fmt, ".b_descriptor_type: {:#02X},", self.b_descriptor_type);
        defmt::debug_assert_eq!(
            self.b_descriptor_type,
            0x05,
            "bDescriptorType should be 0x05 for EndpointDescriptor"
        );

        let endpoint_number = self.b_endpoint_address & 0x0F;
        let endpoint_direction = if (self.b_endpoint_address & 0x80) != 0 {
            "IN"
        } else {
            "OUT"
        };
        defmt::write!(
            fmt,
            ".b_endpoint_address: {{ address: {}, direction: {} }},",
            endpoint_number,
            endpoint_direction
        );

        let transfer_type = match self.bm_attributes & 0x03 {
            0b00 => "Control",
            0b01 => "Isochronous",
            0b10 => "Bulk",
            0b11 => "Interrupt",
            _ => "Unknown", // Should not happen
        };
        defmt::write!(fmt, ".bm_attributes: {{ transfer_type: {},", transfer_type);

        if transfer_type == "Isochronous" {
            let sync_type = match (self.bm_attributes >> 2) & 0x03 {
                0b00 => "No Synchronization",
                0b01 => "Asynchronous",
                0b10 => "Adaptive",
                0b11 => "Synchronous",
                _ => "Unknown", // Should not happen
            };
            defmt::write!(fmt, " sync_type: {},", sync_type);

            let usage_type = match (self.bm_attributes >> 4) & 0x03 {
                0b00 => "Data Endpoint",
                0b01 => "Feedback Endpoint",
                0b10 => "Implicit Feedback Data Endpoint",
                0b11 => "Reserved",
                _ => "Unknown", // Should not happen
            };
            defmt::write!(fmt, " usage_type: {},", usage_type);
        }
        defmt::write!(fmt, " }},");

        defmt::write!(fmt, ".w_max_packet_size: {},", { self.w_max_packet_size });
        defmt::write!(fmt, ".b_interval: {},", self.b_interval);

        defmt::write!(fmt, "}}");
    }
}

#[repr(C, packed)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
#[derive(Copy, Clone)] // Derive Format for defmt, and other useful traits
pub struct InterfaceDescriptor {
    /// bLength - Size of this descriptor in bytes.
    pub b_length: u8,

    /// bDescriptorType - Descriptor type. Always `USB_DESCRIPTOR_TYPE_INTERFACE` (0x04) for Interface descriptors.
    pub b_descriptor_type: u8,

    /// bInterfaceNumber - Number of this interface. Zero-based index of this interface as an argument to the Set_Configuration request.
    pub b_interface_number: u8,

    /// bAlternateSetting - Value used to select an alternate setting for this interface. Zero is used for the default setting.
    pub b_alternate_setting: u8,

    /// bNumEndpoints - Number of endpoints used by this interface (excluding endpoint zero).
    pub b_num_endpoints: u8,

    /// bInterfaceClass - Class code (assigned by the USB-IF). See USB Class Definitions.
    pub b_interface_class: u8,

    /// bInterfaceSubClass - Subclass code (assigned by the USB-IF).
    pub b_interface_sub_class: u8,

    /// bInterfaceProtocol - Protocol code (assigned by the USB-IF).
    pub b_interface_protocol: u8,

    /// iInterface - Index of string descriptor describing this interface. Zero if there is no string descriptor for this interface.
    pub i_interface: u8,
}
