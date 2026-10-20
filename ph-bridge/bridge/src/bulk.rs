//! PH Terminal Bulk — raw payload writer over the vendor bulk interface (IF3).
//!
//! Coordination lives in the PH protocol on the CDC `PH Terminal`; this module
//! only moves the raw payload bytes on the vendor bulk endpoints.

use rusb::{Device, DeviceHandle, GlobalContext};

use crate::device::{USB_PRODUCT_ID, USB_VENDOR_ID};
use crate::PhError;

const BULK_IFNUM: u8 = 3;

/// Sender for the `PH Terminal Bulk` payload channel.
pub struct BulkWriter {
    handle: DeviceHandle<GlobalContext>,
    iface: u8,
    ep_out: u8,
}

impl BulkWriter {
    /// Open the vendor bulk interface of the PicoHarness probe.
    ///
    /// `serial` (optional) pins a specific probe instance so the writer matches
    /// the same device the PH Terminal client is talking to.
    pub fn open(serial: Option<&str>) -> Result<Self, PhError> {
        for dev in rusb::devices()?.iter() {
            let desc = dev.device_descriptor()?;
            if desc.vendor_id() != USB_VENDOR_ID || desc.product_id() != USB_PRODUCT_ID {
                continue;
            }
            if let Some(want) = serial {
                let handle = dev.open()?;
                let got = handle
                    .read_string_descriptor_ascii(desc.serial_number_string_index().unwrap_or(0));
                if got.as_deref().ok() != Some(want) {
                    continue;
                }
            }
            let addrs = pick_endpoints(&dev)?;
            let handle = dev.open()?;
            // Kernel detach is unavailable on Windows and macOS; claiming the
            // vendor interface there does not require it.
            match handle.set_auto_detach_kernel_driver(true) {
                Ok(()) | Err(rusb::Error::NotSupported) => {}
                Err(error) => return Err(error.into()),
            }
            handle.claim_interface(BULK_IFNUM)?;
            return Ok(Self {
                handle,
                iface: BULK_IFNUM,
                ep_out: addrs.0,
            });
        }
        Err(PhError::Usb(rusb::Error::NotFound))
    }

    /// Write a raw payload chunk (delivered on the vendor OUT endpoint).
    pub fn write(&mut self, data: &[u8]) -> Result<(), PhError> {
        let written =
            self.handle
                .write_bulk(self.ep_out, data, std::time::Duration::from_secs(30))?;
        if written != data.len() {
            return Err(PhError::Usb(rusb::Error::Other));
        }
        Ok(())
    }
}

impl Drop for BulkWriter {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(self.iface);
    }
}

fn pick_endpoints(dev: &Device<GlobalContext>) -> Result<(u8, u8), rusb::Error> {
    let config = dev.active_config_descriptor()?;
    for interface in config.interfaces() {
        for desc in interface.descriptors() {
            if desc.interface_number() != BULK_IFNUM {
                continue;
            }
            let mut out = None;
            let mut input = None;
            for ep in desc.endpoint_descriptors() {
                if ep.transfer_type() != rusb::TransferType::Bulk {
                    continue;
                }
                if ep.direction() == rusb::Direction::In {
                    input = Some(ep.address());
                } else {
                    out = Some(ep.address());
                }
            }
            if let (Some(o), Some(i)) = (out, input) {
                return Ok((o, i));
            }
        }
    }
    Err(rusb::Error::NotFound)
}
