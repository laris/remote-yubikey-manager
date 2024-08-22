//! Host USB
use super::*;

/// A handler to pass requests to a USB device of the host
#[derive(Clone)]
pub struct UsbHostInterfaceHandler {
    handle: Arc<Mutex<DeviceHandle<GlobalContext>>>,
}

impl UsbHostInterfaceHandler {
    pub fn new(handle: Arc<Mutex<DeviceHandle<GlobalContext>>>) -> Self {
        Self { handle }
    }
}

impl UsbInterfaceHandler for UsbHostInterfaceHandler {
    fn handle_urb(
        &mut self,
        _interface: &UsbInterface,
        ep: UsbEndpoint,
        transfer_buffer_length: u32,
        setup: SetupPacket,
        req: &[u8],
    ) -> Result<Vec<u8>> {
        debug!(
            "To host device: ep={:?} setup={:?} req={:?}",
            ep, setup, req
        );
        let mut buffer = vec![0u8; transfer_buffer_length as usize];
        let timeout = std::time::Duration::new(1, 0);
        let handle = self.handle.lock().unwrap();
        if ep.attributes == EndpointAttributes::Control as u8 {
            // control
            if let Direction::In = ep.direction() {
                // control in
                if let Ok(len) = handle.read_control(
                    setup.request_type,
                    setup.request,
                    setup.value,
                    setup.index,
                    &mut buffer,
                    timeout,
                ) {
                    return Ok(Vec::from(&buffer[..len]));
                }
            } else {
                // control out
                handle
                    .write_control(
                        setup.request_type,
                        setup.request,
                        setup.value,
                        setup.index,
                        req,
                        timeout,
                    )
                    .ok();
            }
        } else if ep.attributes == EndpointAttributes::Interrupt as u8 {
            // interrupt
            if let Direction::In = ep.direction() {
                // interrupt in
                if let Ok(len) = handle.read_interrupt(ep.address, &mut buffer, timeout) {
                    info!("intr in {:?}", &buffer[..len]);
                    return Ok(Vec::from(&buffer[..len]));
                }
            } else {
                // interrupt out
                handle.write_interrupt(ep.address, req, timeout).ok();
            }
        } else if ep.attributes == EndpointAttributes::Bulk as u8 {
            // bulk
            if let Direction::In = ep.direction() {
                // bulk in
                if let Ok(len) = handle.read_bulk(ep.address, &mut buffer, timeout) {
                    return Ok(Vec::from(&buffer[..len]));
                }
            } else {
                // bulk out
                handle.write_bulk(ep.address, req, timeout).ok();
            }
        }
        Ok(vec![])
    }

    fn get_class_specific_descriptor(&self) -> Vec<u8> {
        vec![]
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

/// A handler to pass requests to a USB device of the host
#[derive(Clone)]
pub struct UsbHostDeviceHandler {
    handle: Arc<Mutex<DeviceHandle<GlobalContext>>>,
}

impl UsbHostDeviceHandler {
    pub fn new(handle: Arc<Mutex<DeviceHandle<GlobalContext>>>) -> Self {
        Self { handle }
    }
}

impl UsbDeviceHandler for UsbHostDeviceHandler {
    fn handle_urb(
        &mut self,
        transfer_buffer_length: u32,
        setup: SetupPacket,
        req: &[u8],
    ) -> Result<Vec<u8>> {
        debug!("To host device: setup={:?} req={:?}", setup, req);
        let mut buffer = vec![0u8; transfer_buffer_length as usize];
        let timeout = std::time::Duration::new(1, 0);
        let handle = self.handle.lock().unwrap();
        // control
        if setup.request_type & 0x80 == 0 {
            // control out
            handle
                .write_control(
                    setup.request_type,
                    setup.request,
                    setup.value,
                    setup.index,
                    req,
                    timeout,
                )
                .ok();
        } else {
            // control in
            if let Ok(len) = handle.read_control(
                setup.request_type,
                setup.request,
                setup.value,
                setup.index,
                &mut buffer,
                timeout,
            ) {
                return Ok(Vec::from(&buffer[..len]));
            }
        }
        Ok(vec![])
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}
