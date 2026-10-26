//! Finite acquisitions over the dedicated Logic Analyzer CDC, never PH Terminal.
use std::{
    io::{self, Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// Firmware SUMP channel order, mapped back to Work GPIO bits for decoders.
pub const PINS: [u8; 16] = [
    28, 27, 26, 5, 17, 16, 11, 10, 15, 14, 13, 12, 19, 21, 18, 20,
];
pub use ph_protocol::logic::{profile, RATES};

#[derive(Clone, Copy)]
pub struct Options {
    pub count: u32,
    pub channels: u32,
    pub rate: u32,
    pub edge: u8,
    pub pin: u8,
    pub timeout_ms: u16,
}
impl Options {
    pub fn validate(&self) -> io::Result<()> {
        let capacity = profile(self.channels, self.edge, self.pin).map_or(0, |p| p.max_samples);
        if self.count == 0
            || self.count > capacity
            || !RATES.contains(&self.rate)
            || self.edge > 2
            || (self.edge != 0
                && (self.count < 8
                    || !PINS.contains(&self.pin)
                    || self.rate > 20_000_000
                    || self.timeout_ms == 0
                    || self.timeout_ms > 5000))
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "Logic sample count exceeds selected-channel memory capacity or sample rate is unsupported; edge capture requires >=8 samples, a connected Work GPIO, <=20 MS/s and timeout 1..5000 ms"));
        }
        Ok(())
    }
}
#[derive(Debug)]
pub struct Capture {
    pub samples: Vec<u32>,
    pub sample_hz: u32,
    pub trigger_index: Option<u32>,
}

fn read_until(
    port: &mut impl Read,
    bytes: &mut [u8],
    deadline: Instant,
    cancel: &AtomicBool,
) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Logic capture cancelled",
            ));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Logic capture timed out",
            ));
        }
        match port.read(&mut bytes[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Logic Analyzer disconnected",
                ))
            }
            Ok(count) => offset += count,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn acquire(
    port: &mut (impl Read + Write),
    options: Options,
    cancel: &AtomicBool,
) -> io::Result<Capture> {
    options.validate()?;
    port.write_all(&[0x02])?;
    let mut identity = [0; 4];
    read_until(
        port,
        &mut identity,
        Instant::now() + Duration::from_secs(1),
        cancel,
    )?;
    if &identity != b"1ALS" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Selected CDC is not a PicoHarness SUMP analyzer",
        ));
    }
    let profile = profile(options.channels, options.edge, options.pin).unwrap();
    let quantum = profile.quantum.max(4);
    let wire_count = options.count.div_ceil(quantum) * quantum;
    if options.channels != 0 {
        let selected = PINS.iter().enumerate().fold(0u32, |mask, (i, pin)| {
            mask | if profile.mask & (1u32 << pin) != 0 { 1u32 << i } else { 0 }
        });
        port.write_all(&[0x83])?;
        port.write_all(&selected.to_le_bytes())?;
    }
    let channel = PINS.iter().position(|p| *p == options.pin).unwrap_or(0);
    let mask = if options.edge == 0 {
        0
    } else {
        1u32 << channel
    };
    let value = if options.edge == 1 { mask } else { 0 };
    let packed = options.channels != 0 || options.count > ph_protocol::logic::LEGACY_SAMPLES;
    // This firmware's SUMP count is count/4, not the OLS count/4-1 encoding.
    for (opcode, value) in [
        (0x80, 100_000_000 / options.rate - 1),
        (0x81, wire_count / 4),
        (0x82, if packed { 0x80000000 } else { 0 }),
        (0xc0, mask),
        (0xc1, value),
    ] {
        port.write_all(&[opcode])?;
        port.write_all(&value.to_le_bytes())?;
    }
    port.write_all(&[0x01])?;
    let wait = if options.edge == 0 {
        0.
    } else {
        options.timeout_ms as f64 / 1000.
    };
    let deadline = Instant::now()
        + Duration::from_secs_f64(wait + wire_count as f64 / options.rate as f64 + 0.2);
    let wire_bytes = if packed { wire_count as usize * profile.storage_bits as usize / 8 } else { wire_count as usize * 4 };
    let mut bytes = vec![0; wire_bytes];
    // Bound trigger waiting separately from USB transfer time.
    read_until(port, &mut bytes[..1], deadline, cancel)?;
    read_until(
        port,
        &mut bytes[1..],
        Instant::now() + Duration::from_secs_f64(3. + wire_bytes as f64 / 200_000.),
        cancel,
    )?;
    let sample_hz = if packed {
        port.write_all(&[0x84])?;
        let mut status = [0; 20];
        read_until(port, &mut status, Instant::now() + Duration::from_secs(1), cancel)?;
        let word = |i| u32::from_le_bytes(status[i..i + 4].try_into().unwrap());
        if &status[..4] != b"PCP1" || word(8) != wire_count || word(4) & 0x1e != 0 || word(12) == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                format!("incomplete or stalled Logic capture: flags=0x{:x}, samples={}", word(4), word(8))));
        }
        if word(16) != crc32c(&bytes) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Logic capture CRC mismatch"));
        }
        word(12)
    } else { options.rate };
    let mut samples = Vec::with_capacity(wire_count as usize);
    for index in 0..wire_count as usize {
        let mut sample = 0;
        if packed {
            let field = if options.edge != 0 && profile.storage_bits == 8 {
                bytes[index] as u32
            } else if options.edge != 0 && profile.storage_bits == 16 {
                u16::from_le_bytes(bytes[index * 2..index * 2 + 2].try_into().unwrap()) as u32
            } else {
                let pack = 32 / profile.input_bits as usize;
                let offset = (index / pack) * 4;
                let word = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
                let shift = (pack - 1 - index % pack) * profile.input_bits as usize;
                (word >> shift) & (u32::MAX >> (32 - profile.input_bits))
            };
            let raw = field << profile.pin_base;
            for (debug, work) in ph_protocol::logic::PINS {
                if profile.mask & (1u32 << work) != 0 {
                    sample |= ((raw >> debug) & 1) << work;
                }
            }
        } else {
            let word = &bytes[index * 4..index * 4 + 4];
            let bits = u32::from_le_bytes(word.try_into().unwrap());
            if bits >> 16 != 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid SUMP channel word"));
            }
            for (channel, pin) in PINS.iter().enumerate() {
                sample |= ((bits >> channel) & 1) << pin;
            }
        }
        samples.push(sample);
    }
    let trigger_index = if options.edge == 0 {
        None
    } else {
        // The counted PIO retains a quarter-window of history. Its edge sample
        // precedes the first post-trigger sample by one position.
        let post = wire_count as usize / 4;
        let trigger = (post.saturating_sub(2)..=(post + 1).min(samples.len() - 1))
            .find(|&i| {
                i > 0
                    && (samples[i - 1] & (1 << options.pin) != 0) == (options.edge == 2)
                    && (samples[i] & (1 << options.pin) != 0) == (options.edge == 1)
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "SUMP returned no valid trigger edge (capture timed out or was incomplete)",
                )
            })?;
        Some(trigger as u32)
    };
    samples.truncate(options.count as usize);
    Ok(Capture {
        samples,
        sample_hz,
        trigger_index,
    })
}

fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 { crc = (crc >> 1) ^ (0x82f63b78u32 & 0u32.wrapping_sub(crc & 1)); }
    }
    !crc
}

pub fn capture(path: &Path, options: Options, cancel: &AtomicBool) -> io::Result<Capture> {
    options.validate()?;
    let mut port = serialport::new(path.to_string_lossy(), 115200)
        .timeout(Duration::from_millis(20))
        .open()?;
    port.write_data_terminal_ready(true)?;
    port.write_all(&[0; 5])?;
    std::thread::sleep(Duration::from_millis(10));
    port.clear(serialport::ClearBuffer::Input)?;
    let result = acquire(&mut port, options, cancel);
    // Reset even on timeout/cancellation so the next owner gets a clean stream.
    let reset = port.write_all(&[0; 5]);
    std::thread::sleep(Duration::from_millis(10));
    let _ = port.clear(serialport::ClearBuffer::Input);
    match result {
        Ok(value) => {
            reset?;
            Ok(value)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Wire {
        reply: io::Cursor<Vec<u8>>,
        written: Vec<u8>,
    }
    impl Read for Wire {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.reply.read(b)
        }
    }
    impl Write for Wire {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    fn wire(words: &[u32]) -> Wire {
        let mut reply = b"1ALS".to_vec();
        for word in words {
            reply.extend(word.to_le_bytes());
        }
        Wire {
            reply: io::Cursor::new(reply),
            written: Vec::new(),
        }
    }
    fn options() -> Options {
        Options {
            count: 4,
            channels: 0,
            rate: 2_000_000,
            edge: 0,
            pin: 5,
            timeout_ms: 1000,
        }
    }
    #[test]
    fn sump_wire_count_divider_and_work_gpio_mapping() {
        let mut port = wire(&[1, 1 << 3, 1 << 10, 1 << 15]);
        let capture = acquire(&mut port, options(), &AtomicBool::new(false)).unwrap();
        assert_eq!(capture.samples, [1 << 28, 1 << 5, 1 << 13, 1 << 20]);
        assert_eq!(&port.written[..6], &[2, 0x80, 49, 0, 0, 0]);
        assert_eq!(&port.written[6..11], &[0x81, 1, 0, 0, 0]);
        assert_eq!(port.written.last(), Some(&1));
    }
    #[test]
    fn short_request_consumes_complete_aligned_wire_capture() {
        let mut port = wire(&[8, 0, 0, 0]);
        let mut opts = options();
        opts.count = 1;
        assert_eq!(
            acquire(&mut port, opts, &AtomicBool::new(false))
                .unwrap()
                .samples,
            [1 << 5]
        );
        assert_eq!(port.reply.position(), 20);
    }
    #[test]
    fn rising_and_falling_trigger_use_wire_channels_and_work_gpio() {
        for edge in [1, 2] {
            let mut opts = options();
            opts.count = 16;
            opts.edge = edge;
            opts.pin = 13;
            let words: Vec<_> = (0..16)
                .map(|i| if (i >= 3) == (edge == 1) { 1 << 10 } else { 0 })
                .collect();
            let mut port = wire(&words);
            let capture = acquire(&mut port, opts, &AtomicBool::new(false)).unwrap();
            assert_eq!(capture.trigger_index, Some(3));
            assert_eq!(&port.written[16..21], &[0xc0, 0, 4, 0, 0]);
            assert_eq!(
                &port.written[21..26],
                if edge == 1 {
                    &[0xc1, 0, 4, 0, 0]
                } else {
                    &[0xc1, 0, 0, 0, 0]
                }
            );
        }
    }
    fn packed_wire(opts: Options, raw: &[u32], flags: u32) -> Wire {
        let p = profile(opts.channels, opts.edge, opts.pin).unwrap();
        let mut bytes = Vec::new();
        if opts.edge != 0 && p.storage_bits <= 16 {
            for sample in raw {
                let field = sample >> p.pin_base;
                bytes.push(field as u8);
                if p.storage_bits == 16 { bytes.push((field >> 8) as u8); }
            }
        } else {
            let pack = 32 / p.input_bits as usize;
            for group in raw.chunks(pack) {
                let mut word = 0u32;
                for sample in group {
                    if p.input_bits != 32 { word <<= p.input_bits; }
                    word |= (sample >> p.pin_base) & (u32::MAX >> (32 - p.input_bits));
                }
                bytes.extend(word.to_le_bytes());
            }
        }
        let mut reply = b"1ALS".to_vec();
        reply.extend(&bytes);
        reply.extend(b"PCP1");
        for value in [flags, raw.len() as u32, opts.rate, crc32c(&bytes)] {
            reply.extend(value.to_le_bytes());
        }
        Wire { reply: io::Cursor::new(reply), written: Vec::new() }
    }
    #[test]
    fn selected_channel_packing_preserves_order_and_crc_for_all_widths() {
        assert_eq!(crc32c(b"123456789"), 0xe3069283);
        for mask in [1 << 12, (1 << 12) | (1 << 13), (1 << 10) | (1 << 12),
                     (1 << 10) | (1 << 17), (1 << 10) | (1 << 19), 0x1c3ffc20] {
            let mut opts = options(); opts.channels = mask; opts.count = 61;
            let raw: Vec<u32> = (0..64).map(|i| ph_protocol::logic::PINS.iter()
                .filter(|(_, work)| mask & (1u32 << work) != 0 && (i + *work as usize) % 3 == 0)
                .fold(0, |value, (debug, _)| value | (1u32 << debug))).collect();
            let mut port = packed_wire(opts, &raw, 0);
            let result = acquire(&mut port, opts, &AtomicBool::new(false)).unwrap();
            let expected: Vec<u32> = raw[..61].iter().map(|raw| ph_protocol::logic::PINS.iter()
                .fold(0, |value, (debug, work)| value | (((raw >> debug) & 1) << work))).collect();
            assert_eq!(result.samples, expected);
            assert_eq!(port.written.last(), Some(&0x84));
            let mut corrupt = packed_wire(opts, &raw, 0);
            corrupt.reply.get_mut()[4] ^= 1;
            assert!(acquire(&mut corrupt, opts, &AtomicBool::new(false)).unwrap_err().to_string().contains("CRC"));
            assert!(acquire(&mut packed_wire(opts, &raw, 2), opts, &AtomicBool::new(false)).is_err());
        }
    }

    #[test]
    fn invalid_or_truncated_data_and_cancel_are_errors() {
        for words in [vec![1 << 16; 4], vec![0; 3]] {
            assert!(acquire(&mut wire(&words), options(), &AtomicBool::new(false)).is_err());
        }
        assert!(acquire(&mut wire(&[0; 4]), options(), &AtomicBool::new(true)).is_err());
        let mut opts = options();
        opts.edge = 1;
        assert!(acquire(&mut wire(&[0; 4]), opts, &AtomicBool::new(false)).is_err());
    }
}
