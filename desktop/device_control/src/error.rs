use thiserror::Error;

use crate::protocol::ProtocolError;

#[derive(Debug, Error)]
pub enum Error {
    #[error("USB error: {0}")]
    Usb(#[from] rusb::Error),

    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),

    #[error("PDBG device was not found{0}")]
    DeviceNotFound(String),

    #[error("USB interface {interface} has no bulk {direction} endpoint")]
    MissingEndpoint {
        interface: u8,
        direction: &'static str,
    },

    #[error("device returned status {status} for command 0x{command:02x}")]
    DeviceStatus { command: u8, status: u8 },

    #[error("unexpected response to command 0x{command:02x}: {detail}")]
    UnexpectedResponse { command: u8, detail: String },

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("SWD transfer failed with ACK 0x{ack:02x}")]
    SwdAck { ack: u8 },

    #[error("USB response timed out")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, Error>;
