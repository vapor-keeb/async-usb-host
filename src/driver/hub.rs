use crate::{
    descriptor::{
        hub::HubDescriptor, parse_descriptor, ConfigurationDescriptor, Descriptor, DeviceDescriptor,
    },
    errors::UsbHostError,
    pipe::USBHostPipe,
    request::{Request, RequestTypeRecipient, RequestTypeType},
    types::DataTog,
    DeviceHandle, Driver, Host,
};

pub(crate) async fn register_hub<D: Driver>(
    pipe: &USBHostPipe<D>,
    handle: DeviceHandle,
    descriptor: DeviceDescriptor,
) -> Result<(), UsbHostError> {
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
        pipe
            .control_transfer(
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
        pipe
            .control_transfer(
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
        pipe
            .control_transfer(
                handle,
                &Request::get_configuration_descriptor(0, cfg.total_length),
                &mut buf,
            )
            .await
    );
    // TODO should probably iterate through the descriptor
    let mut cfg_buf = &buf[..len];
    while !cfg_buf.is_empty() {
        let desc = parse_descriptor(cfg_buf)?;
        let len = match desc {
            Descriptor::Device(device_descriptor) => device_descriptor.length,
            Descriptor::Configuration(configuration_descriptor) => configuration_descriptor.length,
            Descriptor::Endpoint(endpoint_descriptor) => {
                // interview quality code
                // TODO needs to go into a separate polling task
                // for each port
                // GET_STATUS
                // SET_FEATURE

                // This is just an interrupt transfer
                /*
                loop {
                    let mut in_buf: [u8; 64] = [0; 64];
                    let in_buf_len =
                        pipe
                        .data_in(
                            endpoint_descriptor.b_endpoint_address,
                            DataTog::DATA0,
                            &mut in_buf,
                        )
                        .await;
                    if let Ok(in_buf_len) = in_buf_len {
                        trace!("{}", in_buf[..in_buf_len]);
                        break;
                    }
                } */

                endpoint_descriptor.b_length
            }
            Descriptor::Interface(interface_descriptor) => interface_descriptor.b_length,
        } as usize;

        cfg_buf = &cfg_buf[len..];
    }

    Ok(())
}
