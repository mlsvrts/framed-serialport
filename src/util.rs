pub use serialport::available_ports;

/// Attempts to detect a USB serial port, using the device's VID, PID, and Interface Id
///
/// Support USB interface ids requires a branch of [`serialport`], which will be removed
/// when the feature is merged into the core library; this feature is useful for selecting
/// a specific interface on composite serial devices.
pub fn detect_usb_port(
    vid: u16,
    pid: u16,
    interface: Option<u8>,
    ports: &Vec<serialport::SerialPortInfo>,
) -> Option<String> {
    for port in ports {
        match &port.port_type {
            serialport::SerialPortType::UsbPort(info) => {
                if vid == info.vid && pid == info.pid && info.interface == interface {
                    return Some(port.port_name.clone());
                }
            }
            _ => continue,
        }
    }

    None
}

/// Attempts to list and then detect a USB serial port using the device's VID, PID, and Interface Id
pub fn try_detect_usb_port(
    vid: u16,
    pid: u16,
    interface: Option<u8>,
) -> Result<String, serialport::Error> {
    let ports = available_ports()?;

    detect_usb_port(vid, pid, interface, &ports).ok_or(serialport::Error::new(
        serialport::ErrorKind::NoDevice,
        format!(
            "failed to detect a usb serialport at 0x{:X} 0x{:X} {:?}",
            vid, pid, interface
        ),
    ))
}
