//! Unified Work console: UART commands/short messages, SPI/DMA bulk messages.
use crate::live::Event;
use ph_app_link::PhBoardLink;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

pub const SHORT: usize = 128;
pub const BLOCK: usize = 2048;
const MAX_DATA: usize = 16 * 1024 * 1024;
const DEBUG_TX_CAPACITY: usize = 4096;
const WORK_TX_CAPACITY: usize = 1024;
const UART_FIFO_CAPACITY: usize = 32;
const TRANSPORT_MARGIN: Duration = Duration::from_millis(500);
// Work announces the protocol once per second. Leave room for its next poll.
const DISCOVERY_WINDOW: Duration = Duration::from_millis(1200);

pub trait Io {
    fn uart_config(&mut self, baud: u32) -> Result<u32, String>;
    /// Console bridge status from the Debug core: (mode, actual_baud) where
    /// mode 1 = pico_harness (framed), 0 = plain UART.
    fn term_status(&mut self) -> Result<(u8, u32), String>;
    // Submit once. Returning a partial count never implies a payload replay.
    fn uart_write(&mut self, data: &[u8]) -> Result<usize, String>;
    fn uart_read(&mut self) -> Result<Vec<u8>, String>;
    fn spi_config(&mut self) -> Result<(), String>;
    fn spi_transfer(&mut self, data: &[u8]) -> Result<Vec<u8>, String>;
}
impl Io for PhBoardLink {
    fn uart_config(&mut self, baud: u32) -> Result<u32, String> {
        self.uart_config(0, baud, 8, 1, 0)
            .map_err(|e| e.to_string())
    }
    fn term_status(&mut self) -> Result<(u8, u32), String> {
        let (mode, baud, _, _) = PhBoardLink::term_status(self).map_err(|e| e.to_string())?;
        Ok((mode as u8, baud))
    }
    fn uart_write(&mut self, data: &[u8]) -> Result<usize, String> {
        self.uart_write_some(0, data).map_err(|e| e.to_string())
    }
    fn uart_read(&mut self) -> Result<Vec<u8>, String> {
        self.uart_read(0, 4095).map_err(|e| e.to_string())
    }
    fn spi_config(&mut self) -> Result<(), String> {
        self.spi_config(1_000_000, 0)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    fn spi_transfer(&mut self, data: &[u8]) -> Result<Vec<u8>, String> {
        self.spi_transfer(data).map_err(|e| e.to_string())
    }
}

pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for byte in bytes {
        crc ^= (*byte as u16) << 8;
        for _ in 0..8 {
            crc = (crc << 1) ^ if crc & 0x8000 != 0 { 0x1021 } else { 0 };
        }
    }
    crc
}
#[derive(Debug, Clone, PartialEq)]
struct Frame {
    kind: u8,
    sequence: u16,
    data: Vec<u8>,
}
impl Frame {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![b'P', b'H', 1, self.kind];
        bytes.extend(self.sequence.to_le_bytes());
        bytes.extend((self.data.len() as u16).to_le_bytes());
        bytes.extend(&self.data);
        bytes.extend(crc16(&bytes[2..]).to_le_bytes());
        bytes
    }
}
#[derive(Default)]
struct Decoder {
    bytes: Vec<u8>,
}
impl Decoder {
    fn feed(&mut self, input: &[u8]) -> (Vec<Frame>, Vec<u8>) {
        self.bytes.extend(input);
        let mut frames = vec![];
        let mut plain = vec![];
        loop {
            if self.bytes.is_empty() {
                break;
            }
            if self.bytes[0] != b'P' || (self.bytes.len() >= 2 && self.bytes[1] != b'H') {
                plain.push(self.bytes.remove(0));
                continue;
            }
            if self.bytes.len() < 8 {
                break;
            }
            let size = u16::from_le_bytes([self.bytes[6], self.bytes[7]]) as usize;
            if self.bytes[2] != 1 || size > SHORT {
                plain.push(self.bytes.remove(0));
                continue;
            }
            if self.bytes.len() < size + 10 {
                break;
            }
            let expected = u16::from_le_bytes([self.bytes[size + 8], self.bytes[size + 9]]);
            if crc16(&self.bytes[2..size + 8]) != expected {
                plain.push(self.bytes.remove(0));
                continue;
            }
            frames.push(Frame {
                kind: self.bytes[3],
                sequence: u16::from_le_bytes([self.bytes[4], self.bytes[5]]),
                data: self.bytes[8..8 + size].to_vec(),
            });
            self.bytes.drain(..size + 10);
        }
        (frames, plain)
    }
}

/// Trace decoding keeps UART fragments across USB reads, separately per direction.
/// SPI carries bulk payload only; its command and CRC confirmation travel on UART.
#[derive(Default)]
pub struct TraceDecoder {
    uart: Decoder,
}
impl TraceDecoder {
    pub fn describe(&mut self, bus: &str, bytes: &[u8]) -> String {
        if bus == "SPI" {
            return format!("SPI bulk payload · {} B", bytes.len());
        }
        let (frames, plain) = self.uart.feed(bytes);
        let mut descriptions: Vec<String> = frames.iter().map(describe_frame).collect();
        if !plain.is_empty() {
            descriptions.push(format!(
                "UART console · {} B · {}",
                plain.len(),
                preview(&plain)
            ));
        }
        if !self.uart.bytes.is_empty() {
            descriptions.push(format!(
                "UART frame fragment · {} B buffered",
                self.uart.bytes.len()
            ));
        }
        descriptions.join("\n")
    }
}
fn preview(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(48)]);
    format!(
        "\"{}\"{}",
        text.escape_debug(),
        if bytes.len() > 48 { "…" } else { "" }
    )
}
fn describe_frame(frame: &Frame) -> String {
    let name = match frame.kind {
        0x01 => "Hello / capabilities request",
        0x81 => "Hello / capabilities reply",
        0x02 => "Console TX",
        0x82 => "Console TX ACK",
        0x03 => "Baud prepare",
        0x83 => "Baud prepare ACK",
        0x04 => "Verify baud",
        0x84 => "Verify baud reply",
        0x05 => "Commit baud",
        0x85 => "Commit baud ACK",
        0x06 => "Bulk TX prepare",
        0x86 => "Bulk TX ready",
        0x87 => "Bulk transfer complete",
        0x08 => "Bulk RX prepare",
        0x88 => "Bulk RX ready",
        0x90 => "Console RX message",
        0x91 => "Bulk RX available",
        _ => "Unknown command",
    };
    let parameter = match (frame.kind, frame.data.as_slice()) {
        (0x03 | 0x83 | 0x84 | 0x85, [a, b, c, d]) => {
            format!(" · {} baud", u32::from_le_bytes([*a, *b, *c, *d]))
        }
        (0x06 | 0x86 | 0x88 | 0x91, [a, b]) => {
            format!(" · bulk length={} B", u16::from_le_bytes([*a, *b]))
        }
        (0x87, [a, b]) => format!(" · data CRC=0x{:04X}", u16::from_le_bytes([*a, *b])),
        (0x81, [version, capabilities, a, b]) => format!(
            " · Work v{version}, UART={}, SPI={}, baud={}, block={} B",
            capabilities & 1 != 0,
            capabilities & 2 != 0,
            capabilities & 4 != 0,
            u16::from_le_bytes([*a, *b])
        ),
        (0x02 | 0x90, data) => format!(" · {}", preview(data)),
        _ => String::new(),
    };
    format!(
        "{name} (0x{:02X}) · PH v1 · seq={} · payload={} B{parameter} · CRC OK",
        frame.kind,
        frame.sequence,
        frame.data.len()
    )
}

pub struct Link {
    events: Sender<Event>,
    decoder: Decoder,
    pending: VecDeque<Frame>,
    framed: bool,
    negotiated: bool,
    baud: u32,
    sequence: u16,
    last_rx: Instant,
    bulk_ready: bool,
    discovery_started: Option<Instant>,
    /// Last time the console state was read from the device.
    status_polled_at: Option<Instant>,
    discovery_complete: bool,
    cancel: Option<Arc<AtomicBool>>,
}
impl Link {
    pub fn new(events: Sender<Event>, baud: u32, framed: bool) -> Self {
        let link = Self {
            events,
            decoder: Decoder::default(),
            pending: VecDeque::new(),
            framed,
            negotiated: false,
            baud,
            sequence: 0,
            last_rx: Instant::now(),
            bulk_ready: false,
            discovery_started: None,
            status_polled_at: None,
            discovery_complete: framed,
            cancel: None,
        };
        link.state();
        link
    }
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }
    pub fn check_cancelled(&self) -> Result<(), String> {
        if self
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
        {
            Err("Operation canceled".into())
        } else {
            Ok(())
        }
    }
    fn trace(&self, bus: &'static str, direction: &'static str, bytes: &[u8]) {
        let _ = self.events.send(Event::Trace {
            bus,
            direction,
            bytes: bytes.to_vec(),
        });
    }
    fn state(&self) {
        let _ = self.events.send(Event::WorkSupport {
            verified: self.negotiated,
            bulk_ready: self.bulk_ready,
        });
        let _ = self.events.send(Event::ConsoleState {
            framed: self.framed,
            baud: self.baud,
        });
    }
    pub fn reset(&mut self, io: &mut impl Io) -> Result<(), String> {
        self.framed = false;
        self.negotiated = false;
        self.bulk_ready = false;
        self.discovery_started = None;
        self.discovery_complete = false;
        self.decoder.bytes.clear();
        self.pending.clear();
        // The Debug core owns the Work protocol; ask it for the console state
        // instead of probing Work ourselves.
        match io.term_status() {
            Ok((mode, baud)) => {
                self.framed = mode == 1;
                self.negotiated = mode == 1;
                self.baud = if baud != 0 { baud } else { io.uart_config(115200)? };
            }
            Err(_) => self.baud = io.uart_config(115200)?,
        }
        self.state();
        Ok(())
    }
    pub fn poll(&mut self, io: &mut impl Io) -> Result<(), String> {
        let discovery_started = *self.discovery_started.get_or_insert_with(Instant::now);
        // The console state changes rarely; query it at most every 500 ms so
        // the 50 ms poll loop stays cheap for the device.
        let due = self
            .status_polled_at
            .is_none_or(|t| t.elapsed() >= Duration::from_millis(500));
        if due {
            self.status_polled_at = Some(Instant::now());
        }
        if due {
            if let Ok((mode, baud)) = io.term_status() {
            let changed = self.framed != (mode == 1) || self.negotiated != (mode == 1);
            self.framed = mode == 1;
            self.negotiated = mode == 1;
            if baud != 0 {
                self.baud = baud;
            }
            if changed {
                self.state();
            }
            }
        }
        let bytes = io.uart_read()?;
        if !bytes.is_empty() {
            self.last_rx = Instant::now();
            self.trace("UART", "RX", &bytes);
            // The Debug core hands Work's console output to us as plain bytes:
            // pico_harness framing lives on the Debug core now, so there is
            // nothing to peel here - surface the text directly.
            let _ = self.events.send(Event::UartRx(bytes));
        }
        if self.framed || discovery_started.elapsed() >= DISCOVERY_WINDOW {
            self.discovery_complete = true;
        }
        Ok(())
    }
    pub fn poll_console(&mut self, io: &mut impl Io) -> Result<(), String> {
        self.check_cancelled()?;
        self.poll(io)?;
        if self.bulk_ready {
            let received = self.receive(io, BLOCK)?;
            if !received.is_empty() {
                let _ = self.events.send(Event::UartRx(received));
            }
        }
        Ok(())
    }
    fn write(&self, io: &mut impl Io, bytes: &[u8]) -> Result<(), String> {
        let started = Instant::now();
        let timeout = self.write_budget(bytes.len());
        let mut written = 0;
        while written < bytes.len() {
            self.check_cancelled()?;
            if started.elapsed() >= timeout {
                return Err("Debug UART transmit queue did not accept the remaining data".into());
            }
            let remaining = &bytes[written..];
            let chunk = &remaining[..remaining.len().min(SHORT)];
            let accepted = io.uart_write(chunk)?;
            if accepted > remaining.len() {
                return Err("Debug UART accepted more bytes than submitted".into());
            }
            if accepted == 0 {
                thread::sleep(Duration::from_millis(2));
            } else {
                self.trace("UART", "TX", &remaining[..accepted]);
                written += accepted;
            }
        }
        Ok(())
    }
    fn wire_time(&self, bytes: usize) -> Duration {
        // UART is always 8N1: one start bit, eight data bits and one stop bit.
        Duration::from_nanos((bytes as u64 * 10_000_000_000).div_ceil(self.baud.max(1) as u64))
    }
    fn write_budget(&self, bytes: usize) -> Duration {
        (self.wire_time(DEBUG_TX_CAPACITY + UART_FIFO_CAPACITY + bytes) + TRANSPORT_MARGIN)
            .max(Duration::from_secs(3))
    }
    fn acknowledgment_budget(&self, kind: u8, request_bytes: usize) -> Duration {
        let response_bytes = match kind {
            1 | 3 | 4 | 5 => 14,
            2 => 10,
            6 | 8 | 0x87 => 12,
            _ => SHORT + 10,
        };
        // Baud verification and recovery run with application writes paused and
        // a freshly configured Debug UART. Keep wrong-rate probes bounded.
        let queued_bytes = if (3..=5).contains(&kind) {
            0
        } else {
            DEBUG_TX_CAPACITY + WORK_TX_CAPACITY + 2 * UART_FIFO_CAPACITY
        };
        (self.wire_time(queued_bytes + request_bytes + response_bytes) + TRANSPORT_MARGIN)
            .max(Duration::from_secs(1))
    }
    fn baud_rollback_wait(baud: u32) -> Duration {
        // Match Work's lease: allow 64 handshake bytes at the actual new
        // rate, with a 1.5 s floor, then leave another 100 ms for its poll.
        let lease =
            Duration::from_micros(640_000_000u64.div_ceil(baud.max(300) as u64)) + TRANSPORT_MARGIN;
        lease.max(Duration::from_millis(1500)) + Duration::from_millis(100)
    }
    fn wait(
        &mut self,
        io: &mut impl Io,
        kind: u8,
        sequence: u16,
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        let started = Instant::now();
        loop {
            self.check_cancelled()?;
            if let Some(index) = self
                .pending
                .iter()
                .position(|frame| frame.kind == kind && frame.sequence == sequence)
            {
                return Ok(self.pending.remove(index).unwrap().data);
            }
            if started.elapsed() >= timeout {
                return Err(format!("Work did not acknowledge command 0x{kind:02x}"));
            }
            self.poll(io)?;
            thread::sleep(Duration::from_millis(2));
        }
    }
    fn request(
        &mut self,
        io: &mut impl Io,
        kind: u8,
        data: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        self.check_cancelled()?;
        self.sequence = self.sequence.wrapping_add(1).max(1);
        let sequence = self.sequence;
        self.write(
            io,
            &Frame {
                kind,
                sequence,
                data: data.to_vec(),
            }
            .encode(),
        )?;
        let timeout = self.acknowledgment_budget(kind, data.len() + 10);
        Ok((sequence, self.wait(io, kind | 0x80, sequence, timeout)?))
    }
    fn discover_console(&mut self, io: &mut impl Io) -> Result<(), String> {
        self.check_cancelled()?;
        let started = Instant::now();
        while !self.framed && !self.discovery_complete && started.elapsed() < DISCOVERY_WINDOW {
            self.check_cancelled()?;
            if let Ok((mode, baud)) = io.term_status() {
                self.framed = mode == 1;
                if baud != 0 {
                    self.baud = baud;
                }
                self.state();
            }
            self.poll(io)?; // keep draining console output while waiting
            if !self.framed {
                thread::sleep(Duration::from_millis(5));
            }
        }
        self.discovery_complete = true;
        self.check_cancelled()
    }
    fn ensure_protocol(&mut self, io: &mut impl Io) -> Result<(), String> {
        self.check_cancelled()?;
        if !self.negotiated {
            let (mode, baud) = io.term_status()?;
            if mode != 1 {
                return Err(
                    "Work console is a plain UART; flash a pico_harness firmware (harness-target)"
                        .into(),
                );
            }
            self.framed = true;
            self.negotiated = true;
            if baud != 0 {
                self.baud = baud;
            }
            self.state();
        }
        Ok(())
    }
    pub fn set_baud(&mut self, io: &mut impl Io, baud: u32) -> Result<(), String> {
        if !(300..=2_000_000).contains(&baud) {
            return Err("Baud must be 300..2000000".into());
        }
        // The Debug core performs the pico_harness prepare/verify/commit
        // transaction; the host just asks for the rate.
        let applied = io.uart_config(baud)?;
        if applied.abs_diff(baud) as f64 > baud as f64 * 0.02 {
            return Err(format!("Work applied {applied} Hz, not {baud} Hz"));
        }
        self.baud = applied;
        self.state();
        return Ok(());
        #[allow(unreachable_code)]
        let previous = self.baud;
        let mut switched_baud = baud;
        let result: Result<(), String> = (|| {
            let (_, accepted) = self.request(io, 3, &baud.to_le_bytes())?;
            if accepted != baud.to_le_bytes() {
                return Err("Work rejected the requested baud".into());
            }
            thread::sleep(Duration::from_millis(120));
            self.baud = io.uart_config(baud)?;
            switched_baud = self.baud;
            let (_, response) = self.request(io, 4, &[])?;
            let work =
                u32::from_le_bytes(response.try_into().map_err(|_| "Invalid Work baud reply")?);
            if work.abs_diff(self.baud) as f64 > self.baud as f64 * 0.02 {
                return Err("Work and Debug baud rates differ by more than 2%".into());
            }
            switched_baud = work;
            self.request(io, 5, &[])?;
            self.check_cancelled()?;
            Ok(())
        })();
        if let Err(error) = result {
            // Work may already have switched or committed. Restore and publish
            // the actual endpoint even when the session is being closed.
            let cancel = self.cancel.take();
            thread::sleep(Self::baud_rollback_wait(switched_baud));
            self.decoder.bytes.clear();
            self.pending.clear();
            let restored = (|| -> Result<(), String> {
                self.baud = io.uart_config(previous)?;
                if self.request(io, 4, &[]).is_ok() {
                    return Ok(());
                }
                // A lost commit ACK can leave Work at the new rate. Reach it there
                // and perform a coordinated switch back rather than assuming rollback.
                self.baud = io.uart_config(baud)?;
                self.request(io, 4, &[])?;
                self.request(io, 3, &previous.to_le_bytes())?;
                thread::sleep(Duration::from_millis(120));
                self.baud = io.uart_config(previous)?;
                self.request(io, 4, &[])?;
                self.request(io, 5, &[])?;
                Ok(())
            })()
            .map_err(|error| {
                // An unanswered old-rate probe does not prove Work switched.
                // Leave Debug at the original rate if neither endpoint can be
                // verified, while retaining the unverified-restoration error.
                match io.uart_config(previous) {
                    Ok(actual) => {
                        self.baud = actual;
                        error
                    }
                    Err(endpoint_error) => {
                        format!("{error}; Debug endpoint restoration also failed: {endpoint_error}")
                    }
                }
            });
            self.cancel = cancel;
            self.state();
            if let Err(restore_error) = restored {
                return Err(format!("Baud change failed: {error}; restoration could not be verified: {restore_error}"));
            }
            return Err(format!(
                "Baud change failed and reverted to {previous}: {error}"
            ));
        }
        self.state();
        Ok(())
    }
    fn transfer(&self, io: &mut impl Io, bytes: &[u8]) -> Result<Vec<u8>, String> {
        self.check_cancelled()?;
        self.trace("SPI", "TX", bytes);
        let received = io.spi_transfer(bytes)?;
        self.trace("SPI", "RX", &received);
        if received.len() != bytes.len() {
            return Err("Incomplete SPI block".into());
        }
        Ok(received)
    }
    pub fn receive(&mut self, io: &mut impl Io, maximum: usize) -> Result<Vec<u8>, String> {
        self.check_cancelled()?;
        self.ensure_protocol(io)?;
        self.bulk_ready = false;
        self.state();
        let (sequence, response) = self.request(io, 8, &[])?;
        let length = u16::from_le_bytes(response.try_into().map_err(|_| "Invalid bulk size reply")?)
            as usize;
        if length == 0 {
            return Ok(vec![]);
        }
        if length > BLOCK || length > maximum {
            return Err("Work bulk response exceeds the receive limit".into());
        }
        io.spi_config()?;
        let bytes = self.transfer(io, &vec![0; length])?;
        let timeout = self.acknowledgment_budget(0x87, 0);
        let expected = self.wait(io, 0x87, sequence, timeout)?;
        if expected != crc16(&bytes).to_le_bytes() {
            return Err("SPI receive checksum mismatch".into());
        }
        Ok(bytes)
    }
    pub fn send(
        &mut self,
        io: &mut impl Io,
        bytes: &[u8],
        mut progress: impl FnMut(usize, usize),
    ) -> Result<(), String> {
        self.check_cancelled()?;
        if bytes.is_empty() || bytes.len() > MAX_DATA {
            return Err("Send 1 byte to 16 MiB of Work data".into());
        }
        if bytes.len() <= SHORT {
            // Console text is carried raw in PH `ConsoleTx`; the Debug core
            // frames it for Work when the harness protocol is active.
            self.discover_console(io)?;
            self.write(io, bytes)?;
            progress(bytes.len(), bytes.len());
        } else {
            self.ensure_protocol(io)?;
            io.spi_config()?;
            let mut sent = 0;
            for block in bytes.chunks(BLOCK) {
                self.check_cancelled()?;
                let (sequence, accepted) =
                    self.request(io, 6, &(block.len() as u16).to_le_bytes())?;
                if accepted != (block.len() as u16).to_le_bytes() {
                    return Err("Work rejected bulk length".into());
                }
                self.transfer(io, block)?;
                let timeout = self.acknowledgment_budget(0x87, 0);
                let response = self.wait(io, 0x87, sequence, timeout)?;
                if response != crc16(block).to_le_bytes() {
                    return Err("Work bulk checksum mismatch".into());
                }
                // Give a Work application time to publish a response after its callback.
                thread::sleep(Duration::from_millis(2));
                let received = self.receive(io, BLOCK)?;
                if !received.is_empty() {
                    let _ = self.events.send(Event::UartRx(received));
                }
                sent += block.len();
                progress(sent, bytes.len());
            }
        }
        self.check_cancelled()?;
        let _ = self.events.send(Event::UartTx(bytes.to_vec()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ConsoleIo {
        term_mode: u8,
        applied_baud: Option<u32>,
        hello_after: Option<usize>,
        reads: usize,
        writes: Vec<Vec<u8>>,
        replies: VecDeque<Vec<u8>>,
        bulk_sequence: u16,
        blocks: Vec<Vec<u8>>,
        cancel_on_read: Option<Arc<AtomicBool>>,
        cancel_on_write: Option<Arc<AtomicBool>>,
        receive_delay: usize,
        write_limits: VecDeque<usize>,
        tx_decoder: Decoder,
    }
    impl Default for ConsoleIo {
        fn default() -> Self {
            Self {
                term_mode: 1, // Emulates a pico_harness Work console.
                applied_baud: None,
                hello_after: None,
                reads: 0,
                writes: Vec::new(),
                replies: VecDeque::new(),
                bulk_sequence: 0,
                blocks: Vec::new(),
                cancel_on_read: None,
                cancel_on_write: None,
                receive_delay: 0,
                write_limits: VecDeque::new(),
                tx_decoder: Decoder::default(),
            }
        }
    }
    impl ConsoleIo {
        fn reply(&mut self, kind: u8, sequence: u16, data: &[u8]) {
            self.replies.push_back(
                Frame {
                    kind,
                    sequence,
                    data: data.to_vec(),
                }
                .encode(),
            );
        }
    }
    impl Io for ConsoleIo {
        fn uart_config(&mut self, baud: u32) -> Result<u32, String> {
            Ok(self.applied_baud.unwrap_or(baud))
        }
        fn term_status(&mut self) -> Result<(u8, u32), String> {
            Ok((self.term_mode, self.applied_baud.unwrap_or(115_200)))
        }
        fn uart_write(&mut self, data: &[u8]) -> Result<usize, String> {
            let accepted = self
                .write_limits
                .pop_front()
                .unwrap_or(data.len())
                .min(data.len());
            self.writes.push(data[..accepted].to_vec());
            if let Some(cancel) = self.cancel_on_write.take() {
                cancel.store(true, Ordering::Relaxed);
            }
            let (frames, _) = self.tx_decoder.feed(&data[..accepted]);
            for frame in frames {
                let response = match frame.kind {
                    1 => vec![1, 7, 0, 8],
                    2 => vec![],
                    6 => {
                        self.bulk_sequence = frame.sequence;
                        frame.data
                    }
                    8 => vec![0, 0],
                    _ => panic!("Unexpected request kind {}", frame.kind),
                };
                self.reply(frame.kind | 0x80, frame.sequence, &response);
            }
            Ok(accepted)
        }
        fn uart_read(&mut self) -> Result<Vec<u8>, String> {
            self.reads += 1;
            if let Some(cancel) = self.cancel_on_read.take() {
                cancel.store(true, Ordering::Relaxed);
            }
            if self.hello_after.is_some_and(|after| self.reads >= after) {
                self.hello_after = None;
                self.reply(0x81, 0, &[1, 7, 0, 8]);
            }
            if !self.replies.is_empty() && self.receive_delay > 0 {
                self.receive_delay -= 1;
                return Ok(vec![]);
            }
            Ok(self.replies.pop_front().unwrap_or_default())
        }
        fn spi_config(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn spi_transfer(&mut self, data: &[u8]) -> Result<Vec<u8>, String> {
            self.blocks.push(data.to_vec());
            self.reply(0x87, self.bulk_sequence, &crc16(data).to_le_bytes());
            Ok(vec![0; data.len()])
        }
    }

    #[test]
    fn cancellation_interrupts_protocol_discovery_and_acknowledgment_waits() {
        let (tx, _) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut link = Link::new(tx.clone(), 115200, false).with_cancel(cancel.clone());
        let mut io = ConsoleIo {
            cancel_on_read: Some(cancel.clone()),
            ..Default::default()
        };
        assert_eq!(
            link.send(&mut io, b"unsent", |_, _| {}).unwrap_err(),
            "Operation canceled"
        );
        assert!(io.writes.is_empty());
        assert_eq!(io.reads, 1);

        cancel.store(false, Ordering::Relaxed);
        let mut link = Link::new(tx, 115200, true).with_cancel(cancel.clone());
        let mut io = ConsoleIo {
            cancel_on_write: Some(cancel),
            ..Default::default()
        };
        assert_eq!(
            link.send(&mut io, b"awaiting-ack", |_, _| {}).unwrap_err(),
            "Operation canceled"
        );
        assert_eq!(io.writes.len(), 1);
        assert_eq!(io.reads, 0);
    }

    #[test]
    fn low_baud_deadlines_cover_the_wire_time_and_queued_console_output() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut link = Link::new(tx, 300, true);
        let payload = [0xa7; SHORT];
        let frame_bytes = payload.len() + 10;
        let write_budget = link.write_budget(frame_bytes);
        let acknowledgment_budget = link.acknowledgment_budget(2, frame_bytes);
        // At 300 baud the complete short frame alone needs 4.6 seconds.
        assert!(write_budget > Duration::from_millis(4600));
        assert!(
            write_budget >= link.wire_time(DEBUG_TX_CAPACITY + frame_bytes),
            "A full Debug queue must have time to drain before accepting the frame"
        );
        // An application may echo the request before its ACK, behind old output.
        let work_reply_time = link.wire_time(WORK_TX_CAPACITY + frame_bytes + 10);
        assert!(acknowledgment_budget >= link.wire_time(frame_bytes) + work_reply_time);
        assert!(link.acknowledgment_budget(4, 10) < Duration::from_millis(1500));
        assert_eq!(
            Link::baud_rollback_wait(300),
            Duration::from_micros(2_733_334)
        );
        assert_eq!(
            Link::baud_rollback_wait(115200),
            Duration::from_millis(1600)
        );

        let mut io = ConsoleIo {
            receive_delay: 5,
            ..Default::default()
        };
        link.send(&mut io, &payload, |_, _| {}).unwrap();
        // Console text is a single raw PH `ConsoleTx`, no Work ACK to await.
        assert_eq!(io.writes.concat(), payload);
        assert!(rx
            .try_iter()
            .any(|event| matches!(event, Event::UartTx(bytes) if bytes == payload)));

        // The policy gives slow hardware time, without delaying successful calls.
        link.baud = 115200;
        assert_eq!(
            link.acknowledgment_budget(2, frame_bytes),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn partial_uart_acceptance_retries_only_the_unaccepted_suffix() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut link = Link::new(tx, 300, true);
        let mut io = ConsoleIo {
            write_limits: [0, 2, 0, 4, 1].into(),
            ..Default::default()
        };
        link.send(&mut io, b"partial-console-command", |_, _| {})
            .unwrap();
        // The raw console payload is retried suffix-only until fully accepted.
        let expected = b"partial-console-command".to_vec();
        assert_eq!(io.writes.concat(), expected);
        let traced: Vec<_> = rx
            .try_iter()
            .filter_map(|event| match event {
                Event::Trace {
                    bus: "UART",
                    direction: "TX",
                    bytes,
                } => Some(bytes),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(
            traced, expected,
            "Trace records accepted bytes, including partial writes"
        );
    }

    #[test]
    fn cancellation_stops_a_partly_accepted_uart_frame_without_replay() {
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut link = Link::new(tx, 300, true).with_cancel(cancel.clone());
        let mut io = ConsoleIo {
            write_limits: [5].into(),
            cancel_on_write: Some(cancel),
            ..Default::default()
        };
        assert_eq!(
            link.send(&mut io, b"canceled", |_, _| {}).unwrap_err(),
            "Operation canceled"
        );
        assert_eq!(io.writes.len(), 1);
        assert_eq!(io.writes[0].len(), 5);
        assert!(!rx.try_iter().any(|event| matches!(event, Event::UartTx(_))));
    }

    #[test]
    fn frames_resynchronize_after_bad_crc_and_survive_every_split() {
        let frame = Frame {
            kind: 0x90,
            sequence: 12,
            data: b"hello".to_vec(),
        };
        let encoded = frame.encode();
        for split in 0..=encoded.len() {
            let mut decoder = Decoder::default();
            let mut all = decoder.feed(&encoded[..split]).0;
            all.extend(decoder.feed(&encoded[split..]).0);
            assert_eq!(all, vec![frame.clone()]);
        }
        let mut damaged = encoded.clone();
        damaged[8] ^= 1;
        damaged.extend(&encoded);
        assert_eq!(Decoder::default().feed(&damaged).0, vec![frame]);
    }

    #[test]
    fn transport_trace_explains_handshake_baud_and_split_frames() {
        let hello = [0x50, 0x48, 1, 1, 1, 0, 0, 0, 0x55, 0x97];
        let decoded = TraceDecoder::default().describe("UART", &hello);
        assert!(
            decoded.contains("Hello / capabilities request")
                && decoded.contains("seq=1")
                && decoded.contains("CRC OK"),
            "{decoded}"
        );
        let baud = Frame {
            kind: 3,
            sequence: 42,
            data: 921600u32.to_le_bytes().to_vec(),
        }
        .encode();
        for split in 1..baud.len() {
            let mut decoder = TraceDecoder::default();
            decoder.describe("UART", &baud[..split]);
            let decoded = decoder.describe("UART", &baud[split..]);
            assert!(
                decoded.contains("Baud prepare") && decoded.contains("921600 baud"),
                "{decoded}"
            );
        }
        let mut damaged = hello;
        damaged[9] ^= 1;
        assert!(!TraceDecoder::default()
            .describe("UART", &damaged)
            .contains("CRC OK"));
        assert_eq!(
            TraceDecoder::default().describe("SPI", &[0x12; 128]),
            "SPI bulk payload · 128 B"
        );
    }

    #[test]
    #[ignore = "Requires the harness-signals Work firmware and a connected PicoHarness probe"]
    fn connected_work_console_bulk_and_baud() {
        let serial = std::env::var("PDBG_PROBE_SERIAL").expect("PDBG_PROBE_SERIAL");
        let mut io = PhBoardLink::open_serial(serial).unwrap();
        io.uart_config(0, 115200, 8, 1, 0).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut work = Link::new(tx, 115200, false);
        work.ensure_protocol(&mut io).unwrap();
        work.send(&mut io, b"console-test", |_, _| {}).unwrap();
        let payload = vec![0xa7; BLOCK * 2 + 123];
        work.send(&mut io, &payload, |_, _| {}).unwrap();
        let mut echoed = vec![];
        let mut uart_trace = false;
        let mut spi_trace = false;
        let mut short_echo = false;
        for event in rx.try_iter() {
            match event {
                Event::UartRx(bytes) if bytes.iter().all(|byte| *byte == 0xa7) => {
                    echoed.extend(bytes)
                }
                Event::UartRx(bytes) if bytes == b"console-test" => short_echo = true,
                Event::Trace { bus: "UART", .. } => uart_trace = true,
                Event::Trace { bus: "SPI", .. } => spi_trace = true,
                _ => {}
            }
        }
        assert_eq!(echoed, payload);
        assert!(short_echo && uart_trace && spi_trace);
        work.set_baud(&mut io, 921600).unwrap();
        assert!(work.baud.abs_diff(921600) < 20000);
        work.send(&mut io, b"new-baud", |_, _| {}).unwrap();
        work.set_baud(&mut io, 115200).unwrap();
        struct FailConfig(PhBoardLink);
        impl Io for FailConfig {
            fn term_status(&mut self) -> Result<(u8, u32), String> {
                Ok((1, 115_200))
            }
            fn uart_config(&mut self, baud: u32) -> Result<u32, String> {
                if baud > 200000 {
                    Err("Injected Debug UART configuration failure".into())
                } else {
                    Io::uart_config(&mut self.0, baud)
                }
            }
            fn uart_write(&mut self, data: &[u8]) -> Result<usize, String> {
                Io::uart_write(&mut self.0, data)
            }
            fn uart_read(&mut self) -> Result<Vec<u8>, String> {
                Io::uart_read(&mut self.0)
            }
            fn spi_config(&mut self) -> Result<(), String> {
                Io::spi_config(&mut self.0)
            }
            fn spi_transfer(&mut self, data: &[u8]) -> Result<Vec<u8>, String> {
                Io::spi_transfer(&mut self.0, data)
            }
        }
        let mut failed = FailConfig(io);
        assert!(work
            .set_baud(&mut failed, 921600)
            .unwrap_err()
            .contains("reverted"));
        work.request(&mut failed, 4, &[]).unwrap();
        work.send(&mut failed, b"rollback-ok", |_, _| {}).unwrap();
        // Keep UART active throughout this finite capture. A 20 ms periodic
        // source can leave a valid 16 ms capture with only a control reply.
        work.send(&mut failed, b"stream-on", |_, _| {}).unwrap();
        let id = failed
            .0
            .start_capture_triggered(32768, 2_000_000, 2, 12, 1000)
            .unwrap();
        let started = Instant::now();
        let (rate, count, elapsed_us) = loop {
            if let Some(result) = failed.0.poll_capture(id).unwrap() {
                break result;
            }
            assert!(started.elapsed() < Duration::from_secs(3));
            thread::sleep(Duration::from_millis(2));
        };
        let capture = failed.0.read_capture(id, count, elapsed_us).unwrap();
        work.send(&mut failed, b"stream-off", |_, _| {}).unwrap();
        let mut logic = crate::logic::Logic::default();
        for decoder in crate::logic::Decoder::catalog() {
            logic.add_decoder(decoder);
        }
        logic.set_capture(capture.samples, rate);
        let uart = logic
            .annotations
            .iter()
            .filter(|annotation| annotation.protocol == "UART")
            .count();
        let pwm = logic
            .annotations
            .iter()
            .filter(|annotation| annotation.protocol == "PWM")
            .count();
        assert!(uart > 10 && pwm > 100, "UART={uart}, PWM={pwm}");
        assert!(logic
            .annotations
            .iter()
            .any(|annotation| annotation.protocol == "UART"
                && annotation.text == "0x50 'P'"
                && !annotation.error));
        std::fs::write("/tmp/picoharness-uart-pwm.vcd", logic.vcd()).unwrap();
        let id = failed
            .0
            .start_capture_triggered(32768, 8_000_000, 2, 21, 1000)
            .unwrap();
        work.send(&mut failed, &vec![0xa7; BLOCK], |_, _| {})
            .unwrap();
        let (rate, count, elapsed_us) = failed
            .0
            .poll_capture(id)
            .unwrap()
            .expect("SPI-triggered capture completed during bulk transfer");
        let capture = failed.0.read_capture(id, count, elapsed_us).unwrap();
        logic.set_capture(capture.samples, rate);
        let spi = logic
            .annotations
            .iter()
            .filter(|annotation| annotation.protocol == "SPI")
            .count();
        let pwm = logic
            .annotations
            .iter()
            .filter(|annotation| annotation.protocol == "PWM")
            .count();
        assert!(spi > 100 && pwm > 20, "SPI={spi}, PWM={pwm}");
        assert!(logic
            .annotations
            .iter()
            .filter(|annotation| annotation.protocol == "SPI")
            .skip(1)
            .all(|annotation| annotation.text == "MOSI A7 / MISO 00"));
        std::fs::write("/tmp/picoharness-concurrent-signals.vcd", logic.vcd()).unwrap();
        std::fs::write(
            "/tmp/picoharness-concurrent-decode.csv",
            logic.annotation_csv(),
        )
        .unwrap();
        struct DropCommit {
            io: PhBoardLink,
            decoder: Decoder,
            drop_once: bool,
        }
        impl Io for DropCommit {
            fn term_status(&mut self) -> Result<(u8, u32), String> {
                Ok((1, 115_200))
            }
            fn uart_config(&mut self, baud: u32) -> Result<u32, String> {
                Io::uart_config(&mut self.io, baud)
            }
            fn uart_write(&mut self, data: &[u8]) -> Result<usize, String> {
                Io::uart_write(&mut self.io, data)
            }
            fn uart_read(&mut self) -> Result<Vec<u8>, String> {
                let bytes = Io::uart_read(&mut self.io)?;
                let (frames, _) = self.decoder.feed(&bytes);
                let mut output = vec![];
                for frame in frames {
                    if self.drop_once && frame.kind == 0x85 {
                        self.drop_once = false;
                    } else {
                        output.extend(frame.encode());
                    }
                }
                Ok(output)
            }
            fn spi_config(&mut self) -> Result<(), String> {
                Io::spi_config(&mut self.io)
            }
            fn spi_transfer(&mut self, data: &[u8]) -> Result<Vec<u8>, String> {
                Io::spi_transfer(&mut self.io, data)
            }
        }
        let mut dropped = DropCommit {
            io: failed.0,
            decoder: Decoder::default(),
            drop_once: true,
        };
        assert!(work
            .set_baud(&mut dropped, 921600)
            .unwrap_err()
            .contains("reverted"));
        work.request(&mut dropped, 4, &[]).unwrap();
        work.send(&mut dropped, b"lost-ack-rollback-ok", |_, _| {})
            .unwrap();
        eprintln!("Passive capture verified: UART={uart}, SPI={spi}, PWM={pwm}, SPI sample rate={rate} Hz");
        eprintln!("UART short echo, {} bytes of SPI bulk echo, readonly traces, 921600 baud and rollback verified", payload.len());
    }
    #[test]
    fn console_state_comes_from_the_device_term_status() {
        // The Debug core owns Hello discovery; the host reads the mode from
        // TermStatus instead of sniffing Work frames.
        let (tx, rx) = std::sync::mpsc::channel();
        let mut link = Link::new(tx, 115200, false);
        let mut io = ConsoleIo { term_mode: 1, ..Default::default() };
        link.send(&mut io, b"first-command", |_, _| {}).unwrap();
        assert!(link.framed && link.negotiated);
        assert_eq!(io.writes, [b"first-command".to_vec()]);
        assert!(rx
            .try_iter()
            .any(|event| matches!(event, Event::ConsoleState { framed: true, .. })));

        // A plain-UART device reports mode 0 and stays unframed.
        let (tx, _) = std::sync::mpsc::channel();
        let mut plain = Link::new(tx, 115200, false);
        let mut io = ConsoleIo { term_mode: 0, ..Default::default() };
        plain.poll(&mut io).unwrap();
        assert!(!plain.framed && !plain.negotiated);
    }

    #[test]
    fn console_payload_is_sent_once_as_a_ph_message() {
        let (tx, _) = std::sync::mpsc::channel();
        let mut link = Link::new(tx, 115200, false);
        let mut io = ConsoleIo::default();
        link.send(&mut io, b"plain-command", |_, _| {}).unwrap();
        assert_eq!(io.writes, [b"plain-command".to_vec()]);
        let reads = io.reads;
        link.send(&mut io, b"next-command", |_, _| {}).unwrap();
        assert_eq!(io.reads, reads, "A second send must not repeat discovery");
        assert_eq!(io.writes, [b"plain-command".to_vec(), b"next-command".to_vec()]);
    }

    #[test]
    fn set_baud_uses_the_device_transaction() {
        // The Debug core performs the pico_harness prepare/verify/commit dance;
        // the host asks for the rate and validates what the device applied.
        let (tx, rx) = std::sync::mpsc::channel();
        let mut link = Link::new(tx, 115200, true);
        let mut io = ConsoleIo { applied_baud: Some(921_600), ..Default::default() };
        link.set_baud(&mut io, 921_600).unwrap();
        assert_eq!(link.baud, 921_600);
        let published = rx
            .try_iter()
            .filter_map(|event| match event {
                Event::ConsoleState { baud, .. } => Some(baud),
                _ => None,
            })
            .last();
        assert_eq!(published, Some(921_600));

        // A device rate beyond the 2% tolerance is an error.
        let mut io = ConsoleIo { applied_baud: Some(115_200), ..Default::default() };
        let error = link.set_baud(&mut io, 921_600).unwrap_err();
        assert!(error.contains("not 921600 Hz"), "{error}");
    }

    #[test]
    fn console_state_event_is_published_on_change() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut link = Link::new(tx, 115200, false);
        let mut io = ConsoleIo { term_mode: 1, ..Default::default() };
        link.poll(&mut io).unwrap();
        assert!(link.framed && link.negotiated);
        assert!(rx
            .try_iter()
            .any(|event| matches!(event, Event::ConsoleState { framed: true, .. })));
    }

}
