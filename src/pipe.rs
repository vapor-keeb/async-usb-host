use core::mem::transmute;

use arrayvec::ArrayVec;
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex, pipe};
use embassy_time::Timer;

use crate::{
    descriptor::{parse_descriptor, DeviceDescriptor},
    device_addr::DeviceDisconnectMask,
    errors::UsbHostError,
    request::{self, Request, StandardDeviceRequest},
    types::{self, DataTog, DevInfo, InterruptChannel},
    DeviceAddressManager, DeviceHandle, Driver, TRANSFER_TIMEOUT,
};

// not Send anyways
#[allow(async_fn_in_trait)]
pub trait Pipe {
    fn set_addr(&mut self, addr: u8);
    /// When setup is called, it should send a setup request, also setup the
    /// hardware to send / expect DATA1 packets on subsequent data_in / data_out
    async fn setup(&mut self, buf: &[u8; 8]) -> Result<(), UsbHostError>;
    async fn data_in(
        &mut self,
        endpoint: u8,
        tog: DataTog,
        buf: &mut [u8],
    ) -> Result<usize, UsbHostError>;
    async fn data_out(
        &mut self,
        endpoint: u8,
        tog: DataTog,
        buf: &[u8],
    ) -> Result<(), UsbHostError>;
}

struct USBHostPipeInner<D: Driver, const NR_DEVICES: usize> {
    pipe: D::Pipe,
    address_alloc: DeviceAddressManager<NR_DEVICES>,
}

impl<D: Driver, const NR_DEVICES: usize> USBHostPipeInner<D, NR_DEVICES> {
    async fn setup(&mut self, req: &Request) -> Result<(), UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        #[cfg(not(target_endian = "little"))]
        compile_error!("Only little endian supported");
        let setup_fut = self.pipe.setup(unsafe { transmute(req) });
        match select(timeout_fut, setup_fut).await {
            Either::First(_) => Err(UsbHostError::TransferTimeout),
            Either::Second(r) => r,
        }
    }

    async fn data_in_with_retry(
        &mut self,
        endpoint: u8,
        tog: DataTog,
        buf: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        let mut data_in_with_retry = async || loop {
            match self.pipe.data_in(endpoint, tog, buf).await {
                Ok(size) => return Ok(size),
                Err(UsbHostError::NAK) => {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        };
        let data_in_fut = data_in_with_retry();
        match select(timeout_fut, data_in_fut).await {
            Either::First(_) => Err(UsbHostError::TransferTimeout),
            Either::Second(r) => r,
        }
    }

    async fn data_in(
        &mut self,
        endpoint: u8,
        tog: DataTog,
        buf: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        let data_fut = self.pipe.data_in(endpoint, tog, buf);
        match select(timeout_fut, data_fut).await {
            Either::First(_) => Err(UsbHostError::TransferTimeout),
            Either::Second(r) => r,
        }
    }

    async fn data_out(
        &mut self,
        endpoint: u8,
        tog: DataTog,
        buf: &[u8],
    ) -> Result<(), UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        let data_fut = self.pipe.data_out(endpoint, tog, buf);
        match select(timeout_fut, data_fut).await {
            Either::First(_) => Err(UsbHostError::TransferTimeout),
            Either::Second(r) => r,
        }
    }
}

pub struct USBHostPipe<D: Driver, const NR_DEVICES: usize> {
    inner: Mutex<CriticalSectionRawMutex, USBHostPipeInner<D, NR_DEVICES>>,
}

impl<D: Driver, const NR_DEVICES: usize> USBHostPipe<D, NR_DEVICES> {
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
        parent: DevInfo,
    ) -> Result<DeviceHandle, UsbHostError> {
        let mut inner = self.inner.lock().await;
        let handle = inner
            .address_alloc
            .alloc_device_address(max_packet_size, parent);

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
            inner.setup(&request).await?;
            // Status stage (no data)
            inner.data_in_with_retry(0, DataTog::DATA1, &mut []).await?;

            Ok(())
        })()
        .await
        {
            inner.address_alloc.free_address(handle);
            return Err(e);
        }

        Ok(handle)
    }

    async fn get_device_descriptor<'b>(
        &self,
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
        inner.pipe.set_addr(0);
        inner.setup(&request).await?;
        trace!("setup finished");

        let mut tog = DataTog::DATA1;
        // Data stage
        let mut bytes_read = 0usize;
        let in_result = inner.data_in_with_retry(0, tog, buf).await?;
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
                .data_in_with_retry(0, tog, unsafe {
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
        inner.data_out(0, DataTog::DATA1, &[]).await?;

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
            types::EndpointDirection::In => inner.data_in(endpoint, tog, buf).await,
            types::EndpointDirection::Out => inner.data_out(endpoint, tog, buf).await.map(|_| 0),
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

        inner.pipe.set_addr(device_handle.address());

        // Setup stage
        inner.setup(request).await?;

        // (Optional) data stage
        if request.length > 0 {
            match dir {
                RequestTypeDirection::HostToDevice => todo!(),
                RequestTypeDirection::DeviceToHost => {
                    let mut tog = DataTog::DATA1;
                    loop {
                        let len = inner
                            .data_in_with_retry(0, tog, &mut buffer[bytes_received..])
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
                inner.data_in_with_retry(0, DataTog::DATA1, &mut []).await?;
            }
            RequestTypeDirection::DeviceToHost => {
                inner.data_out(0, DataTog::DATA1, &[]).await?;
            }
        }

        Ok(bytes_received)
    }

    pub async fn dev_attach(
        &self,
        parent: DevInfo,
    ) -> Result<(DeviceDescriptor, DeviceHandle), UsbHostError> {
        let mut buffer: [u8; 18] = [0u8; 18];
        let d = self.get_device_descriptor(&mut buffer).await?;
        let max_packet_size = d.max_packet_size;
        trace!("DeviceDescriptor: {}", d);

        let handle = self
            .assign_device_address(max_packet_size as u16, parent)
            .await?;
        trace!("Device addressed {}", handle.address());

        Ok((d.clone(), handle))
    }

    pub async fn root_detach(&self) -> DeviceDisconnectMask {
        let mut inner = self.inner.lock().await;
        inner.address_alloc.free_all_addresses()
    }

    pub async fn dev_detach(&self, dev_info: DevInfo) -> DeviceDisconnectMask {
        let mut inner = self.inner.lock().await;
        inner.address_alloc.free_subtree(dev_info)
    }
}
