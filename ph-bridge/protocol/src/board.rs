use std::convert::TryFrom;

pub const MAGIC: [u8; 4] = *b"PDBG";
pub const VERSION: u8 = 1;
pub const HEADER_SIZE: usize = 16;
pub const MAX_PAYLOAD: usize = 4096;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    GetInfo = 1,
    GetStatus = 2,
    PowerSet = 3,
    ResetPulse = 4,
    BootselSet = 5,
    EnterBootsel = 6,
    Measure = 7,
    GpioRead = 8,
    Capture = 9,
    CaptureRead = 10,
    GpioDiag = 11,
    GpioPull = 12,
    UartConfig = 13,
    UartWrite = 14,
    UartRead = 15,
    UartStatus = 16,
    SpiConfig = 17,
    SpiTransfer = 18,
    CaptureStatus = 19,
    DebugEnterBootsel = 20,
    CaptureStart = 21,
    CapturePoll = 22,
    CaptureCancel = 23,
}

impl Command {
    pub const fn response(self) -> u8 {
        self as u8 | 0x80
    }
}

impl From<Command> for u8 {
    fn from(value: Command) -> Self {
        value as u8
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub command: u8,
    pub flags: u16,
    pub sequence: u32,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn request(command: Command, sequence: u32, payload: &[u8]) -> Self {
        Self {
            command: command as u8,
            flags: 0,
            sequence,
            payload: payload.to_vec(),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        if self.payload.len() > MAX_PAYLOAD {
            return Err(ProtocolError::PayloadTooLarge(self.payload.len()));
        }
        let length = self.payload.len() as u16;
        let mut header = [0u8; HEADER_SIZE];
        header[0..4].copy_from_slice(&MAGIC);
        header[4] = VERSION;
        header[5] = self.command;
        header[6..8].copy_from_slice(&self.flags.to_le_bytes());
        header[8..12].copy_from_slice(&self.sequence.to_le_bytes());
        header[12..14].copy_from_slice(&length.to_le_bytes());
        let header_crc = crc16_ccitt(&header);
        header[14..16].copy_from_slice(&header_crc.to_le_bytes());

        let mut packet = Vec::with_capacity(HEADER_SIZE + self.payload.len() + 4);
        packet.extend_from_slice(&header);
        packet.extend_from_slice(&self.payload);
        packet.extend_from_slice(&crc32c(&self.payload).to_le_bytes());
        Ok(packet)
    }

    pub fn decode(packet: &[u8]) -> Result<Self, ProtocolError> {
        if packet.len() < HEADER_SIZE + 4 {
            return Err(ProtocolError::ShortFrame(packet.len()));
        }
        if packet[0..4] != MAGIC {
            return Err(ProtocolError::BadMagic);
        }
        if packet[4] != VERSION {
            return Err(ProtocolError::UnsupportedVersion(packet[4]));
        }
        let flags = u16::from_le_bytes([packet[6], packet[7]]);
        let sequence = u32::from_le_bytes([packet[8], packet[9], packet[10], packet[11]]);
        let length = u16::from_le_bytes([packet[12], packet[13]]) as usize;
        if length > MAX_PAYLOAD {
            return Err(ProtocolError::PayloadTooLarge(length));
        }
        let total = HEADER_SIZE + length + 4;
        if packet.len() != total {
            return Err(ProtocolError::LengthMismatch {
                expected: total,
                actual: packet.len(),
            });
        }

        let mut header = [0u8; HEADER_SIZE];
        header.copy_from_slice(&packet[..HEADER_SIZE]);
        let saved_header_crc = u16::from_le_bytes([header[14], header[15]]);
        header[14] = 0;
        header[15] = 0;
        if crc16_ccitt(&header) != saved_header_crc {
            return Err(ProtocolError::HeaderCrc);
        }

        let payload = packet[HEADER_SIZE..HEADER_SIZE + length].to_vec();
        let saved_payload_crc = u32::from_le_bytes([
            packet[total - 4],
            packet[total - 3],
            packet[total - 2],
            packet[total - 1],
        ]);
        if crc32c(&payload) != saved_payload_crc {
            return Err(ProtocolError::PayloadCrc);
        }

        Ok(Self {
            command: packet[5],
            flags,
            sequence,
            payload,
        })
    }

    pub fn total_length_from_prefix(prefix: &[u8]) -> Result<Option<usize>, ProtocolError> {
        if prefix.len() < HEADER_SIZE {
            return Ok(None);
        }
        if prefix[0..4] != MAGIC {
            return Err(ProtocolError::BadMagic);
        }
        if prefix[4] != VERSION {
            return Err(ProtocolError::UnsupportedVersion(prefix[4]));
        }
        let length = u16::from_le_bytes([prefix[12], prefix[13]]) as usize;
        if length > MAX_PAYLOAD {
            return Err(ProtocolError::PayloadTooLarge(length));
        }
        Ok(Some(HEADER_SIZE + length + 4))
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("short frame ({0} bytes)")]
    ShortFrame(usize),
    #[error("bad PDBG magic")]
    BadMagic,
    #[error("unsupported PDBG protocol version {0}")]
    UnsupportedVersion(u8),
    #[error("payload is too large ({0} bytes)")]
    PayloadTooLarge(usize),
    #[error("frame length mismatch: expected {expected}, got {actual}")]
    LengthMismatch { expected: usize, actual: usize },
    #[error("header CRC mismatch")]
    HeaderCrc,
    #[error("payload CRC mismatch")]
    PayloadCrc,
    #[error("device returned status {0}")]
    DeviceStatus(u8),
    #[error("unknown command 0x{0:02x}")]
    UnknownCommand(u8),
}

pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for byte in data {
        crc ^= (*byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82f63b78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct Measurement {
    pub shunt_uv: f64,
    pub bus_mv: f64,
    pub current_ma: f64,
    pub power_mw: f64,
}

impl Measurement {
    pub fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        if payload.is_empty() {
            return Err(ProtocolError::ShortFrame(0));
        }
        if payload[0] != 0 {
            return Err(ProtocolError::DeviceStatus(payload[0]));
        }
        if payload.len() == 17 {
            let shunt_uv = i32::from_le_bytes(payload[1..5].try_into().unwrap()) as f64;
            let bus_uv = i32::from_le_bytes(payload[5..9].try_into().unwrap()) as f64;
            let current_ua = i32::from_le_bytes(payload[9..13].try_into().unwrap()) as f64;
            let power_uw = i32::from_le_bytes(payload[13..17].try_into().unwrap()) as f64;
            return Ok(Self {
                shunt_uv,
                bus_mv: bus_uv / 1000.0,
                current_ma: current_ua / 1000.0,
                power_mw: power_uw / 1000.0,
            });
        }
        if payload.len() == 9 {
            let shunt = i16::from_le_bytes(payload[1..3].try_into().unwrap()) as f64;
            let bus = i16::from_le_bytes(payload[3..5].try_into().unwrap()) as f64;
            let current = i16::from_le_bytes(payload[5..7].try_into().unwrap()) as f64;
            let power = i16::from_le_bytes(payload[7..9].try_into().unwrap()) as f64;
            return Ok(Self {
                shunt_uv: shunt * 2.5,
                bus_mv: bus * 1.25,
                current_ma: current * 0.1,
                power_mw: power * 2.5,
            });
        }
        Err(ProtocolError::LengthMismatch {
            expected: 17,
            actual: payload.len(),
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerStatus {
    pub power_on: bool,
    pub power_good: bool,
    pub alert: bool,
    pub power_fault: bool,
    pub work_3v3_mv: u16,
}

impl PowerStatus {
    pub fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        if payload.len() != 4 && payload.len() != 7 {
            return Err(ProtocolError::LengthMismatch {
                expected: 7,
                actual: payload.len(),
            });
        }
        if payload[0] != 0 {
            return Err(ProtocolError::DeviceStatus(payload[0]));
        }
        Ok(Self {
            power_on: payload[1] != 0,
            power_good: payload[2] != 0,
            alert: payload[3] != 0,
            power_fault: payload.get(4).copied().unwrap_or(0) != 0,
            work_3v3_mv: if payload.len() == 7 {
                u16::from_le_bytes([payload[5], payload[6]])
            } else {
                0
            },
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct UartStatus {
    pub rx_available: u16,
    pub tx_space: u16,
    pub rx_overruns: u32,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CaptureStatus {
    pub sample_hz: u32,
    pub count: u32,
    pub elapsed_us: u32,
    /// First post-trigger sample; absent on immediate / older captures.
    pub trigger_index: Option<u32>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct CaptureResult {
    pub capture_id: u32,
    pub elapsed_us: u32,
    pub samples: Vec<u32>,
}

impl CaptureResult {
    pub fn actual_frequency(&self) -> Option<f64> {
        if self.elapsed_us == 0 {
            None
        } else {
            Some(self.samples.len() as f64 * 1_000_000.0 / self.elapsed_us as f64)
        }
    }
}

impl TryFrom<u8> for Command {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        let command = match value {
            1 => Self::GetInfo,
            2 => Self::GetStatus,
            3 => Self::PowerSet,
            4 => Self::ResetPulse,
            5 => Self::BootselSet,
            6 => Self::EnterBootsel,
            7 => Self::Measure,
            8 => Self::GpioRead,
            9 => Self::Capture,
            10 => Self::CaptureRead,
            11 => Self::GpioDiag,
            12 => Self::GpioPull,
            13 => Self::UartConfig,
            14 => Self::UartWrite,
            15 => Self::UartRead,
            16 => Self::UartStatus,
            17 => Self::SpiConfig,
            18 => Self::SpiTransfer,
            19 => Self::CaptureStatus,
            20 => Self::DebugEnterBootsel,
            21 => Self::CaptureStart,
            22 => Self::CapturePoll,
            23 => Self::CaptureCancel,
            _ => return Err(ProtocolError::UnknownCommand(value)),
        };
        Ok(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_and_crc() {
        let frame = Frame::request(Command::PowerSet, 0x1234_5678, &[1]);
        let encoded = frame.encode().unwrap();
        assert_eq!(Frame::decode(&encoded).unwrap(), frame);
        assert_eq!(crc32c(b"123456789"), 0xe306_9283);
    }

    #[test]
    fn frame_rejects_payload_corruption() {
        let frame = Frame::request(Command::GetInfo, 1, b"test");
        let mut encoded = frame.encode().unwrap();
        *encoded.last_mut().unwrap() ^= 1;
        assert_eq!(Frame::decode(&encoded), Err(ProtocolError::PayloadCrc));
    }

    #[test]
    fn dualcore_measurement_and_power_status_decode() {
        let mut measurement = vec![0];
        measurement.extend_from_slice(&(-10i32).to_le_bytes());
        measurement.extend_from_slice(&(5_000_000i32).to_le_bytes());
        measurement.extend_from_slice(&(-200i32).to_le_bytes());
        measurement.extend_from_slice(&(1_234_560i32).to_le_bytes());
        let decoded = Measurement::decode(&measurement).unwrap();
        assert_eq!(decoded.shunt_uv, -10.0);
        assert_eq!(decoded.bus_mv, 5000.0);
        assert_eq!(decoded.current_ma, -0.2);
        assert_eq!(decoded.power_mw, 1234.56);

        let status = PowerStatus::decode(&[0, 0, 1, 0, 1, 0xe4, 0x0c]).unwrap();
        assert!(!status.power_on);
        assert!(status.power_good);
        assert!(status.power_fault);
        assert_eq!(status.work_3v3_mv, 3300);
    }
}
