//! Versioned socket contract shared by the daemon and its clients.
//!
//! A connection is an authenticated session. Requests and replies are bounded,
//! newline-delimited JSON-RPC 2.0; one request is outstanding per connection.
//! Different connections can progress independently.

pub mod board;
pub mod logic;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead};

pub const VERSION: u32 = 1;
pub const DEFAULT_ADDR: &str = "127.0.0.1:44840";
pub const MAX_FRAME: usize = 2 * 1024 * 1024;
pub const MAX_DATA: usize = 1024 * 1024;

/// Read at most one bounded frame without allowing an untrusted peer to grow
/// memory indefinitely. EOF between frames is a clean session disconnect.
pub fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete JSON frame",
                ))
            };
        }
        let end = available.iter().position(|b| *b == b'\n');
        let count = end.map_or(available.len(), |n| n + 1);
        if frame.len() + count > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSON frame exceeds limit",
            ));
        }
        frame.extend_from_slice(&available[..count]);
        reader.consume(count);
        if end.is_some() {
            return Ok(Some(frame));
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error)]
#[error("{kind}: {message}")]
pub struct RpcError {
    pub code: i32,
    pub kind: String,
    pub message: String,
    /// Only queries marked retryable may be repeated automatically. Mutation
    /// failures can mean that the device acted before its reply was lost.
    pub retryable: bool,
}

impl RpcError {
    pub fn new(code: i32, kind: &str, message: impl Into<String>) -> Self {
        Self {
            code,
            kind: kind.into(),
            message: message.into(),
            retryable: false,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(-32602, "invalid_argument", message)
    }
    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(-32003, "busy", message)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub serial: Option<String>,
    pub bus: u8,
    pub address: u8,
    pub product: String,
    pub access_error: Option<String>,
    pub terminal: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Location {
    pub bus: u8,
    pub ports: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DebugConfig {
    pub serial: String,
    pub target: String,
    #[serde(default = "default_openocd")]
    pub openocd: String,
    #[serde(default)]
    pub scripts: String,
}
fn default_openocd() -> String {
    "openocd".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventBatch {
    pub instance: String,
    pub cursor: u64,
    pub gap: bool,
    pub events: Vec<Value>,
}

pub mod dap;
