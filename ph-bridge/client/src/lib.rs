//! Device clients for phd. This crate has no USB or serial-port dependency.
//! Dropping a connection releases its exclusive daemon resources.

mod cmsis_dap;
mod link;
pub mod rpc;
pub mod usb;
pub use cmsis_dap::{CmsisDap, TransferRequest};
pub use link::{PhBoardLink, UartTrace};
pub use ph_protocol::board as protocol;
pub use ph_protocol::logic as logic_capture;
pub use ph_protocol::{DebugConfig, DeviceInfo, DeviceSelector, EventBatch};
pub use protocol::{CaptureResult, Command, Measurement, PowerStatus, UartStatus};
pub use rpc::{Config, Error, Result, RpcClient};
pub use usb::enumerate_devices;

pub use ph_image as image;
pub mod debug;
pub mod debug_update;
pub mod firmware;
