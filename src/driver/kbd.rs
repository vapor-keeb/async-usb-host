use embassy_time::Timer;

use crate::{
    descriptor::{DeviceDescriptor, ParsingError},
    driver::get_configuration_descriptor,
    errors::UsbHostError,
    pipe::USBHostPipe,
    request::{Request, RequestType, RequestTypeDirection, RequestTypeRecipient, RequestTypeType},
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

    async fn configure<D: HostDriver, const NR_DEVICES: usize>(
        &mut self,
        pipe: &USBHostPipe<D, NR_DEVICES>,
    ) -> Result<(), UsbHostError> {
        // Pull Configuration Descriptor
        let mut buf: [u8; 255] = [0; 255];

        let config_iter = get_configuration_descriptor(self.device, &mut buf, pipe).await?;
        let mut endpoint_address = None;

        for desc in config_iter {
            match desc? {
                crate::descriptor::Descriptor::Device(_device_descriptor) => todo!(),
                crate::descriptor::Descriptor::Configuration(configuration_descriptor) => {
                    // Set configuration
                    pipe.control_transfer(
                        self.device,
                        &crate::request::Request::set_configuration(configuration_descriptor.value),
                        &mut [],
                    )
                    .await?;
                    trace!("set configuration");
                }
                crate::descriptor::Descriptor::Endpoint(endpoint_descriptor) => {
                    // TODO: handle multiple endpoints
                    // For HID keyboard, we're looking for an IN interrupt endpoint
                    if endpoint_address.is_none()
                        && (endpoint_descriptor.b_endpoint_address & 0x80) != 0
                    {
                        endpoint_address = Some(endpoint_descriptor.b_endpoint_address);
                    }
                }
                crate::descriptor::Descriptor::Interface(interface_descriptor) => {
                    // Verify this is a HID keyboard interface (class 3, subclass 1, protocol 1)
                    if interface_descriptor.b_interface_class == 0x03
                        && interface_descriptor.b_interface_sub_class == 0x01
                        && interface_descriptor.b_interface_protocol == 0x01
                    {
                        debug!("Found HID keyboard interface");
                    } else {
                        debug!("Found non-HID keyboard interface");
                    }
                }
                crate::descriptor::Descriptor::UnknownDescriptor {
                    descriptor_type,
                    length: _,
                    data,
                } => {
                    if descriptor_type == crate::descriptor::hid::HID_DESCRIPTOR_TYPE {
                        if let Some(hid_desc) = crate::descriptor::hid::HIDDescriptor::parse(data) {
                            trace!("Found HID descriptor: {:?}", hid_desc);
                            continue;
                        }
                    }
                }
            }
        }

        // Send SET_IDLE request to disable automatic repeat
        let set_idle_request = Request {
            request_type: {
                let mut r = RequestType::default();
                r.set_data_direction(RequestTypeDirection::HostToDevice);
                r.set_type(RequestTypeType::Class);
                r.set_recipient(RequestTypeRecipient::Interface);
                r
            },
            request: 0x0A, // SET_IDLE
            value: 0,      // 0 = disable idle
            index: 0,      // interface number
            length: 0,
        };

        pipe.control_transfer(self.device, &set_idle_request, &mut [])
            .await?;
        debug!("SET_IDLE request sent successfully");

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
        pipe: &'a USBHostPipe<D, NR_DEVICES>,
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
            Timer::after_millis(10).await;
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
