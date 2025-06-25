use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{self, Channel, Receiver, Sender},
    pipe::Writer,
};
use usb_dfu_target::consts::{DfuRequest, State, DFU_PROTOCOL_RT, USB_CLASS_APPN_SPEC};

use crate::{
    descriptor::{Descriptor, DeviceDescriptor},
    driver::USBHostDeviceDriver,
    errors::UsbHostError,
    request::Request,
    types::Bcd16,
    DeviceHandle,
};

use super::{get_configuration_descriptor, DeviceChannel};

pub enum DFUOperation {
    StartDownload,
    Bytes([u8; 8]),
    Manifest,
    Detach,
}

#[derive(Default, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
struct DFUInfo {
    capabilities: DFUCapabilities,

    /// detach timeout in ms
    detach_timeout: u16,

    /// transfer size in bytes
    transfer_size: u16,

    /// dfu version
    version: Bcd16,
}

#[derive(Default, Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
struct DFUCapabilities(u8);

impl DFUCapabilities {
    pub fn can_download(&self) -> bool {
        self.0 & 0b00000001 != 0
    }
    pub fn can_upload(&self) -> bool {
        self.0 & 0b00000010 != 0
    }
    pub fn can_manifest(&self) -> bool {
        self.0 & 0b00000100 != 0
    }
    pub fn can_detach(&self) -> bool {
        self.0 & 0b00001000 != 0
    }
}

static DFU_CHANNEL: Channel<CriticalSectionRawMutex, DFUOperation, 1> = Channel::new();

pub struct UsbDfu {
    pub device: DeviceHandle,
    info: DFUInfo,
}

impl UsbDfu {
    pub fn channel_sender() -> Sender<'static, CriticalSectionRawMutex, DFUOperation, 1> {
        DFU_CHANNEL.sender()
    }

    fn channel_receiver(&self) -> Receiver<'static, CriticalSectionRawMutex, DFUOperation, 1> {
        DFU_CHANNEL.receiver()
    }
}

impl USBHostDeviceDriver for UsbDfu {
    async fn try_attach<D: crate::HostDriver, const NR_DEVICES: usize>(
        pipe: &crate::pipe::USBHostPipe<D, NR_DEVICES>,
        device: DeviceHandle,
        _desc: DeviceDescriptor,
    ) -> Result<Self, crate::errors::UsbHostError> {
        let mut buf: [u8; 255] = [0; 255];
        let desc_iter = get_configuration_descriptor(device, &mut buf, pipe).await?;

        let mut is_dfu = false;
        let mut dfu_info: Option<DFUInfo> = None;

        for desc in desc_iter {
            match desc? {
                Descriptor::Configuration(configuration_descriptor) => {
                    trace!(
                        "found configuration descriptor: {:?}",
                        configuration_descriptor
                    );
                }
                Descriptor::Endpoint(endpoint_descriptor) => {
                    trace!("found endpoint descriptor: {:?}", endpoint_descriptor);
                }
                Descriptor::Interface(interface_descriptor) => {
                    if interface_descriptor.b_interface_class == USB_CLASS_APPN_SPEC
                        && interface_descriptor.b_interface_sub_class == DFU_PROTOCOL_RT
                    {
                        trace!("found DFU interface descriptor: {:?}", interface_descriptor);
                        is_dfu = true;
                    }
                }
                Descriptor::UnknownDescriptor {
                    descriptor_type,
                    length,
                    data,
                } => match descriptor_type {
                    0x21 => {
                        // DFU Functional Descriptor
                        if length >= 9 {
                            let mut info = DFUInfo::default();
                            info.capabilities = DFUCapabilities(data[2]);
                            info.detach_timeout = u16::from_le_bytes([data[3], data[4]]);
                            info.transfer_size = u16::from_le_bytes([data[5], data[6]]);
                            info.version = Bcd16::from_le_bytes([data[7], data[8]]);
                            trace!("DFU info: {:?}", info);
                            dfu_info = Some(info);
                        }
                    }
                    _ => {
                        trace!("found unknown descriptor: {:?}", descriptor_type);
                    }
                },
                _ => panic!("unexpected descriptor: {:?}", desc),
            }
        }

        if is_dfu {
            Ok(UsbDfu {
                device,
                info: dfu_info.ok_or(UsbHostError::InvalidState)?,
            })
        } else {
            Err(UsbHostError::UnexpectedDevice)
        }
    }

    async fn run<D: crate::HostDriver, const NR_DEVICES: usize>(
        self,
        pipe: &crate::pipe::USBHostPipe<D, NR_DEVICES>,
    ) -> Result<(), crate::errors::UsbHostError> {
        let device_handle = self.device;
        let mut buffer = [0u8; 64];
        let dfu_get_state = Request {
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
        pipe.control_transfer(device_handle, &dfu_get_state, &mut buffer)
            .await?;
        trace!("DFU device attached, state: {:?}", buffer[0]);
        let channel_receiver = self.channel_receiver();
        let mut dfu_block_counter = 0u16;
        loop {
            let dfu_op = channel_receiver.receive().await;
            match dfu_op {
                DFUOperation::StartDownload => {
                    trace!("Starting download");
                    dfu_block_counter = 0;
                }
                DFUOperation::Bytes(mut bytes) => {
                    for _ in 0..3 {
                        let dfu_download = Request {
                            request_type: {
                                let mut t = crate::request::RequestType::default();
                                t.set_type(crate::request::RequestTypeType::Class);
                                t.set_recipient(crate::request::RequestTypeRecipient::Interface);
                                t.set_data_direction(
                                    crate::request::RequestTypeDirection::HostToDevice,
                                );
                                t
                            },
                            request: DfuRequest::Dnload as u8,
                            value: dfu_block_counter,
                            index: 0,
                            length: 8,
                        };
                        trace!("sending ctrl transfer to do DFU");
                        match pipe
                            .control_transfer(device_handle, &dfu_download, &mut bytes)
                            .await
                        {
                            Ok(_) => {
                                info!("Downloaded block {:?}", dfu_block_counter);
                                dfu_block_counter = dfu_block_counter.wrapping_add(1);
                                break;
                            }
                            Err(e) => {
                                error!("Error downloading bytes: {:?}", e);
                            }
                        }
                    }
                }
                DFUOperation::Manifest => {
                    trace!("Received manifest");
                }
                DFUOperation::Detach => {
                    trace!("Detaching");
                    return Ok(());
                }
            }
        }
    }
}
