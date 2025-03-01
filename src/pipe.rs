use core::mem::transmute;

use arrayvec::ArrayVec;
use embassy_futures::select::{select, Either};
use embassy_time::Timer;

use crate::{descriptor::{parse_descriptor, DeviceDescriptor}, errors::UsbHostError, request::{self, Request, StandardDeviceRequest}, types::{self, DataTog, InterruptChannel}, DeviceAddressAllocator, DeviceHandle, Driver, InterruptTransfer, TRANSFER_TIMEOUT};

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

pub(crate) struct PipeWrap<D: Driver>(D::Pipe);

impl<D: Driver> PipeWrap<D> {
    pub fn new(pipe: D::Pipe) -> Self {
        Self(pipe)
    }

    async fn setup(&mut self, req: &Request) -> Result<(), UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        let setup_fut = self.0.setup(unsafe { transmute(req) });
        match select(timeout_fut, setup_fut).await {
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
        let mut data_in_with_retry = async || loop {
            match self.0.data_in(endpoint, tog, buf).await {
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

    async fn data_out(
        &mut self,
        endpoint: u8,
        tog: DataTog,
        buf: &[u8],
    ) -> Result<(), UsbHostError> {
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);
        // TODO retry like data_in
        let data_fut = self.0.data_out(endpoint, tog, buf);
        match select(timeout_fut, data_fut).await {
            Either::First(_) => Err(UsbHostError::TransferTimeout),
            Either::Second(r) => r,
        }
    }

    fn set_addr(&mut self, addr: u8) {
        self.0.set_addr(addr);
    }

    pub async fn assign_device_address(&mut self, addr: u8) -> Result<(), UsbHostError> {
        let request = Request {
            request_type: {
                use request::*;
                let mut t = RequestType::default();
                t.set_data_direction(RequestTypeDirection::HostToDevice);
                t.set_recipient(RequestTypeRecipient::Device);
                t
            },
            request: StandardDeviceRequest::SetAddress as u8,
            value: addr as u16,
            index: 0,
            length: 0,
        };

        // Setup stage
        self.setup(&request).await?;
        // Status stage (no data)
        self.data_in(0, DataTog::DATA1, &mut []).await?;

        Ok(())
    }

    pub async fn get_device_descriptor<'a>(
        &mut self,
        buf: &'a mut [u8],
    ) -> Result<&'a DeviceDescriptor, UsbHostError> {
        debug_assert!(buf.len() >= 18);
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
        self.setup(&request).await?;
        trace!("setup finished");

        let mut tog = DataTog::DATA1;
        // Data stage
        let mut bytes_read = 0usize;
        let in_result = self.data_in(0, tog, buf).await?;
        tog.next();
        bytes_read += in_result;

        while bytes_read < core::mem::size_of::<DeviceDescriptor>() {
            let chopped_off_buf = &buf[bytes_read..];
            // SAFETY:
            // If the return Ok(desc); statement within the match block was executed,
            // the borrow is no longer in effect. Therefore, the unsafe transmute
            // is safe because there are no other outstanding immutable borrows of
            // the memory region being modified.
            let in_result = self
                .data_in(0, tog, unsafe {
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
        self.data_out(0, DataTog::DATA1, &[]).await?;

        debug_assert!(bytes_read == core::mem::size_of::<DeviceDescriptor>());
        let dev_desc = parse_descriptor(buf)
            .and_then(|desc| desc.device().ok_or(UsbHostError::InvalidResponse))?;
        Ok(dev_desc)
    }

    pub async fn try_interrupt_transfer(
        &mut self,
        interrupt_channel: &mut InterruptChannel,
        buffer: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        let endpoint = interrupt_channel.endpoint_address.number;
        let tog = interrupt_channel.tog;
        let buf = buffer;
        let timeout_fut = Timer::after(TRANSFER_TIMEOUT);

        self.set_addr(interrupt_channel.device_handle.address());
        let mut interrupt_transfer = async || {
            let res = match interrupt_channel.endpoint_address.direction {
                types::EndpointDirection::In => self.0.data_in(endpoint, tog, buf).await,
                types::EndpointDirection::Out => {
                    self.0.data_out(endpoint, tog, buf).await.map(|_| 0)
                }
            }?;
            interrupt_channel.tog.next();
            return Ok(res);
        };
        let interrupt_transfer_fut = interrupt_transfer();
        match select(timeout_fut, interrupt_transfer_fut).await {
            Either::First(_) => Err(UsbHostError::TransferTimeout),
            Either::Second(r) => r,
        }
    }

    pub async fn interrupt_transfer<'a, const NR_PENDING_TRANSFERS: usize>(
        &mut self,
        interrupt_xfer: &mut ArrayVec<InterruptTransfer<'a>, NR_PENDING_TRANSFERS>,
    ) {
        for t in interrupt_xfer {
            match self
                .try_interrupt_transfer(&mut t.channel, t.buffer)
                .await
            {
                Ok(_) => return, // TODO: Do something? maybe
                Err(_) => {
                    // nothing to do try again LOL
                }
            }
        }

    }

    pub async fn control_transfer(
        &mut self,
        device_handle: DeviceHandle,
        request: &Request,
        buffer: &mut [u8],
    ) -> Result<usize, UsbHostError> {
        use request::RequestTypeDirection;
        let dir = request.request_type.data_direction();
        let mut bytes_received = 0usize;

        debug_assert!(buffer.len() >= request.length as usize);

        self.set_addr(device_handle.address());

        // Setup stage
        self.setup(request).await?;

        // (Optional) data stage
        if request.length > 0 {
            match dir {
                RequestTypeDirection::HostToDevice => todo!(),
                RequestTypeDirection::DeviceToHost => {
                    let mut tog = DataTog::DATA1;
                    loop {
                        let len = self.data_in(0, tog, &mut buffer[bytes_received..]).await?;
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
                self.data_in(0, DataTog::DATA1, &mut []).await?;
            }
            RequestTypeDirection::DeviceToHost => {
                self.data_out(0, DataTog::DATA1, &[]).await?;
            }
        }

        Ok(bytes_received)
    }

    pub async fn dev_attach(
        &mut self,
        address_alloc: &mut DeviceAddressAllocator,
    ) -> Result<(DeviceDescriptor, DeviceHandle), UsbHostError> {
        let mut buffer: [u8; 18] = [0u8; 18];
        let d = self.get_device_descriptor(&mut buffer).await?;
        let max_packet_size = d.max_packet_size;
        let handle = address_alloc.alloc_device_address(max_packet_size);
        trace!("DeviceDescriptor: {}", d);

        self.assign_device_address(handle.address()).await?;
        trace!("Device addressed {}", handle.address());

        Ok((
            d.clone(),
            handle,
        ))
    }
}