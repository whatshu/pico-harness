//! `PhClient` — PH protocol request/response over a byte transport.
//!
//! The transport is the CDC `PH Terminal` serial port in practice; tests inject
//! a scripted transport so the whole protocol layer is unit-testable without
//! hardware. Unsolicited events (ConsoleData, BulkTxDone, ...) are captured
//! while waiting for a response and drained by the caller.

use std::collections::VecDeque;
use std::time::{Duration, Instant};


use thiserror::Error;

use crate::framing::{self, FrameBuffer, FrameError};
use crate::msg::ph_message;
use crate::msg::*;

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(2000);

#[derive(Debug, Error)]
pub enum PhError {
    #[error("transport I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame error: {0}")]
    Frame(#[from] FrameError),
    #[error("protobuf decode failed: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("device answered with status {0:?}")]
    Status(Status),
    #[error("no reply within {0:?}")]
    Timeout(Duration),
    #[error("device connection lost")]
    Disconnected,
    #[error("serial port: {0}")]
    Serial(#[from] serialport::Error),
    #[error("USB error: {0}")]
    Usb(#[from] rusb::Error),
}

/// A byte pipe with blocking `send`/`recv` semantics (0 bytes = no data yet).
pub trait Transport {
    fn send(&mut self, data: &[u8]) -> Result<(), PhError>;
    /// Read up to `buf.len()` bytes; returns the count read, 0 when nothing is
    /// available yet (timeout), or an error on hard failure.
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, PhError>;
}

/// Transport over a real serial port (the CDC PH Terminal).
pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
}

impl SerialTransport {
    pub fn open(path: &str) -> Result<Self, PhError> {
        let port = serialport::new(path, 115_200)
            .timeout(Duration::from_millis(2))
            .open()?;
        Ok(Self { port })
    }
}

impl Transport for SerialTransport {
    fn send(&mut self, data: &[u8]) -> Result<(), PhError> {
        self.port.write(data)?;
        self.port.flush()?;
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, PhError> {
        match self.port.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(0),
            Err(e) => Err(PhError::Io(e)),
        }
    }
}

pub struct PhClient<T: Transport> {
    transport: T,
    fb: FrameBuffer,
    events: VecDeque<PhMessage>,
    seq: u32,
    timeout: Duration,
}

impl<T: Transport> PhClient<T> {
    pub fn new(transport: T) -> Self {
        Self::with_timeout(transport, DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(transport: T, timeout: Duration) -> Self {
        Self {
            transport,
            fb: FrameBuffer::new(),
            events: VecDeque::new(),
            seq: 0,
            timeout,
        }
    }

    pub fn next_seq(&mut self) -> u32 {
        self.seq = self.seq.wrapping_add(1);
        if self.seq == 0 {
            self.seq = 1;
        }
        self.seq
    }

    /// Send a request and wait for the matching reply. Unsolicited events that
    /// arrive while waiting are parked in `events`.
    pub fn request(&mut self, msg: &PhMessage) -> Result<PhMessage, PhError> {
        let seq = msg.seq;
        self.transport.send(&framing::encode(msg))?;
        let deadline = Instant::now() + self.timeout;

        loop {
            if let Some(frame) = self.fb.try_next()? {
                let is_event = matches!(
                    frame.body,
                    Some(ph_message::Body::ConsoleData(_))
                        | Some(ph_message::Body::BulkTxDone(_))
                        | Some(ph_message::Body::BulkRxAvail(_))
                );
                if frame.seq == seq && !is_event {
                    return Ok(frame);
                }
                if is_event || frame.seq == 0 {
                    self.events.push_back(frame);
                    continue;
                }
                return Err(PhError::Disconnected); // unexpected response
            }

            if Instant::now() >= deadline {
                return Err(PhError::Timeout(self.timeout));
            }
            let mut tmp = [0u8; 256];
            let n = self.transport.recv(&mut tmp)?;
            if n > 0 {
                self.fb.extend(&tmp[..n]);
            }
        }
    }

    /// Take unsolicited events collected during `request`.
    pub fn drain_events(&mut self) -> Vec<PhMessage> {
        self.events.drain(..).collect()
    }

    /// Read for up to `max_wait`, returning as soon as anything has been
    /// parsed. The console poll uses this so it pays no fixed latency when the
    /// Work core answers immediately.
    pub fn recv_events_short(&mut self, max_wait: Duration) -> Vec<PhMessage> {
        let deadline = Instant::now() + max_wait;
        loop {
            if !self.events.is_empty() || Instant::now() >= deadline {
                break;
            }
            if let Some(frame) = self.fb.try_next().unwrap_or(None) {
                self.events.push_back(frame);
                continue;
            }
            let mut tmp = [0u8; 256];
            if let Ok(n) = self.transport.recv(&mut tmp) {
                if n > 0 {
                    self.fb.extend(&tmp[..n]);
                }
            }
        }
        self.events.drain(..).collect()
    }

    /// Wait up to `duration` for unsolicited events (no request sent).
    /// Used by the console reader to watch for work output.
    pub fn recv_events(&mut self, duration: Duration) -> Vec<PhMessage> {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            if let Some(frame) = self.fb.try_next().unwrap_or(None) {
                self.events.push_back(frame);
                continue;
            }
            let mut tmp = [0u8; 256];
            if let Ok(n) = self.transport.recv(&mut tmp) {
                if n > 0 {
                    self.fb.extend(&tmp[..n]);
                }
            }
        }
        self.events.drain(..).collect()
    }

    fn expect_ok(&mut self, msg: &PhMessage) -> Result<(), PhError> {
        let resp = self.request(msg)?;
        if resp.status != Status::Ok as i32 {
            Err(self.status_err(resp))
        } else {
            Ok(())
        }
    }

    fn status_err(&self, resp: PhMessage) -> PhError {
        PhError::Status(Status::try_from(resp.status).unwrap_or(Status::Ok))
    }

    // ---- control --------------------------------------------------------

    pub fn get_info(&mut self) -> Result<GetInfo, PhError> {
        let req = PhMessage {
            seq: self.next_seq(),
            body: Some(ph_message::Body::GetInfo(GetInfo::default())),
            ..Default::default()
        };
        let resp = self.request(&req)?;
        match resp.body {
            Some(ph_message::Body::GetInfo(info)) => Ok(info),
            _ => Err(self.status_err(resp)),
        }
    }

    pub fn get_status(&mut self) -> Result<GetStatus, PhError> {
        let req = PhMessage {
            seq: self.next_seq(),
            body: Some(ph_message::Body::GetStatus(GetStatus::default())),
            ..Default::default()
        };
        let resp = self.request(&req)?;
        match resp.body {
            Some(ph_message::Body::GetStatus(st)) => Ok(st),
            _ => Err(self.status_err(resp)),
        }
    }

    pub fn power_set(&mut self, enabled: bool) -> Result<(), PhError> {
        let req = PhMessage {
            seq: self.next_seq(),
            body: Some(ph_message::Body::PowerSet(PowerSet { enabled })),
            ..Default::default()
        };
        self.expect_ok(&req)
    }

    pub fn measure(&mut self) -> Result<Measure, PhError> {
        let req = PhMessage {
            seq: self.next_seq(),
            body: Some(ph_message::Body::Measure(Measure::default())),
            ..Default::default()
        };
        let resp = self.request(&req)?;
        match resp.body {
            Some(ph_message::Body::Measure(m)) => Ok(m),
            _ => Err(self.status_err(resp)),
        }
    }

    pub fn reset_pulse(&mut self, duration_ms: u32) -> Result<(), PhError> {
        let req = PhMessage {
            seq: self.next_seq(),
            body: Some(ph_message::Body::ResetPulse(ResetPulse { duration_ms })),
            ..Default::default()
        };
        self.expect_ok(&req)
    }

    /// Console bridge status (mode/baud/fifo counters) as the Debug core sees
    /// the Work UART.
    pub fn term_status(&mut self) -> Result<TermStatus, PhError> {
        let req = PhMessage {
            seq: self.next_seq(),
            body: Some(ph_message::Body::TermStatus(TermStatus::default())),
            ..Default::default()
        };
        let resp = self.request(&req)?;
        match resp.body {
            Some(ph_message::Body::TermStatus(st)) => Ok(st),
            _ => Err(self.status_err(resp)),
        }
    }

    pub fn debug_bootsel(&mut self) -> Result<(), PhError> {
        let req = PhMessage {
            seq: self.next_seq(),
            body: Some(ph_message::Body::DebugBootsel(DebugBootsel::default())),
            ..Default::default()
        };
        self.expect_ok(&req)
    }

    // ---- terminal -------------------------------------------------------

    pub fn console_tx(&mut self, data: &[u8]) -> Result<(), PhError> {
        if data.len() > 128 {
            return Err(PhError::Status(Status::ErrArg));
        }
        let mut tx = ConsoleTx::default();
        tx.data.extend_from_slice(data);
        let req = PhMessage {
            seq: self.next_seq(),
            body: Some(ph_message::Body::ConsoleTx(tx)),
            ..Default::default()
        };
        self.expect_ok(&req)
    }

    pub fn term_config(
        &mut self,
        baud: u32,
        data_bits: u32,
        stop_bits: u32,
        parity: u32,
    ) -> Result<u32, PhError> {
        let req = PhMessage {
            seq: self.next_seq(),
            body: Some(ph_message::Body::TermConfig(TermConfig {
                baud,
                data_bits,
                stop_bits,
                parity,
                actual_baud: 0,
            })),
            ..Default::default()
        };
        let resp = self.request(&req)?;
        match resp.body {
            Some(ph_message::Body::TermConfig(tc)) => Ok(tc.actual_baud),
            _ => Err(self.status_err(resp)),
        }
    }

    // ---- bulk -----------------------------------------------------------

    pub fn bulk_tx_begin(&mut self, route: Route, length: u32, crc32c: u32) -> Result<(), PhError> {
        let req = PhMessage {
            seq: self.next_seq(),
            body: Some(ph_message::Body::BulkTxBegin(BulkTxBegin {
                route: route as i32,
                length,
                crc32c,
            })),
            ..Default::default()
        };
        self.expect_ok(&req)
    }
}

impl PhClient<SerialTransport> {
    /// Open the PH Terminal CDC serial and build a client against it.
    pub fn open_serial(path: &str) -> Result<Self, PhError> {
        let tr = SerialTransport::open(path)?;
        Ok(PhClient::new(tr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Transport whose `send` decodes the request and scripts a reply sequence
    /// per request, so tests never race against the client's synchronous wait.
    struct ScriptedTransport {
        recv: Vec<u8>,
        queue: Vec<Box<dyn FnMut(&PhMessage) -> Vec<PhMessage>>>,
        seq_seen: u32,
    }

    impl ScriptedTransport {
        fn script(mut self, handler: impl FnMut(&PhMessage) -> Vec<PhMessage> + 'static) -> Self {
            self.queue.push(Box::new(handler));
            self
        }
    }

    impl Transport for ScriptedTransport {
        fn send(&mut self, data: &[u8]) -> Result<(), PhError> {
            // Note: `data` is exactly one framed request.
            self.recv.clear();
            let mut fb = FrameBuffer::new();
            fb.extend(data);
            let req = fb.try_next()?.ok_or(PhError::Disconnected)?;
            self.seq_seen = req.seq;
            for msg in self.queue.first_mut().unwrap()(&req) {
                self.recv.extend(framing::encode(&msg));
            }
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8]) -> Result<usize, PhError> {
            let n = buf.len().min(self.recv.len());
            buf[..n].copy_from_slice(&self.recv[..n]);
            self.recv.drain(..n);
            Ok(n)
        }
    }

    fn reply(seq: u32, status: Status) -> PhMessage {
        PhMessage {
            seq,
            status: status as i32,
            body: None,
        }
    }

    #[test]
    fn get_info_roundtrip() {
        let tr = ScriptedTransport {
            recv: Vec::new(),
            queue: vec![Box::new(|req: &PhMessage| {
                let mut info = GetInfo::default();
                info.info = "PicoHarness test".into();
                vec![PhMessage {
                    seq: req.seq,
                    status: Status::Ok as i32,
                    body: Some(ph_message::Body::GetInfo(info)),
                }]
            })],
            seq_seen: 0,
        };
        let mut client = PhClient::new(tr);
        let info = client.get_info().unwrap();
        assert_eq!(info.info, "PicoHarness test");
    }

    #[test]
    fn control_handlers_send_the_expected_bodies() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let seen: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let seen2 = seen.clone();
        let tr = ScriptedTransport {
            recv: Vec::new(),
            queue: vec![Box::new(move |req| {
                seen2.borrow_mut().push(match &req.body {
                    Some(ph_message::Body::PowerSet(_)) => "power_set",
                    Some(ph_message::Body::GetStatus(_)) => "get_status",
                    Some(ph_message::Body::Measure(_)) => "measure",
                    Some(ph_message::Body::ConsoleTx(_)) => "console_tx",
                    Some(ph_message::Body::BulkTxBegin(_)) => "bulk_tx_begin",
                    _ => "other",
                });
                // Echo the request body so body-returning methods decode.
                vec![PhMessage {
                    seq: req.seq,
                    status: Status::Ok as i32,
                    body: req.body.clone(),
                }]
            })],
            seq_seen: 0,
        };
        let mut client = PhClient::new(tr);
        client.power_set(true).unwrap();
        client.get_status().unwrap();
        client.measure().unwrap();
        client.console_tx(b"hi").unwrap();
        client.bulk_tx_begin(Route::WorkSpi, 4, 0x1234_5678).unwrap();
        assert_eq!(&*seen.borrow(), &["power_set", "get_status", "measure", "console_tx", "bulk_tx_begin"]);
    }

    #[test]
    fn error_status_is_surfaced() {
        let tr = ScriptedTransport {
            recv: Vec::new(),
            queue: vec![Box::new(|req| vec![reply(req.seq, Status::ErrNotready)])],
            seq_seen: 0,
        };
        let mut client = PhClient::new(tr);
        match client.power_set(true) {
            Err(PhError::Status(Status::ErrNotready)) => {}
            other => panic!("expected ErrNotReady, got {other:?}"),
        }
    }

    #[test]
    fn unsolicited_event_is_captured_before_response() {
        let tr = ScriptedTransport {
            recv: Vec::new(),
            queue: vec![Box::new(|req| {
                let mut cd = ConsoleData::default();
                cd.data.extend_from_slice(b"hi");
                let event = PhMessage {
                    seq: 0,
                    body: Some(ph_message::Body::ConsoleData(cd)),
                    ..Default::default()
                };
                vec![event, reply(req.seq, Status::Ok)]
            })],
            seq_seen: 0,
        };
        let mut client = PhClient::new(tr);
        client.power_set(true).unwrap();
        let events = client.drain_events();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].body, Some(ph_message::Body::ConsoleData(_))));
    }

    #[test]
    fn recv_events_collects_unsolicited_output() {
        let mut tr = ScriptedTransport {
            recv: Vec::new(),
            queue: Vec::new(),
            seq_seen: 0,
        };
        let mut cd = ConsoleData::default();
        cd.data.extend_from_slice(b"hi");
        tr.recv.extend(framing::encode(&PhMessage {
            seq: 0,
            body: Some(ph_message::Body::ConsoleData(cd)),
            ..Default::default()
        }));
        let mut client = PhClient::new(tr);
        let events = client.recv_events(Duration::from_millis(50));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].body, Some(ph_message::Body::ConsoleData(_))));
    }

    #[test]
    fn request_timeout_without_reply() {
        let tr = ScriptedTransport {
            recv: Vec::new(),
            queue: vec![Box::new(|_| vec![])], // device never replies
            seq_seen: 0,
        };
        let mut client = PhClient::with_timeout(tr, Duration::from_millis(20));
        let req = PhMessage {
            seq: client.next_seq(),
            body: Some(ph_message::Body::GetInfo(GetInfo::default())),
            ..Default::default()
        };
        assert!(matches!(client.request(&req), Err(PhError::Timeout(_))));
    }
}