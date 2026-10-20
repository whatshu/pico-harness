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
    #[error("malformed length prefix (not a 32-bit varint)")]
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
        // A busy stream can always end in a partial frame. Reclaim consumed
        // bytes even then, rather than waiting for an empty buffer forever.
        self.compact();
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
        let mut msg_len = 0u32;
        let mut vlen = 0usize;
        for i in 0..5 {
            let Some(&b) = buf.get(i) else {
                // No terminating byte yet. In particular, 0x80 alone is the
                // beginning of length 128, not an invalid zero-length frame.
                return Ok(None);
            };
            if i == 4 && b & 0xf0 != 0 {
                self.start += i + 1;
                return Err(FrameError::BadVarint);
            }
            msg_len |= ((b & 0x7f) as u32) << (7 * i);
            vlen = i + 1;
            if b & 0x80 == 0 {
                break;
            }
        }
        if msg_len == 0 || msg_len > PH_MSG_MAX as u32 {
            // Drop the corrupt/oversized prefix so the stream can resync.
            self.start += vlen;
            return Err(FrameError::TooLarge(msg_len as usize));
        }
        let total = vlen + msg_len as usize;
        if buf.len() < total {
            return Ok(None); // incomplete frame: need more bytes
        }

        let body = &self.buf[self.start + vlen..self.start + total];
        let msg = PhMessage::decode(body);
        self.start += total;
        Ok(Some(msg?))
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

    #[test]
    fn split_multibyte_length_prefix_is_not_a_corrupt_frame() {
        let mut message = sample();
        message.body = Some(ph_message::Body::GetInfo(crate::msg::GetInfo {
            info: "x".repeat(121),
        }));
        assert_eq!(message.encoded_len(), 128);
        let bytes = encode(&message);
        assert_eq!(bytes[0], 0x80);
        let mut fb = FrameBuffer::new();
        for &byte in &bytes[..bytes.len() - 1] {
            fb.extend(&[byte]);
            assert_eq!(fb.try_next().unwrap(), None);
        }
        fb.extend(&bytes[bytes.len() - 1..]);
        assert_eq!(fb.try_next().unwrap(), Some(message));
    }

    #[test]
    fn malformed_prefix_and_body_do_not_poison_the_next_frame() {
        for corrupt in [vec![0x80; 5], vec![1, 0xff]] {
            let mut fb = FrameBuffer::new();
            fb.extend(&corrupt);
            fb.extend(&encode(&sample()));
            assert!(fb.try_next().is_err());
            assert_eq!(fb.try_next().unwrap(), Some(sample()));
        }
    }

    #[test]
    fn continuous_fragmented_stream_reclaims_consumed_storage() {
        let bytes = encode(&sample());
        let mut fb = FrameBuffer::new();
        fb.extend(&bytes[..1]);
        for _ in 0..10_000 {
            fb.extend(&bytes[1..]);
            fb.extend(&bytes[..1]);
            assert_eq!(fb.try_next().unwrap(), Some(sample()));
            assert_eq!(fb.try_next().unwrap(), None);
            assert!(fb.buf.len() <= bytes.len() + 1);
        }
    }
}
