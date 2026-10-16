//! Rust access to the dualcore Debug Core device.
//!
//! [`PdbgLink`] carries board control (power, measurement, console, baud) over
//! the PH protocol on the CDC `PH Terminal`; it is a thin adapter over the
//! `ph_bridge` crate (see `host/ph-bridge` and `doc/ARCHITECTURE.md`).
//! [`CmsisDap`] drives Work-core SWD through the standard CMSIS-DAP v2
//! interface. The name `PdbgLink` is kept because the desktop app still calls
//! it; the PDBG wire protocol itself is retired.

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
