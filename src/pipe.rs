use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::Timer;

use crate::{
    descriptor::{parse_descriptor, DeviceDescriptor},
    device_addr::DeviceDisconnectMask,
    errors::UsbHostError,
    request::{self, Request, StandardDeviceRequest},
    types::{self, DataTog, DevInfo, EndpointType, InterruptChannel, PortInfo, UsbSpeed},
    DeviceAddressManager, DeviceHandle, HostDriver, TRANSFER_TIMEOUT,
};

// not Send anyways
#[allow(async_fn_in_trait)]
pub trait Pipe {
    fn set_addr(&mut self, addr: u8);
    /// When setup is called, it should send a setup request, also setup the
    /// hardware to send / expect DATA1 packets on subsequent data_in / data_out
    async fn setup(&mut self, buf: Option<&[u8; 8]>) -> Result<(), UsbHostError>;

    // TODO: fix ep_type to a proper type
    // msb: lsb
    // 00 control
    // 01 isochronous
    // 10 bulk
    // 11 interrupt
    async fn split(
        &mut self,
        complete: bool,
        port: u8,
        ep_type: EndpointType,
        speed: UsbSpeed,
    ) -> Result<(), UsbHostError>;

    async fn data_in(
        &mut self,
        endpoint: u8,
        tog: DataTog,
        wait_for_reply: bool,
        send_ack: bool,
        buf: &mut [u8],
    ) -> Result<usize, UsbHostError>;
    async fn data_out(
        &mut self,
        endpoint: u8,
        tog: DataTog,
        buf: Option<&[u8]>,
    ) -> Result<(), UsbHostError>;
}

struct USBHostPipeInner<D: HostDriver, const NR_DEVICES: usize> {
    pipe: D::Pipe,
    address_alloc: DeviceAddressManager<NR_DEVICES>,
}

/// wrapper around the underlying pipe implementation with support for split transactions
impl<D: HostDriver, const NR_DEVICES: usize> USBHostPipeInner<D, NR_DEVICES> {
    async fn split_setup(
        &mut self,
        tt_addr: u8,
        tt_port: u8,
        address: u8,
        speed: UsbSpeed,
        req: &Request,
    ) -> Result<(), UsbHostError> {
        loop {
            self.pipe.set_addr(tt_addr);
            self.pipe
                .split(false, tt_port, EndpointType::Control, speed)
                .await?;

            self.pipe.set_addr(address);
            let setup_fut = self.pipe.setup(Some(unsafe {
                core::mem::transmute::<&Request, &[u8; 8]>(req)
            }));
            match setup_fut.await {
                Ok(()) => break,
                Err(UsbHostError::NAK) => {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        loop {
            self.pipe.set_addr(tt_addr);
            self.pipe
                .split(true, tt_port, EndpointType::Control, speed)
                .await?;
            self.pipe.set_addr(address);
            let setup_fut = self.pipe.setup(None);
            match setup_fut.await {
                Ok(()) => return Ok(()),
                Err(UsbHostError::NYET) => {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }

    async fn setup(
        &mut self,
        dev_info: &DevInfo,
        address: u8,
        req: &Request,
    ) -> Result<(), UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        #[cfg(not(target_endian = "little"))]
        compile_error!("Only little endian supported");
        if let Some((tt_addr, tt_port)) = dev_info.transaction_translator() {
            return self
                .split_setup(tt_addr, tt_port, address, dev_info.speed(), req)
                .await;
        }
        self.pipe.set_addr(address);
        let setup_fut = self.pipe.setup(Some(unsafe {
            core::mem::transmute::<&Request, &[u8; 8]>(req)
        }));
        match select(timeout_fut, setup_fut).await {
            Either::First(_) => Err(UsbHostError::TransferTimeout),
            Either::Second(r) => r,
        }
    }

    async fn data_in_with_retry(
        &mut self,
        dev_info: &DevInfo,
        address: u8,
        endpoint: u8,
        endpoint_type: EndpointType,
        tog: DataTog,
        buf: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        loop {
            match self
                .data_in(dev_info, address, endpoint, endpoint_type, tog, buf)
                .await
            {
                Ok(size) => return Ok(size),
                Err(UsbHostError::NAK) => {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }

    async fn split_data_in(
        &mut self,
        tt_addr: u8,
        tt_port: u8,
        address: u8,
        endpoint: u8,
        endpoint_type: EndpointType,
        speed: UsbSpeed,
        tog: DataTog,
        buf: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        let wait_for_reply = match endpoint_type {
            EndpointType::Control => true,
            EndpointType::Interrupt => false,
            _ => todo!(),
        };

        for _ in 0..3 {
            loop {
                self.pipe.set_addr(tt_addr);
                // TODO: this is a huge problem, fix
                self.pipe
                    .split(false, tt_port, endpoint_type, speed)
                    .await?;
                self.pipe.set_addr(address);
                let in_fut = self.pipe.data_in(endpoint, tog, wait_for_reply, true, &mut []);

                match in_fut.await {
                    Ok(_) => {
                        break;
                    }
                    Err(UsbHostError::NAK) => {
                        continue;
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }

            let mut csplit_count = 0;

            // Do the csplit, retry on NYET
            loop {
                self.pipe.set_addr(tt_addr);
                self.pipe.split(true, tt_port, endpoint_type, speed).await?;
                self.pipe.set_addr(address);
                let in_fut = self.pipe.data_in(endpoint, tog, true, false, buf);
                match in_fut.await {
                    Ok(size) => {
                        return Ok(size)
                    },
                    Err(UsbHostError::NYET) => {
                        // TODO:
                        // I don't understand the spec here. Windows + WCH both does retry of the CSPLIt
                        // But from what I can tell by reading the spec, it should be a retry of SSPLIT,
                        // because we should be "last" since we are expecting either a brief report or NAK.
                        // It seems like that I am understanding "last" incorrectly. We also retry the SSPLIT
                        // on failure, because it seems like sometimes the TT does not actually process the
                        // SSPLIT, causing us to retry the CSPLIT indefinitely.
                        // if endpoint_type == EndpointType::Interrupt {
                        // Maybe do something speical? consider the spec draw these differently
                        // }
                        Timer::after_micros(20).await;
                        csplit_count += 1;
                        if csplit_count >= 5 {
                            break;
                        }
                        continue;
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
        }

        // If 3 retry failed, return stall
        Err(UsbHostError::STALL)
    }

    async fn data_in(
        &mut self,
        dev_info: &DevInfo,
        address: u8,
        endpoint: u8,
        endpoint_type: EndpointType,
        tog: DataTog,
        buf: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        if let Some((tt_addr, tt_port)) = dev_info.transaction_translator() {
            let fut = self.split_data_in(
                tt_addr,
                tt_port,
                address,
                endpoint,
                endpoint_type,
                dev_info.speed(),
                tog,
                buf,
            );
            match select(timeout_fut, fut).await {
                Either::First(_) => Err(UsbHostError::TransferTimeout),
                Either::Second(r) => r,
            }
        } else {
            let fut = self.pipe.data_in(endpoint, tog, true, true, buf);
            match select(timeout_fut, fut).await {
                Either::First(_) => Err(UsbHostError::TransferTimeout),
                Either::Second(r) => r,
            }
        }
    }

    async fn data_out_with_retry(
        &mut self,
        dev_info: &DevInfo,
        address: u8,
        endpoint: u8,
        endpoint_type: EndpointType,
        tog: DataTog,
        buf: &[u8],
    ) -> Result<(), UsbHostError> {
        loop {
            match self
                .data_out(dev_info, address, endpoint, endpoint_type, tog, buf)
                .await
            {
                Ok(()) => return Ok(()),
                Err(UsbHostError::NAK) => {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }

    async fn split_data_out(
        &mut self,
        tt_addr: u8,
        tt_port: u8,
        address: u8,
        endpoint: u8,
        endpoint_type: EndpointType,
        speed: UsbSpeed,
        tog: DataTog,
        buf: &[u8],
    ) -> Result<(), UsbHostError> {
        loop {
            self.pipe.set_addr(tt_addr);
            // TODO: this is a huge problem, fix
            self.pipe
                .split(false, tt_port, endpoint_type, speed)
                .await?;
            self.pipe.set_addr(address);
            let in_fut = self.pipe.data_out(endpoint, tog, Some(buf));

            match in_fut.await {
                Ok(_) => {
                    break;
                }
                Err(UsbHostError::NAK) => {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        // Do the csplit, retry on NYET
        loop {
            self.pipe.set_addr(tt_addr);
            self.pipe.split(true, tt_port, endpoint_type, speed).await?;
            self.pipe.set_addr(address);
            let in_fut = self.pipe.data_out(endpoint, tog, None);
            match in_fut.await {
                Ok(size) => return Ok(size),
                Err(UsbHostError::NYET) => {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }

    async fn data_out(
        &mut self,
        dev_info: &DevInfo,
        address: u8,
        endpoint: u8,
        endpoint_type: EndpointType,
        tog: DataTog,
        buf: &[u8],
    ) -> Result<(), UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        if let Some((tt_addr, tt_port)) = dev_info.transaction_translator() {
            let fut = self.split_data_out(
                tt_addr,
                tt_port,
                address,
                endpoint,
                endpoint_type,
                dev_info.speed(),
                tog,
                buf,
            );
            match select(timeout_fut, fut).await {
                Either::First(_) => Err(UsbHostError::TransferTimeout),
                Either::Second(r) => r,
            }
        } else {
            let fut = self.pipe.data_out(endpoint, tog, Some(buf));
            match select(timeout_fut, fut).await {
                Either::First(_) => Err(UsbHostError::TransferTimeout),
                Either::Second(r) => r,
            }
        }
    }
}

pub struct USBHostPipe<D: HostDriver, const NR_DEVICES: usize> {
    inner: Mutex<CriticalSectionRawMutex, USBHostPipeInner<D, NR_DEVICES>>,
}

impl<D: HostDriver, const NR_DEVICES: usize> USBHostPipe<D, NR_DEVICES> {
    pub fn new(pipe: D::Pipe) -> Self {
        Self {
            inner: Mutex::new(USBHostPipeInner {
                pipe,
                address_alloc: DeviceAddressManager::new(),
            }),
        }
    }

    pub async fn assign_device_address(
        &self,
        max_packet_size: u16,
        //TODO: take reference
        devinfo: DevInfo,
    ) -> Result<DeviceHandle, UsbHostError> {
        let mut inner = self.inner.lock().await;
        let handle = inner
            .address_alloc
            .alloc_device_address(max_packet_size, devinfo);

        if let Err(e) = (async || {
            let request = Request {
                request_type: {
                    use request::*;
                    let mut t = RequestType::default();
                    t.set_data_direction(RequestTypeDirection::HostToDevice);
                    t.set_recipient(RequestTypeRecipient::Device);
                    t
                },
                request: StandardDeviceRequest::SetAddress as u8,
                value: handle.address() as u16,
                index: 0,
                length: 0,
            };

            // Setup stage
            inner.setup(&devinfo, 0, &request).await?;
            // Status stage (no data)
            inner
                .data_in_with_retry(
                    &devinfo,
                    0,
                    0,
                    EndpointType::Control,
                    DataTog::DATA1,
                    &mut [],
                )
                .await?;

            Ok(())
        })()
        .await
        {
            inner.address_alloc.free_address(handle);
            return Err(e);
        }

        Ok(handle)
    }

    // TODO: maybe take a address, technically we can do this after enumeration
    async fn get_device_descriptor<'b>(
        &self,
        dev_info: &DevInfo,
        buf: &'b mut [u8],
    ) -> Result<&'b DeviceDescriptor, UsbHostError> {
        debug_assert!(buf.len() >= 18);
        let mut inner = self.inner.lock().await;
        // Setup Stage
        let request = Request {
            request_type: {
                use request::*;
                let mut rt = RequestType::default();
                rt.set_data_direction(RequestTypeDirection::DeviceToHost);
                rt.set_type(RequestTypeType::Standard);
                rt.set_recipient(RequestTypeRecipient::Device);
                rt
            },
            request: StandardDeviceRequest::GetDescriptor as u8,
            value: (1 << 8) | 0, // DescriptorType: 1(Device), Index 0
            index: 0,
            length: 64,
        };
        // default address upon initial connection
        inner.setup(dev_info, 0, &request).await?;
        trace!("setup finished");

        let mut tog = DataTog::DATA1;
        // Data stage
        let mut bytes_read = 0usize;
        let in_result = inner
            .data_in_with_retry(dev_info, 0, 0, EndpointType::Control, tog, buf)
            .await?;
        tog.next();
        bytes_read += in_result;

        while bytes_read < core::mem::size_of::<DeviceDescriptor>() {
            let chopped_off_buf = &buf[bytes_read..];
            // SAFETY:
            // If the return Ok(desc); statement within the match block was executed,
            // the borrow is no longer in effect. Therefore, the unsafe transmute
            // is safe because there are no other outstanding immutable borrows of
            // the memory region being modified.
            let in_result = inner
                .data_in_with_retry(dev_info, 0, 0, EndpointType::Control, tog, unsafe {
                    core::slice::from_raw_parts_mut(
                        chopped_off_buf.as_ptr() as *mut u8,
                        chopped_off_buf.len(),
                    )
                })
                .await?;
            tog.next();
            bytes_read += in_result;
        }

        // Status stage
        inner
            .data_out_with_retry(dev_info, 0, 0, EndpointType::Control, DataTog::DATA1, &[])
            .await?;

        debug_assert!(bytes_read == core::mem::size_of::<DeviceDescriptor>());
        let dev_desc = parse_descriptor(buf)
            .and_then(|desc| desc.device().ok_or(UsbHostError::InvalidResponse))?;
        Ok(dev_desc)
    }

    pub async fn interrupt_transfer(
        &self,
        interrupt_channel: &mut InterruptChannel,
        buf: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        let mut inner = self.inner.lock().await;
        let endpoint = interrupt_channel.endpoint_address.number;
        let tog = interrupt_channel.tog;
        let buf = buf;

        inner
            .pipe
            .set_addr(interrupt_channel.device_handle.address());

        let res = match interrupt_channel.endpoint_address.direction {
            types::EndpointDirection::In => {
                inner
                    .data_in(
                        &interrupt_channel.device_handle.dev_info(),
                        interrupt_channel.device_handle.address(),
                        endpoint,
                        EndpointType::Interrupt,
                        tog,
                        buf,
                    )
                    .await
            }
            types::EndpointDirection::Out => inner
                .data_out(
                    &interrupt_channel.device_handle.dev_info(),
                    interrupt_channel.device_handle.address(),
                    endpoint,
                    EndpointType::Interrupt,
                    tog,
                    buf,
                )
                .await
                .map(|_| 0),
        }?;
        interrupt_channel.tog.next();
        Ok(res)
    }

    pub async fn control_transfer(
        &self,
        device_handle: DeviceHandle,
        request: &Request,
        buffer: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        use request::RequestTypeDirection;
        let mut inner = self.inner.lock().await;
        let dir = request.request_type.data_direction();
        let mut bytes_received = 0usize;

        debug_assert!(buffer.len() >= request.length as usize);

        // Setup stage
        inner
            .setup(&device_handle.dev_info(), device_handle.address(), request)
            .await?;

        // (Optional) data stage
        if request.length > 0 {
            match dir {
                RequestTypeDirection::HostToDevice => todo!(),
                RequestTypeDirection::DeviceToHost => {
                    let mut tog = DataTog::DATA1;
                    loop {
                        let len = inner
                            .data_in_with_retry(
                                &device_handle.dev_info(),
                                device_handle.address(),
                                0,
                                EndpointType::Control,
                                tog,
                                &mut buffer[bytes_received..],
                            )
                            .await?;
                        tog.next();
                        bytes_received += len;
                        if len < device_handle.max_packet_size() as usize {
                            break;
                        }
                    }
                }
            }
        }

        // Status stage
        match dir {
            RequestTypeDirection::HostToDevice => {
                inner
                    .data_in_with_retry(
                        &device_handle.dev_info(),
                        device_handle.address(),
                        0,
                        EndpointType::Control,
                        DataTog::DATA1,
                        &mut [],
                    )
                    .await?;
            }
            RequestTypeDirection::DeviceToHost => {
                inner
                    .data_out_with_retry(
                        &device_handle.dev_info(),
                        device_handle.address(),
                        0,
                        EndpointType::Control,
                        DataTog::DATA1,
                        &[],
                    )
                    .await?;
            }
        }

        Ok(bytes_received)
    }

    pub async fn dev_attach(
        &self,
        dev_info: DevInfo,
    ) -> Result<(DeviceDescriptor, DeviceHandle), UsbHostError> {
        let mut buffer: [u8; 18] = [0u8; 18];
        let d = self.get_device_descriptor(&dev_info, &mut buffer).await?;
        let max_packet_size = d.max_packet_size;
        trace!("DeviceDescriptor: {}", d);

        let handle = self
            .assign_device_address(max_packet_size as u16, dev_info)
            .await?;
        trace!("Device addressed {}", handle.address());

        Ok((d.clone(), handle))
    }

    pub async fn root_detach(&self) -> DeviceDisconnectMask {
        let mut inner = self.inner.lock().await;
        inner.address_alloc.free_all_addresses()
    }

    pub async fn dev_detach(&self, port_info: PortInfo) -> DeviceDisconnectMask {
        let mut inner = self.inner.lock().await;
        inner.address_alloc.free_subtree(port_info)
    }
}
