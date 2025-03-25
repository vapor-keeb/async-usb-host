use crate::{descriptor::{DeviceDescriptor, ParsingError}, errors::UsbHostError, pipe::USBHostPipe, DeviceHandle, Driver};

pub struct HidKbd<'a, D: Driver, const NR_DEVICES: usize> {
    pipe: &'a USBHostPipe<D, NR_DEVICES>,
    device: DeviceHandle,
}

impl<'a, D: Driver, const NR_DEVICES: usize> HidKbd<'a, D, NR_DEVICES> {
    pub async fn try_attach(
        pipe: &'a USBHostPipe<D, NR_DEVICES>,
        device: DeviceHandle,
        desc: DeviceDescriptor
    ) -> Result<Self, UsbHostError> {
        // HID use the interface class to declare their class
        if desc.device_class != 0 {
            return Err(UsbHostError::UnexpectedDevice);
        }

        let kbd = Self {
            pipe,
            device,
        };

        kbd.configure().await?;

        Ok(kbd)
    }

    pub async fn run(self) -> Result<(), UsbHostError> {
        loop {}
    }

    async fn configure(&self) -> Result<(), UsbHostError> {
        // Pull Configuration Descriptor
        let mut buf: [u8; 255] = [0; 255];
        let len = self.pipe.control_transfer(
            self.device,
            &crate::request::Request::get_configuration_descriptor(
                0,
                core::mem::size_of::<crate::descriptor::ConfigurationDescriptor>() as u16
            ),
            &mut buf,
        )
        .await?;

        // Fetch a partial configuration descriptor first to get the total length
        let cfg = crate::descriptor::parse_descriptor(&buf[..len])
            .and_then(|desc| desc.configuration().ok_or(UsbHostError::InvalidResponse))?
            .clone();

        trace!("HID keyboard configuration received {} bytes: {:?}", len, cfg);

        // Set configuration
        self.pipe.control_transfer(
            self.device,
            &crate::request::Request::set_configuration(cfg.value),
            &mut []
        )
        .await?;
        trace!("set configuration");

        // Get full configuration descriptor with interfaces and endpoints
        let len = self.pipe.control_transfer(
            self.device,
            &crate::request::Request::get_configuration_descriptor(0, cfg.total_length),
            &mut buf,
        )
        .await?;
        trace!("get configuration descriptor (full) {} bytes", len);
        // Parse the configuration descriptor to find the interrupt endpoint
        let mut cfg_buf = &buf[..len];
        let mut endpoint_address = None;

        while !cfg_buf.is_empty() {
            let desc = match crate::descriptor::parse_descriptor(cfg_buf) {
                Ok(desc) => desc,
                Err(UsbHostError::ParsingError(ParsingError::UnknownType(descriptor_type))) => {
                    todo!()
                }
                Err(e) => return Err(e),
            };
            let desc_len = match desc {
                crate::descriptor::Descriptor::Device(_) => return Err(UsbHostError::InvalidState),
                crate::descriptor::Descriptor::Configuration(configuration_descriptor) => {
                    configuration_descriptor.length
                }
                crate::descriptor::Descriptor::Endpoint(endpoint_descriptor) => {
                    // For HID keyboard, we're looking for an IN interrupt endpoint
                    if endpoint_address.is_none() && (endpoint_descriptor.b_endpoint_address & 0x80) != 0 {
                        endpoint_address = Some(endpoint_descriptor.b_endpoint_address);
                    }
                    endpoint_descriptor.b_length
                }
                crate::descriptor::Descriptor::Interface(interface_descriptor) => {
                    // Verify this is a HID keyboard interface (class 3, subclass 1, protocol 1)
                    if interface_descriptor.b_interface_class == 0x03 &&
                       interface_descriptor.b_interface_sub_class == 0x01 &&
                       interface_descriptor.b_interface_protocol == 0x01 {
                        debug!("Found HID keyboard interface");
                    } else {
                        return Err(UsbHostError::UnexpectedDevice);
                    }
                    interface_descriptor.b_length
                }
            } as usize;

            cfg_buf = &cfg_buf[desc_len..];
        }

        if endpoint_address.is_none() {
            return Err(UsbHostError::InvalidResponse);
        }

        Ok(())
    }
}
