//! Shared USB enumeration for the PDBG and CMSIS-DAP bulk interfaces.

use rusb::{Device, DeviceDescriptor, DeviceHandle, GlobalContext};

use crate::error::{Error, Result};

pub const USB_VENDOR_ID: u16 = 0x2e8a;
pub const USB_PRODUCT_ID: u16 = 0x000c;

#[derive(Clone, Debug)]
pub struct Location {
    pub bus: u8,
    pub ports: Vec<u8>,
}

/// Locate the ROM bootloader on the same physical USB port as the selected probe.
pub fn bootloader_at(location: &Location, product_id: u16) -> Result<Option<(u8, u8)>> {
    for device in rusb::devices()?.iter() {
        let descriptor = device.device_descriptor()?;
        if descriptor.vendor_id() == USB_VENDOR_ID
            && descriptor.product_id() == product_id
            && device.bus_number() == location.bus
            && device.port_numbers()? == location.ports
        {
            return Ok(Some((device.bus_number(), device.address())));
        }
    }
    Ok(None)
}

/// Discovery does not claim either interface or change target state.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub serial: Option<String>,
    pub bus: u8,
    pub address: u8,
    pub product: String,
    pub access_error: Option<String>,
}

pub fn enumerate_devices() -> Result<Vec<DeviceInfo>> {
    let mut found = Vec::new();
    for device in rusb::devices()?.iter() {
        let descriptor = device.device_descriptor()?;
        if descriptor.vendor_id() != USB_VENDOR_ID || descriptor.product_id() != USB_PRODUCT_ID {
            continue;
        }
        // The official Debug Probe shares this VID/PID but has no PDBG bridge.
        // Match the PDBG interface before offering power/UART/telemetry controls.
        if find_class_interface(&device, 0xff, 0x50, 0x01)?.is_none() {
            continue;
        }
        let mut info = DeviceInfo {
            serial: None,
            bus: device.bus_number(),
            address: device.address(),
            product: "PDBG / CMSIS-DAP".into(),
            access_error: None,
        };
        match device.open() {
            Ok(mut handle) => {
                match read_serial(&mut handle, &descriptor) {
                    Ok(serial) => info.serial = serial,
                    Err(error) => info.access_error = Some(error.to_string()),
                }
                if let Ok(product) = handle.read_product_string_ascii(&descriptor) {
                    info.product = product;
                }
            }
            Err(error) => info.access_error = Some(error.to_string()),
        }
        found.push(info);
    }
    Ok(found)
}

/// Optional serial number to match a specific board instance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceSelector {
    pub serial: Option<String>,
}

impl DeviceSelector {
    pub fn any() -> Self {
        Self::default()
    }

    pub fn serial(serial: impl Into<String>) -> Self {
        Self {
            serial: Some(serial.into()),
        }
    }
}

/// An open handle with one USB interface claimed.
pub(crate) struct OpenedInterface {
    pub handle: DeviceHandle<GlobalContext>,
    pub interface: u8,
    pub endpoint_out: u8,
    pub endpoint_in: u8,
}

/// Scan connected devices and open the first one whose interface matches the
/// given USB descriptor classes, claiming it and applying the serial selector.
pub(crate) fn open_selector(
    selector: &DeviceSelector,
    interface_class: u8,
    interface_subclass: u8,
    interface_protocol: u8,
) -> Result<OpenedInterface> {
    for device in rusb::devices()?.iter() {
        let descriptor = device.device_descriptor()?;
        if descriptor.vendor_id() != USB_VENDOR_ID || descriptor.product_id() != USB_PRODUCT_ID {
            continue;
        }
        let Some((interface, endpoint_out, endpoint_in)) = find_class_interface(
            &device,
            interface_class,
            interface_subclass,
            interface_protocol,
        )?
        else {
            continue;
        };
        let mut handle = device.open()?;
        let serial = read_serial(&mut handle, &descriptor)?;
        if let Some(expected) = selector.serial.as_deref() {
            if serial.as_deref() != Some(expected) {
                continue;
            }
        }
        handle.set_auto_detach_kernel_driver(true)?;
        handle.claim_interface(interface)?;
        return Ok(OpenedInterface {
            handle,
            interface,
            endpoint_out,
            endpoint_in,
        });
    }
    let suffix = selector
        .serial
        .as_ref()
        .map(|serial| format!(" with serial {serial}"))
        .unwrap_or_default();
    Err(Error::DeviceNotFound(suffix))
}

/// Find an interface matching the three USB descriptor classes and return it as
/// `(interface, OUT endpoint, IN endpoint)`.
fn find_class_interface(
    device: &Device<GlobalContext>,
    interface_class: u8,
    interface_subclass: u8,
    interface_protocol: u8,
) -> Result<Option<(u8, u8, u8)>> {
    let config = device.active_config_descriptor()?;
    for interface in config.interfaces() {
        for descriptor in interface.descriptors() {
            if descriptor.class_code() != interface_class
                || descriptor.sub_class_code() != interface_subclass
                || descriptor.protocol_code() != interface_protocol
            {
                continue;
            }
            let mut endpoint_out = None;
            let mut endpoint_in = None;
            for endpoint in descriptor.endpoint_descriptors() {
                if endpoint.transfer_type() != rusb::TransferType::Bulk {
                    continue;
                }
                if endpoint.direction() == rusb::Direction::In {
                    endpoint_in = Some(endpoint.address());
                } else {
                    endpoint_out = Some(endpoint.address());
                }
            }
            if let (Some(out), Some(input)) = (endpoint_out, endpoint_in) {
                return Ok(Some((descriptor.interface_number(), out, input)));
            }
            return Err(Error::MissingEndpoint {
                interface: descriptor.interface_number(),
                direction: "IN/OUT",
            });
        }
    }
    Ok(None)
}

fn read_serial(
    handle: &mut DeviceHandle<GlobalContext>,
    descriptor: &DeviceDescriptor,
) -> Result<Option<String>> {
    let Some(index) = descriptor.serial_number_string_index() else {
        return Ok(None);
    };
    Ok(Some(handle.read_string_descriptor_ascii(index)?))
}
