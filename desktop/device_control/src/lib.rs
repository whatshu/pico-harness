//! Rust access to the dualcore Debug Core device.
//!
//! The board exposes two independent USB bulk interfaces. [`PdbgLink`] carries
//! board I/O and power commands using the PDBG protocol, while [`CmsisDap`]
//! drives Work-core SWD through the standard CMSIS-DAP v2 interface.

mod cmsis_dap;
mod error;
mod pdbg;
pub mod protocol;
pub mod usb;

pub use cmsis_dap::{CmsisDap, TransferRequest};
pub use error::{Error, Result};
pub use pdbg::PdbgLink;
pub use protocol::{CaptureResult, Command, Measurement, PowerStatus, UartStatus};
pub use usb::{enumerate_devices, DeviceInfo, DeviceSelector, USB_PRODUCT_ID, USB_VENDOR_ID};
