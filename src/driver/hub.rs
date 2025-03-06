use bitvec::{array::BitArray, bitarr, order::Msb0, BitArr};

use crate::{
    descriptor::{
        hub::{HubDescriptor, HubPortFeature, HubPortStatus, HubPortStatusChange},
        parse_descriptor, ConfigurationDescriptor, Descriptor, DeviceDescriptor,
    },
    errors::UsbHostError,
    pipe::USBHostPipe,
    request::{Request, RequestTypeRecipient, RequestTypeType},
    types::{DataTog, EndpointAddress, InterruptChannel},
    DeviceHandle, Driver, Host,
};

pub(crate) struct Hub {
    handle: DeviceHandle,
    nr_ports: u8,
    ports_connected: BitArr!(for 128),
    interrupt_channel: InterruptChannel,
}

impl Hub {
    pub async fn new<D: Driver>(
        pipe: &USBHostPipe<D>,
        handle: DeviceHandle,
        descriptor: DeviceDescriptor,
    ) -> Result<Self, UsbHostError> {
        // Pull Configuraiton Descriptor
        let mut buf: [u8; 255] = [0; 255];
        let len = unwrap!(
            pipe.control_transfer(
                handle,
                &Request::get_configuration_descriptor(
                    0,
                    core::mem::size_of::<ConfigurationDescriptor>() as u16
                ),
                &mut buf,
            )
            .await
        );
        let cfg = parse_descriptor(&buf[..len])
            .and_then(|desc| desc.configuration().ok_or(UsbHostError::InvalidResponse))?
            .clone();
        trace!("configuration recv {} bytes: {:?}", len, cfg);
        // set config
        pipe.control_transfer(handle, &Request::set_configuration(cfg.value), &mut [])
            .await?;

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
            debug!("port status {}: {:?}", port, port_status);
        }

        // get configuration descriptor again with the proper len
        let len = unwrap!(
            pipe.control_transfer(
                handle,
                &Request::get_configuration_descriptor(0, cfg.total_length),
                &mut buf,
            )
            .await
        );

        // TODO should probably iterate through the descriptor
        let mut cfg_buf = &buf[..len];
        let mut endpoint_address = None;
        while !cfg_buf.is_empty() {
            let desc = parse_descriptor(cfg_buf)?;
            let len = match desc {
                Descriptor::Device(device_descriptor) => device_descriptor.length,
                Descriptor::Configuration(configuration_descriptor) => {
                    configuration_descriptor.length
                }
                Descriptor::Endpoint(endpoint_descriptor) => {
                    assert!(endpoint_address.is_none());
                    endpoint_address = Some(endpoint_descriptor.into());
                    endpoint_descriptor.b_length
                }
                Descriptor::Interface(interface_descriptor) => interface_descriptor.b_length,
            } as usize;

            cfg_buf = &cfg_buf[len..];
        }

        let endpoint_address = endpoint_address.ok_or(UsbHostError::InvalidResponse)?;

        let mut hub = Hub {
            handle,
            nr_ports: hub_desc.number_of_ports,
            ports_connected: BitArray::ZERO,
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
                debug!("port {} status: {:?}", port, status);

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

    async fn clear_port_feature<D: Driver>(
        &mut self,
        pipe: &USBHostPipe<D>,
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

    async fn set_port_feature<D: Driver>(
        &mut self,
        pipe: &USBHostPipe<D>,
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

    async fn get_port_status<D: Driver>(
        &mut self,
        pipe: &USBHostPipe<D>,
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
                Ok((u16::from_le_bytes([status_buf[0], status_buf[1]]).into(),
                    u16::from_le_bytes([status_buf[2], status_buf[3]]).into()))
            }
            Err(UsbHostError::BufferOverflow) => panic!("buffer overflow"),
            Err(e) => Err(e),
        }
    }

    async fn on_status_change(&mut self, pipe: &USBHostPipe<impl Driver>, bitmask: &[u8]) {
        // TODO: left off here
        // Poll port status
        for port in 1..=self.nr_ports {
            if let Ok((status, change)) = self.get_port_status(pipe, port).await {
                debug!("port {} status: {:?}\n change: {:?}", port, status, change);
                if change.connection() {
                    unwrap!(self.clear_port_feature(pipe, port, HubPortFeature::ChangeConnection)
                        .await);
                }
            }
        }
    }

    // Main deal
    pub async fn poll<D: Driver>(&mut self, pipe: &USBHostPipe<D>) {
        // interrupt transfer with pipe
        let mut in_buf: [u8; 64] = [0; 64];
        let in_buf_len = pipe
            .interrupt_transfer(&mut self.interrupt_channel, &mut in_buf)
            .await;
        match in_buf_len {
            Ok(in_buf_len) => {
                trace!("{}", in_buf[..in_buf_len]);
                self.on_status_change(pipe, &in_buf[..in_buf_len]).await;
            }
            Err(UsbHostError::NAK) => return,
            Err(e) => {
                error!("interrupt transfer error: {:?}", e);
            }
        }
    }
}
