use usb_dfu_target::consts::{State, DFU_PROTOCOL_RT, USB_CLASS_APPN_SPEC};

use crate::{
    descriptor::DeviceDescriptor, driver::USBHostDeviceDriver, request::Request, DeviceHandle,
};

pub struct UsbDfu {
    pub device: DeviceHandle,
    pub descriptor: DeviceDescriptor,
}

impl USBHostDeviceDriver for UsbDfu {
    async fn try_attach<D: crate::HostDriver, const NR_DEVICES: usize>(
        pipe: &crate::pipe::USBHostPipe<D, NR_DEVICES>,
        device: DeviceHandle,
        desc: DeviceDescriptor,
    ) -> Result<Self, crate::errors::UsbHostError> {
        // Check if the device is a DFU device by inspecting the descriptor
        if desc.device_class == USB_CLASS_APPN_SPEC && desc.device_sub_class == DFU_PROTOCOL_RT {
            Ok(UsbDfu {
                device,
                descriptor: desc,
            })
        } else {
            Err(crate::errors::UsbHostError::UnexpectedDevice)
        }
    }

    async fn run<D: crate::HostDriver, const NR_DEVICES: usize>(
        self,
        pipe: &crate::pipe::USBHostPipe<D, NR_DEVICES>,
    ) -> Result<(), crate::errors::UsbHostError> {
        let device_handle = self.device;
        let mut buffer = [0u8; 64];
        let request = Request {
            request_type: {
                let mut t = crate::request::RequestType::default();
                t.set_data_direction(crate::request::RequestTypeDirection::DeviceToHost);
                t.set_type(crate::request::RequestTypeType::Class);
                t.set_recipient(crate::request::RequestTypeRecipient::Interface);
                t
            },
            request: 0x05,
            value: 0,
            index: 0,
            length: 1,
        };
        pipe.control_transfer(device_handle, &request, &mut buffer)
            .await?;
        trace!("DFU device attached, state: {:?}", buffer[0]);
        Ok(())
    }
}
