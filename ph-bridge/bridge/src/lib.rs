//! # Pico Harness Bridge (PH Bridge)
//!
//! The single connection layer between the PC and the Debug core.
//!
//! - `msg`    : generated protobuf types (`ph-bridge/proto/ph.proto`, package `picoharness.v1`)
//! - `framing`: length-delimited protobuf framing (the PH protocol envelope)
//! - `client` : `PhClient` — request/response + unsolicited-event handling over a
//!   byte transport (a `PH Terminal` CDC serial port in practice)
//! - `device` : device discovery on Linux/macOS (sysfs)
//! - `bulk`   : `BulkWriter` — raw payloads on the `PH Terminal Bulk` interface
//!
//! Naming follows the repository convention: `ph_` / `Ph_` public symbols.

pub mod bulk;
pub mod client;
pub mod device;
pub mod framing;
pub mod msg;

pub use bulk::BulkWriter;
pub use client::{PhClient, PhError};
pub use msg::{GetInfo, GetStatus, Measure, PhMessage, Status};

/// Max encoded PH message accepted on the wire, mirroring the firmware's
/// `PH_MSG_MAX`. The largest payload (ConsoleTx/ConsoleData, 128 B) fits far
/// below this.
pub const PH_MSG_MAX: usize = 512;
