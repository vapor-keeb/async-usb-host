use crate::{
    descriptor::{DeviceDescriptor, ParsingError},
    errors::UsbHostError,
    pipe::USBHostPipe,
    types::{DataTog, EndpointAddress, EndpointDirection, InterruptChannel},
    DeviceHandle, HostDriver,
};

use super::USBHostDeviceDriver;

pub struct HidKbd {
    device: DeviceHandle,
    interrupt_channel: Option<InterruptChannel>,
}

impl HidKbd {
    fn process_keyboard_report(report: &[u8]) {
        // Standard HID keyboard report format:
        // Byte 0: Modifier keys (CTRL, SHIFT, ALT, etc.)
        // Byte 1: Reserved
        // Bytes 2-7: Up to 6 simultaneous key presses

        let modifiers = report[0];
        let keys = &report[2..8];

        debug!(
            "Keyboard report - modifiers: {:02x}, keys: {:02x}",
            modifiers, keys
        );

        // Process modifier keys
        if modifiers != 0 {
            if modifiers & 0x01 != 0 {
                debug!("Left CTRL pressed");
            }
            if modifiers & 0x02 != 0 {
                debug!("Left SHIFT pressed");
            }
            if modifiers & 0x04 != 0 {
                debug!("Left ALT pressed");
            }
            if modifiers & 0x08 != 0 {
                debug!("Left GUI pressed");
            }
            if modifiers & 0x10 != 0 {
                debug!("Right CTRL pressed");
            }
            if modifiers & 0x20 != 0 {
                debug!("Right SHIFT pressed");
            }
            if modifiers & 0x40 != 0 {
                debug!("Right ALT pressed");
            }
            if modifiers & 0x80 != 0 {
                debug!("Right GUI pressed");
            }
        }

        // Process pressed keys
        for key in keys {
            if *key != 0 {
                debug!(
                    "Key pressed: {:02x} ({})",
                    key,
                    Self::hid_key_to_ascii(*key)
                );
            }
        }
    }

    fn hid_key_to_ascii(key: u8) -> char {
        // Simple mapping of common HID keyboard codes to ASCII
        // This is a basic implementation - a full implementation would handle
        // all keys and consider modifiers
        match key {
            0x04..=0x1D => (key - 0x04 + b'a') as char, // a-z
            0x1E..=0x27 => (key - 0x1E + b'1') as char, // 1-9
            0x28 => '\n',                               // Enter
            0x29 => '\x1B',                             // Escape
            0x2A => '\x08',                             // Backspace
            0x2B => '\t',                               // Tab
            0x2C => ' ',                                // Space
            0x2D => '-',                                // - and _
            0x2E => '=',                                // = and +
            0x2F => '[',                                // [ and {
            0x30 => ']',                                // ] and }
            0x31 => '\\',                               // \ and |
            0x33 => ';',                                // ; and :
            0x34 => '\'',                               // ' and "
            0x35 => '`',                                // ` and ~
            0x36 => ',',                                // , and <
            0x37 => '.',                                // . and >
            0x38 => '/',                                // / and ?
            _ => '?',                                   // Unknown key
        }
    }

    async fn configure<D: HostDriver, const NR_DEVICES: usize>(&mut self, pipe: &USBHostPipe<D, NR_DEVICES>) -> Result<(), UsbHostError> {
        // Pull Configuration Descriptor
        let mut buf: [u8; 255] = [0; 255];
        let len = pipe
            .control_transfer(
                self.device,
                &crate::request::Request::get_configuration_descriptor(
                    0,
                    core::mem::size_of::<crate::descriptor::ConfigurationDescriptor>() as u16,
                ),
                &mut buf,
            )
            .await?;

        // Fetch a partial configuration descriptor first to get the total length
        let cfg = crate::descriptor::parse_descriptor(&buf[..len])
            .and_then(|desc| desc.configuration().ok_or(UsbHostError::InvalidResponse))?
            .clone();

        trace!(
            "HID keyboard configuration received {} bytes: {:?}",
            len,
            cfg
        );

        // Set configuration
        pipe
            .control_transfer(
                self.device,
                &crate::request::Request::set_configuration(cfg.value),
                &mut [],
            )
            .await?;
        trace!("set configuration");

        // Get full configuration descriptor with interfaces and endpoints
        let len = pipe
            .control_transfer(
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
                    // Check if this is a HID descriptor
                    if descriptor_type == crate::descriptor::hid::HID_DESCRIPTOR_TYPE {
                        if let Some(hid_desc) =
                            crate::descriptor::hid::HIDDescriptor::parse(cfg_buf)
                        {
                            trace!("Found HID descriptor: {:?}", hid_desc);
                            // Skip this descriptor and continue parsing
                            cfg_buf = &cfg_buf[hid_desc.total_length as usize..];
                            continue;
                        }
                    }

                    // If we can't parse it as a HID descriptor, skip this descriptor
                    // Assuming the first byte contains the descriptor length
                    if !cfg_buf.is_empty() {
                        let desc_len = cfg_buf[0] as usize;
                        if desc_len > 0 && desc_len <= cfg_buf.len() {
                            debug!("Skipping unknown descriptor of length {} and type {}", desc_len, descriptor_type);
                            cfg_buf = &cfg_buf[desc_len..];
                            continue;
                        }
                    }

                    return Err(UsbHostError::ParsingError(ParsingError::UnknownType(
                        descriptor_type,
                    )));
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
                    if endpoint_address.is_none()
                        && (endpoint_descriptor.b_endpoint_address & 0x80) != 0
                    {
                        endpoint_address = Some(endpoint_descriptor.b_endpoint_address);
                    }
                    endpoint_descriptor.b_length
                }
                crate::descriptor::Descriptor::Interface(interface_descriptor) => {
                    debug!("Found interface: {:?}", interface_descriptor);
                    // Verify this is a HID keyboard interface (class 3, subclass 1, protocol 1)
                    if interface_descriptor.b_interface_class == 0x03
                        && interface_descriptor.b_interface_sub_class == 0x01
                        && interface_descriptor.b_interface_protocol == 0x01
                    {
                        debug!("Found HID keyboard interface");
                    } else {
                        debug!("Found non-HID keyboard interface");
                    }
                    interface_descriptor.b_length
                }
            } as usize;

            cfg_buf = &cfg_buf[desc_len..];
        }

        if let Some(addr) = endpoint_address {
            // Create an InterruptChannel instead of just storing the endpoint address
            let endpoint = EndpointAddress {
                number: addr & 0x0F,
                direction: if (addr & 0x80) != 0 {
                    EndpointDirection::In
                } else {
                    EndpointDirection::Out
                },
            };

            self.interrupt_channel = Some(InterruptChannel {
                device_handle: self.device,
                endpoint_address: endpoint,
                tog: DataTog::DATA0,
            });

            debug!("Using keyboard endpoint: {:?}", endpoint);
            Ok(())
        } else {
            Err(UsbHostError::InvalidResponse)
        }
    }
}

impl USBHostDeviceDriver for HidKbd {
    async fn try_attach<D: HostDriver, const NR_DEVICES: usize>(
        pipe: &USBHostPipe<D, NR_DEVICES>,
        device: DeviceHandle,
        desc: DeviceDescriptor,
    ) -> Result<Self, UsbHostError> {
        // HID use the interface class to declare their class
        if desc.device_class != 0 {
            return Err(UsbHostError::UnexpectedDevice);
        }

        let mut kbd = Self {
            device,
            interrupt_channel: None,
        };

        kbd.configure(pipe).await?;

        Ok(kbd)
    }

    async fn run<'a, D: HostDriver, const NR_DEVICES: usize>(
        self,
        pipe: &'a USBHostPipe<D, NR_DEVICES>
    ) -> Result<(), UsbHostError> {
        let mut prev_report = [0u8; 8];
        let mut buf = [0u8; 8]; // Standard HID keyboard report is 8 bytes

        let Self {
            device: _, // Mark device as unused for now
            interrupt_channel,
        } = self;

        // Ensure we have an interrupt channel configured
        let mut interrupt_channel = interrupt_channel.ok_or(UsbHostError::InvalidState)?;

        loop {
            // Poll the interrupt endpoint for keyboard reports
            match pipe
                .interrupt_transfer(&mut interrupt_channel, &mut buf)
                .await
            {
                Ok(len) => {
                    if len > 0 && buf != prev_report {
                        // Process the keyboard report
                        Self::process_keyboard_report(&buf);
                        prev_report.copy_from_slice(&buf);
                    }
                }
                Err(UsbHostError::NAK) => {
                    // NAK are normal for interrupt endpoints, just continue
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

}
