use bitvec::{array::BitArray, BitArr};

use crate::{
    descriptor::{
        hub::{HubDescriptor, HubPortFeature, HubPortStatus, HubPortStatusChange},
        ConfigurationDescriptor, Descriptor, DeviceDescriptor,
    },
    driver::get_configuration_descriptor,
    errors::UsbHostError,
    pipe::USBHostPipe,
    request::{Request, RequestTypeRecipient, RequestTypeType},
    types::{DataTog, DevInfo, InterruptChannel, PortInfo, UsbSpeed},
    DeviceHandle, HostDriver,
};

type PortChangeBitmask = BitArr!(for 128, in u8);

pub(crate) struct Hub {
    pub(crate) handle: DeviceHandle,
    interrupt_channel: InterruptChannel,
}

pub(crate) enum HubEvent {
    DeviceReset,
    DeviceAttach(DevInfo),
    DeviceDetach(PortInfo),
}

impl Hub {
    pub async fn new<D: HostDriver, const NR_DEVICES: usize>(
        pipe: &USBHostPipe<D, NR_DEVICES>,
        handle: DeviceHandle,
        _descriptor: DeviceDescriptor, // TODO: maybe check if this is a hub?
    ) -> Result<Self, UsbHostError> {
        // Pull uConfiguraiton Descriptor
        let mut buf: [u8; 255] = [0; 255];
        let desc_iter = get_configuration_descriptor(handle, &mut buf, pipe).await?;

        let mut endpoint_address = None;
        for desc in desc_iter {
            match desc? {
                Descriptor::Configuration(cfg) => {
                    pipe.control_transfer(handle, &Request::set_configuration(cfg.value), &mut [])
                        .await?;
                    debug!("found hub configuration: {:?}", cfg);
                }
                Descriptor::Endpoint(endpoint_descriptor) => {
                    assert!(endpoint_address.is_none()); // TODO: this happens on the Anker hub
                    endpoint_address = Some(endpoint_descriptor.into());
                }
                _ => continue, // skip other descriptors
            }
        }
        // set config

        let mut hub_desc = HubDescriptor::default();
        let hub_desc_buf = unsafe {
            core::slice::from_raw_parts_mut(
                &raw mut hub_desc as *mut u8,
                core::mem::size_of::<HubDescriptor>(),
            )
        };
        pipe.control_transfer(
            handle,
            &Request::get_descriptor(
                0x29, // Hub Descriptor
                RequestTypeType::Class,
                0,
                0,
                hub_desc_buf.len() as u16,
            ),
            hub_desc_buf,
        )
        .await?;

        debug!("hub descriptor: {:?}", hub_desc);

        // enable ports
        for port in 1..=hub_desc.number_of_ports {
            pipe.control_transfer(
                handle,
                &Request::set_feature(
                    RequestTypeRecipient::Other,
                    RequestTypeType::Class,
                    8, // Port Power
                    port as u16,
                    0,
                ),
                &mut [],
            )
            .await?;
        }

        for port in 1..=hub_desc.number_of_ports {
            let mut port_status = [0u8; 4];
            pipe.control_transfer(
                handle,
                &Request::get_status(
                    RequestTypeRecipient::Other,
                    RequestTypeType::Class,
                    0,
                    port as u16,
                    port_status.len() as u16,
                ),
                &mut port_status,
            )
            .await?;
        }

        let endpoint_address = endpoint_address.ok_or(UsbHostError::InvalidResponse)?;

        let mut hub = Hub {
            handle,
            interrupt_channel: InterruptChannel {
                device_handle: handle,
                endpoint_address,
                tog: DataTog::DATA0,
            },
        };

        // Port number are 1 based
        // Poll port status
        for port in 1..=hub_desc.number_of_ports {
            if let Ok((status, _)) = hub.get_port_status(pipe, port).await {
                trace!("port {} status: {:?}", port, status);

                // Power it on if it is not already
                if !status.power() {
                    if let Err(e) = hub
                        .set_port_feature(pipe, port, HubPortFeature::Power)
                        .await
                    {
                        error!("failed to enable port {}: {:?}", port, e);
                    }
                }
            }
        }

        Ok(hub)
    }

    async fn clear_port_feature<D: HostDriver, const NR_DEVICES: usize>(
        &mut self,
        pipe: &USBHostPipe<D, NR_DEVICES>,
        port: u8,
        feature: HubPortFeature,
    ) -> Result<(), UsbHostError> {
        pipe.control_transfer(
            self.handle,
            &Request::clear_feature(
                RequestTypeRecipient::Other,
                RequestTypeType::Class,
                feature as u16,
                port as u16,
                0,
            ),
            &mut [],
        )
        .await
        .map(|_| ())
    }

    async fn set_port_feature<D: HostDriver, const NR_DEVICES: usize>(
        &mut self,
        pipe: &USBHostPipe<D, NR_DEVICES>,
        port: u8,
        feature: HubPortFeature,
    ) -> Result<(), UsbHostError> {
        pipe.control_transfer(
            self.handle,
            &Request::set_feature(
                RequestTypeRecipient::Other,
                RequestTypeType::Class,
                feature as u16,
                port as u16,
                0,
            ),
            &mut [],
        )
        .await
        .map(|_| ())
    }

    async fn get_port_status<D: HostDriver, const NR_DEVICES: usize>(
        &mut self,
        pipe: &USBHostPipe<D, NR_DEVICES>,
        port: u8,
    ) -> Result<(HubPortStatus, HubPortStatusChange), UsbHostError> {
        // TODO: handle status change bits
        let mut status_buf = [0u8; 4];
        match pipe
            .control_transfer(
                self.handle,
                &Request::get_status(
                    RequestTypeRecipient::Other,
                    RequestTypeType::Class,
                    0,
                    port as u16,
                    status_buf.len() as u16,
                ),
                &mut status_buf,
            )
            .await
        {
            Ok(len) => {
                assert_eq!(len, 4);
                Ok((
                    u16::from_le_bytes([status_buf[0], status_buf[1]]).into(),
                    u16::from_le_bytes([status_buf[2], status_buf[3]]).into(),
                ))
            }
            Err(UsbHostError::BufferOverflow) => panic!("buffer overflow"),
            Err(e) => Err(e),
        }
    }

    async fn on_status_change<D: HostDriver, const NR_DEVICES: usize>(
        &mut self,
        pipe: &USBHostPipe<D, NR_DEVICES>,
        bitmask: &PortChangeBitmask,
        enumeration_in_progress: bool,
    ) -> Result<Option<HubEvent>, UsbHostError> {
        // Poll port status
        for port in bitmask.iter_ones() {
            if port == 0 {
                continue; // 0 is hub
            }
            if let Ok((status, change)) = self.get_port_status(pipe, port as u8).await {
                debug!("port {} status: {:?}\n change: {:?}", port, status, change);

                if change.connection() {
                    if status.connected() {
                        if !enumeration_in_progress {
                            self.clear_port_feature(
                                pipe,
                                port as u8,
                                HubPortFeature::ChangeConnection,
                            )
                            .await?;
                            trace!("Resetting port {} on hub {}", port, self.handle.address());
                            unwrap!(
                                self.set_port_feature(pipe, port as u8, HubPortFeature::Reset)
                                    .await
                            );
                            return Ok(Some(HubEvent::DeviceReset));
                        }
                        // Enumeration in progress, wait for current enumeration to finish
                    } else {
                        self.clear_port_feature(pipe, port as u8, HubPortFeature::ChangeConnection)
                            .await?;
                        return Ok(Some(HubEvent::DeviceDetach(PortInfo::new(
                            self.handle.address(),
                            port as u8,
                        ))));
                    }
                }

                if change.reset() {
                    if !enumeration_in_progress {
                        error!(
                            "port {} reset changed, but currently not in enumeration",
                            port
                        );
                    }
                    unwrap!(
                        self.clear_port_feature(pipe, port as u8, HubPortFeature::ChangeReset)
                            .await
                    );
                    if !status.reset() {
                        let tt = match (self.handle.dev_info().speed(), status.speed()) {
                            (UsbSpeed::HighSpeed, UsbSpeed::FullSpeed | UsbSpeed::LowSpeed) => {
                                // Hub is the TT for this device
                                Some((self.handle.address(), port as u8))
                            }
                            (_, _) => {
                                // device has the same TT as the hub.
                                self.handle.dev_info().transaction_translator()
                            }
                        };
                        return Ok(Some(HubEvent::DeviceAttach(DevInfo::new(
                            self.handle.address(),
                            port as u8,
                            tt,
                            status.speed(),
                        ))));
                    } else {
                        error!("port {} reset changed but set to true", port);
                    }
                }
            }
        }
        Ok(None)
    }

    // Main deal
    pub async fn poll<D: HostDriver, const NR_DEVICES: usize>(
        &mut self,
        pipe: &USBHostPipe<D, NR_DEVICES>,
        enumeration_in_progress: bool,
    ) -> Result<Option<HubEvent>, UsbHostError> {
        // interrupt transfer with pipe
        let mut in_buf: PortChangeBitmask = BitArray::ZERO;
        let in_buf_len = pipe
            .interrupt_transfer(&mut self.interrupt_channel, in_buf.as_raw_mut_slice())
            .await;
        match in_buf_len {
            Ok(len) => {
                assert!(len > 0);
                self.on_status_change(pipe, &in_buf, enumeration_in_progress)
                    .await
            }
            Err(UsbHostError::NAK) => Ok(None),
            Err(e) => {
                error!("interrupt transfer error: {:?}", e);
                Err(e)
            }
        }
    }
}
