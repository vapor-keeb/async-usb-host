use crate::descriptor::DescriptorType;

pub mod hub;

// Note, this does not have packed, becuase it happens to be aligned.
// so there is the static assert there.
#[repr(C)]
#[derive(Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Request {
    /// bit map of request type
    pub(crate) request_type: RequestType,
    pub(crate) request: u8,
    pub(crate) value: u16,
    pub(crate) index: u16,
    pub(crate) length: u16,
}
static_assertions::const_assert_eq!(core::mem::size_of::<Request>(), 8);

impl Request {
    pub fn set_configuration(configuration: u8) -> Request {
        Request {
            request_type: {
                let mut t = RequestType::default();
                t.set_data_direction(RequestTypeDirection::HostToDevice);
                t.set_recipient(RequestTypeRecipient::Device);
                t
            },
            request: StandardDeviceRequest::SetConfiguration as u8,
            value: configuration as u16,
            index: 0,
            length: 0,
        }
    }

    pub fn get_descriptor(
        descriptor_type: u8,
        request_type_type: RequestTypeType,
        descriptor_index: u8,
        language_id: u16,
        length: u16,
    ) -> Request {
        debug_assert!(length > 0);
        Request {
            request_type: {
                let mut t = RequestType::default();
                t.set_data_direction(RequestTypeDirection::DeviceToHost);
                t.set_type(request_type_type);
                t.set_recipient(RequestTypeRecipient::Device);
                t
            },
            request: StandardDeviceRequest::GetDescriptor as u8,
            value: ((descriptor_type as u16) << 8) | (descriptor_index as u16),
            index: language_id,
            length: length,
        }
    }

    pub fn get_status(recepient: RequestTypeRecipient, request_type_type: RequestTypeType, value: u16, index: u16, length: u16) -> Request {
        Request {
            request_type: {
                let mut t = RequestType::default();
                t.set_data_direction(RequestTypeDirection::DeviceToHost);
                t.set_type(request_type_type);
                t.set_recipient(recepient);
                t
            },
            request: StandardDeviceRequest::GetStatus as u8,
            value: value,
            index: index,
            length: length,
        }
    }

    pub fn set_feature(recepient: RequestTypeRecipient, request_type_type: RequestTypeType, feature: u16, index: u16, length: u16) -> Request {
        Request {
            request_type: {
                let mut t = RequestType::default();
                t.set_data_direction(RequestTypeDirection::HostToDevice);
                t.set_type(request_type_type);
                t.set_recipient(recepient);
                t
            },
            request: StandardDeviceRequest::SetFeature as u8,
            value: feature,
            index: index,
            length: length,
        }
    }

    pub fn clear_feature(recepient: RequestTypeRecipient, request_type_type: RequestTypeType, feature: u16, index: u16, length: u16) -> Request {
        Request {
            request_type: {
                let mut t = RequestType::default();
                t.set_data_direction(RequestTypeDirection::HostToDevice);
                t.set_type(request_type_type);
                t.set_recipient(recepient);
                t
            },
            request: StandardDeviceRequest::ClearFeature as u8,
            value: feature,
            index: index,
            length: length,
        }
    }

    pub fn get_configuration_descriptor(index: u8, length: u16) -> Request {
        Self::get_descriptor(
            DescriptorType::Configuration as u8,
            RequestTypeType::Standard,
            index,
            0,
            length,
        )
    }
}

#[repr(u8)]
#[derive(Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RequestTypeDirection {
    HostToDevice = 0,
    DeviceToHost = 1,
}

#[repr(u8)]
#[derive(Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RequestTypeType {
    Standard = 0,
    Class = 1,
    Vendor = 2,
    Rsvd = 3,
}

#[repr(u8)]
#[derive(Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RequestTypeRecipient {
    Device = 0,
    Interface = 1,
    Endpoint = 2,
    Other = 3,
}

#[repr(transparent)]
#[derive(Clone, Copy, Default)]
/**
 * D7:7 Direction (1: Device to Host)
 * D6:5 Type
 * D4:0 Recipient
 */
pub struct RequestType(u8);

impl RequestType {
    pub fn data_direction(&self) -> RequestTypeDirection {
        if (self.0 >> 7) & 0x1 == 0 {
            RequestTypeDirection::HostToDevice
        } else {
            RequestTypeDirection::DeviceToHost
        }
    }

    pub fn set_data_direction(&mut self, dir: RequestTypeDirection) {
        self.0 = (self.0 & 0x7F) | ((dir as u8 & 0x1) << 7);
    }

    // Not needed? TODO remove
    // pub fn request_type(&self) -> RequestTypeType {
    // }

    pub fn set_type(&mut self, typetype: RequestTypeType) {
        self.0 = (self.0 & !(0x3 << 5)) | ((typetype as u8) << 5);
    }

    pub fn recipient(&self) -> RequestTypeRecipient {
        match self.0 & 0x1F {
            0 => RequestTypeRecipient::Device,
            1 => RequestTypeRecipient::Interface,
            2 => RequestTypeRecipient::Endpoint,
            _ => RequestTypeRecipient::Other,
        }
    }

    pub fn set_recipient(&mut self, recipient: RequestTypeRecipient) {
        self.0 = (self.0 & 0xE0) | recipient as u8;
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for RequestType {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(
            fmt,
            "RequestType {{ Dir: {}, Type: TODO, Recipient: {} }}",
            self.data_direction(),
            // self.request_type(),
            self.recipient(),
        )
    }
}

#[repr(u8)]
pub enum StandardDeviceRequest {
    GetStatus = 0x0,
    ClearFeature = 0x1,
    SetFeature = 0x3,
    SetAddress = 0x5,
    GetDescriptor = 0x6,
    SetDescriptor = 0x7,
    GetConfiguration = 0x8,
    SetConfiguration = 0x9,
}
