//! Length-delimited protobuf framing — the PH protocol envelope.
//!
//! Every PH message on the wire is `<varint length><encoded PhMessage>`, which
//! is exactly what `pb_encode_delimited` / prost's `encode_length_delimited`
//! produce. This module implements the splitter side (buffered stream -> whole
//! messages) plus the writer helper, so requests are framed identically to the
//! firmware side.

use prost::Message;
use thiserror::Error;

use crate::msg::PhMessage;
use crate::PH_MSG_MAX;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("message frame exceeds PH_MSG_MAX ({PH_MSG_MAX} bytes)")]
    TooLarge(usize),
    #[error("malformed length prefix (varint over 5 bytes)")]
    BadVarint,
    #[error("protobuf decode failed: {0}")]
    Decode(#[from] prost::DecodeError),
}

/// Serialize a message with its varint length prefix.
pub fn encode(msg: &PhMessage) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    prost::Message::encode_length_delimited(msg, &mut out).expect("vec io cannot fail");
    out
}

/// Incremental stream splitter for the PH envelope.
#[derive(Default)]
pub struct FrameBuffer {
    buf: Vec<u8>,
    start: usize,
}

impl FrameBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append received bytes.
    pub fn extend(&mut self, bytes: &[u8]) {
        if self.start > 0 && self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// Pop the next complete message from the buffer, if one is present.
    ///
    /// Returns `Ok(None)` when more bytes are required. A corrupt or oversized
    /// prefix yields an error (the sender should resync); the partial frame is
    /// then dropped so the stream can recover on the next boundary.
    pub fn try_next(&mut self) -> Result<Option<PhMessage>, FrameError> {
        let buf = &self.buf[self.start..];
        if buf.is_empty() {
            return Ok(None);
        }

        // Parse the leading varint length (little-endian base-128, max 5 bytes).
        let mut msg_len: u64 = 0;
        let mut vlen = 0usize;
        for (i, &b) in buf.iter().enumerate() {
            if i == 5 {
                return Err(FrameError::BadVarint);
            }
            msg_len |= ((b & 0x7f) as u64) << (7 * i);
            vlen = i + 1;
            if b & 0x80 == 0 {
                break;
            }
        }
        if msg_len == 0 || msg_len > PH_MSG_MAX as u64 {
            // Drop the corrupt/oversized prefix so the stream can resync.
            self.start += vlen;
            return Err(FrameError::TooLarge(msg_len as usize));
        }
        let total = vlen + msg_len as usize;
        if buf.len() < total {
            return Ok(None); // incomplete frame: need more bytes
        }

        let body = &self.buf[self.start + vlen..self.start + total];
        let msg = PhMessage::decode(body)?;
        self.start += total;
        Ok(Some(msg))
    }

    /// Drop buffered bytes already consumed by `try_next`.
    pub fn compact(&mut self) {
        if self.start > 0 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg::ph_message;
    use crate::msg::{GetStatus, Status};

    fn sample() -> PhMessage {
        PhMessage {
            seq: 7,
            status: Status::Ok as i32,
            body: Some(ph_message::Body::GetStatus(GetStatus {
                power_enabled: true,
                power_good: true,
                vbus_mv: 5050,
                ..Default::default()
            })),
        }
    }

    #[test]
    fn roundtrip_single_piece() {
        let bytes = encode(&sample());
        let mut fb = FrameBuffer::new();
        fb.extend(&bytes);
        let got = fb.try_next().unwrap().expect("one message");
        assert_eq!(got, sample());
        // consumed: the sample message is exactly 1 frame
        assert_eq!(fb.try_next().unwrap(), None);
    }

    #[test]
    fn roundtrip_split_anywhere() {
        let bytes = encode(&sample());
        let mut fb = FrameBuffer::new();
        for (i, &b) in bytes.iter().enumerate() {
            fb.extend(&[b]);
            if i == 0 {
                assert!(matches!(fb.try_next(), Ok(None)), "no frame yet");
            }
        }
        assert_eq!(fb.try_next().unwrap().unwrap(), sample());
    }

    #[test]
    fn two_frames_in_one_buffer() {
        let mut fb = FrameBuffer::new();
        let mut joined = encode(&sample());
        joined.extend(encode(&sample()));
        fb.extend(&joined);
        assert_eq!(fb.try_next().unwrap().unwrap(), sample());
        assert_eq!(fb.try_next().unwrap().unwrap(), sample());
        assert_eq!(fb.try_next().unwrap(), None);
    }

    #[test]
    fn oversized_length_is_rejected_and_resynced() {
        // A fake frame claiming 9000 bytes followed by a valid one.
        let bytes = encode(&sample());
        let mut fb = FrameBuffer::new();
        let fake = [0xc0, 0x46]; // varint 0x2340 = 9024 > 512
        fb.extend(&fake);
        fb.extend(&bytes);
        let err = fb.try_next().unwrap_err();
        assert!(matches!(err, FrameError::TooLarge(9024)));
        // After dropping the bad prefix the valid frame should decode.
        assert_eq!(fb.try_next().unwrap().unwrap(), sample());
    }

    #[test]
    fn zero_length_is_rejected() {
        let mut fb = FrameBuffer::new();
        fb.extend(&[0x00, 0x01]);
        assert!(matches!(fb.try_next(), Err(FrameError::TooLarge(0))));
    }
}