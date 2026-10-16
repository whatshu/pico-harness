//! One USB owner per LIVE device. All blocking PDBG and CMSIS-DAP calls run off the egui thread.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::work_link;
use ph_app_link::{CmsisDap, Measurement, PhBoardLink, PowerStatus};

const DHCSR: u32 = 0xe000_edf0;
const DCRSR: u32 = 0xe000_edf4;
const DCRDR: u32 = 0xe000_edf8;
const DBGKEY: u32 = 0xa05f_0000;

pub enum Command {
    Power(bool),
    Reset,
    Baud(u32),
    Capture {
        count: u32,
        rate: u32,
        trigger: u8,
        pin: u8,
        timeout_ms: u16,
    },
    CaptureCancel,
    SwdConnect {
        target: String,
        core: u8,
    },
    SwdDisconnect,
    Swd(String),
    MemoryRead {
        address: u32,
        length: usize,
        target: String,
    },
    MemoryExport {
        address: u32,
        length: usize,
    },
    DataSend(Vec<u8>),
    DataReceive(usize),
    Shutdown,
}

pub enum Event {
    Ready(String),
    Status(PowerStatus),
    Measurement {
        value: Measurement,
        captured_at: Instant,
        timestamp_unix_ms: u64,
    },
    MeasurementError(String),
    UartRx(Vec<u8>),
    UartTx(Vec<u8>),
    Trace {
        bus: &'static str,
        direction: &'static str,
        bytes: Vec<u8>,
    },
    ConsoleState {
        framed: bool,
        baud: u32,
    },
    WorkSupport {
        verified: bool,
        bulk_ready: bool,
    },
    BaudComplete,
    Capture {
        samples: Vec<u32>,
        rate: u32,
        trigger_index: Option<u32>,
    },
    SwdState {
        connected: bool,
        halted: bool,
        idcode: u32,
    },
    SwdRegisters([u32; 17]),
    SwdMemory {
        address: u32,
        bytes: Vec<u8>,
    },
    MemoryExport {
        address: u32,
        bytes: Vec<u8>,
    },
    SwdLog(String),
    DataProgress {
        sent: usize,
        total: usize,
    },
    DataReceived(Vec<u8>),
    DataComplete,
    Error {
        operation: &'static str,
        message: String,
    },
}

pub struct Session {
    pub tx: Sender<Command>,
    pub rx: Receiver<Event>,
    worker: Option<thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
}

impl Session {
    #[cfg(any(test, feature = "ui-perf"))]
    pub fn performance_source() -> (Self, Sender<Event>) {
        let (tx, _commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        (
            Self {
                tx,
                rx,
                worker: None,
                cancel: Arc::new(AtomicBool::new(false)),
            },
            events,
        )
    }

    #[cfg(test)]
    pub fn test_source() -> (Self, Sender<Event>, Receiver<Command>) {
        let (mut session, events) = Self::performance_source();
        let (tx, commands) = mpsc::channel();
        session.tx = tx;
        (session, events, commands)
    }

    pub fn start(serial: String, baud: u32, framed: bool) -> Self {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker = thread::spawn(move || {
            if let Err(message) = run(&serial, baud, framed, commands, &events, worker_cancel) {
                let _ = events.send(Event::Error {
                    operation: "USB",
                    message,
                });
            }
        });
        Self {
            tx,
            rx,
            worker: Some(worker),
            cancel,
        }
    }

    pub fn send(&self, command: Command) -> Result<(), String> {
        self.tx
            .send(command)
            .map_err(|_| "USB session closed".into())
    }
    pub fn shutdown(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(
    serial: &str,
    baud: u32,
    framed: bool,
    commands: Receiver<Command>,
    events: &Sender<Event>,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    let mut link = PhBoardLink::open_serial(serial).map_err(|e| e.to_string())?;
    let info = link.info().map_err(|e| e.to_string())?;
    let actual_baud = link
        .uart_config(0, baud, 8, 1, 0)
        .map_err(|e| e.to_string())?;
    let _ = events.send(Event::Ready(info));
    let mut work =
        work_link::Link::new(events.clone(), actual_baud, framed).with_cancel(cancel.clone());
    let mut dap: Option<CmsisDap> = None;
    let mut next_status = Instant::now();
    let mut next_measure = Instant::now();
    let mut next_uart = Instant::now();
    let mut powered = false;
    let mut failures = 0;
    let asynchronous = link
        .info()
        .map_err(|e| e.to_string())?
        .split(';')
        .any(|field| field == "capture-async");
    let mut capture: Option<(u32, u32, Instant)> = None;
    loop {
        if cancel.load(Ordering::Relaxed) {
            stop_capture(&mut link, &mut capture);
            return Ok(());
        }
        while let Ok(command) = commands.try_recv() {
            if matches!(command, Command::Shutdown) {
                stop_capture(&mut link, &mut capture);
                return Ok(());
            }
            let operation = match &command {
                Command::Power(_) | Command::Reset => "Power",
                Command::Baud(_) => "Baud",
                Command::Capture { .. } | Command::CaptureCancel => "Logic",
                Command::SwdConnect { .. } => "SWD connect",
                Command::SwdDisconnect | Command::Swd(_) | Command::MemoryRead { .. } => "SWD",
                Command::MemoryExport { .. } => "Memory export",
                Command::DataSend(_) | Command::DataReceive(_) => "Data",
                Command::Shutdown => unreachable!(),
            };
            if let Err(message) = execute(
                command,
                &mut link,
                &mut work,
                &mut dap,
                events,
                asynchronous,
                &mut capture,
            ) {
                let _ = events.send(Event::Error { operation, message });
            }
            next_status = Instant::now();
            if cancel.load(Ordering::Relaxed) {
                stop_capture(&mut link, &mut capture);
                return Ok(());
            }
        }
        let now = Instant::now();
        if let Some((id, requested, started)) = capture {
            let result = link
                .poll_capture_status(id)
                .map_err(|e| e.to_string())
                .and_then(|completed| {
                    if let Some(status) = completed {
                        let ph_app_link::protocol::CaptureStatus {
                            sample_hz: rate,
                            count,
                            elapsed_us,
                            trigger_index,
                        } = status;
                        if count != requested {
                            return Err("Capture returned fewer samples than requested".into());
                        }
                        let captured = link
                            .read_capture(id, count, elapsed_us)
                            .map_err(|e| e.to_string())?;
                        let _ = events.send(Event::Capture {
                            samples: captured.samples,
                            rate,
                            trigger_index,
                        });
                        capture = None;
                    } else if started.elapsed() > Duration::from_secs(16) {
                        return Err("Capture timed out".into());
                    }
                    Ok(())
                });
            if let Err(message) = result {
                let _ = link.cancel_capture();
                capture = None;
                let _ = events.send(Event::Error {
                    operation: "Logic",
                    message,
                });
            }
        }
        if now >= next_status {
            match link.status() {
                Ok(status) => {
                    powered = status.power_good && !status.power_fault;
                    failures = 0;
                    let _ = events.send(Event::Status(status));
                }
                Err(error) => {
                    failures += 1;
                    // A transient miss must not kill the session: the link is
                    // still usable and the next poll usually recovers. Only
                    // give up after a long silence (device unplugged).
                    powered = false;
                    // Report the first miss and then sparsely, so a long
                    // outage is visible without flooding the console log.
                    if failures == 1 || failures % 10 == 0 {
                        let _ = events.send(Event::Error {
                            operation: "Link",
                            message: error.to_string(),
                        });
                    }
                    if failures >= 40 {
                        return Err(format!("PH status: {error}"));
                    }
                }
            }
            next_status = now + Duration::from_millis(300);
        }
        if now >= next_measure {
            match link.measure() {
                Ok(value) => {
                    let _ = events.send(Event::Measurement {
                        value,
                        captured_at: Instant::now(),
                        timestamp_unix_ms: ph_app::telemetry::unix_ms(),
                    });
                }
                Err(error) => {
                    let _ = events.send(Event::MeasurementError(error.to_string()));
                }
            }
            next_measure = now + Duration::from_millis(500);
        }
        if now >= next_uart {
            if powered {
                match work.poll_console(&mut link) {
                    Ok(()) => {}
                    Err(error) => {
                        let _ = events.send(Event::Error {
                            operation: "UART",
                            message: error.to_string(),
                        });
                    }
                }
            }
            next_uart = now + Duration::from_millis(50);
        }
        match commands.recv_timeout(Duration::from_millis(10)) {
            Ok(command) => {
                if matches!(command, Command::Shutdown) {
                    stop_capture(&mut link, &mut capture);
                    return Ok(());
                }
                let operation = match &command {
                    Command::Power(_) | Command::Reset => "Power",
                    Command::Baud(_) => "Baud",
                    Command::Capture { .. } | Command::CaptureCancel => "Logic",
                    Command::SwdConnect { .. } => "SWD connect",
                    Command::SwdDisconnect | Command::Swd(_) | Command::MemoryRead { .. } => "SWD",
                    Command::MemoryExport { .. } => "Memory export",
                    Command::DataSend(_) | Command::DataReceive(_) => "Data",
                    Command::Shutdown => unreachable!(),
                };
                if let Err(message) = execute(
                    command,
                    &mut link,
                    &mut work,
                    &mut dap,
                    events,
                    asynchronous,
                    &mut capture,
                ) {
                    let _ = events.send(Event::Error { operation, message });
                }
                next_status = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop_capture(&mut link, &mut capture);
                return Ok(());
            }
        }
    }
}

fn stop_capture(link: &mut PhBoardLink, capture: &mut Option<(u32, u32, Instant)>) {
    if capture.take().is_some() {
        let _ = link.cancel_capture();
    }
}

fn execute(
    command: Command,
    link: &mut PhBoardLink,
    work: &mut work_link::Link,
    dap: &mut Option<CmsisDap>,
    events: &Sender<Event>,
    asynchronous: bool,
    capture: &mut Option<(u32, u32, Instant)>,
) -> Result<(), String> {
    match command {
        Command::Power(on) => {
            if !on {
                disconnect(dap, events);
            }
            link.set_power(on).map_err(|e| e.to_string())?;
            work.reset(link)?;
            let _ = events.send(Event::Status(link.status().map_err(|e| e.to_string())?));
        }
        Command::Reset => {
            disconnect(dap, events);
            link.reset_pulse(50).map_err(|e| e.to_string())?;
            work.reset(link)?;
        }
        Command::Baud(baud) => {
            work.set_baud(link, baud)?;
            let _ = events.send(Event::BaudComplete);
        }
        Command::Capture {
            count,
            rate,
            trigger,
            pin,
            timeout_ms,
        } => {
            if asynchronous {
                if capture.is_some() {
                    return Err("A logic capture is already running".into());
                }
                let id = link
                    .start_capture_triggered(count, rate, trigger, pin, timeout_ms)
                    .map_err(|e| e.to_string())?;
                *capture = Some((id, count, Instant::now()));
                return Ok(());
            }
            let captured = link
                .capture_triggered(count, rate, trigger, pin, timeout_ms)
                .map_err(|e| e.to_string())?;
            let frequency = captured.actual_frequency().unwrap_or(rate as f64).round() as u32;
            let _ = events.send(Event::Capture {
                samples: captured.samples,
                rate: frequency,
                trigger_index: None,
            });
        }
        Command::CaptureCancel => {
            if capture.take().is_some() {
                link.cancel_capture().map_err(|e| e.to_string())?;
            }
        }
        Command::SwdConnect {
            target: name,
            core: chosen,
        } => {
            disconnect(dap, events);
            let serial = link.serial.clone().ok_or("Probe has no USB serial")?;
            let mut opened = CmsisDap::open_serial(serial).map_err(|e| e.to_string())?;
            if name == "rp2040" {
                opened
                    .connect_swd_core(1_000_000, chosen)
                    .map_err(|e| e.to_string())?;
            } else if name == "rp2350" {
                let ap_base = if chosen == 0 { 0x2000 } else { 0x4000 };
                opened
                    .connect_swd_adiv6(1_000_000, ap_base)
                    .map_err(|e| e.to_string())?;
            } else {
                opened.connect_swd(1_000_000).map_err(|e| e.to_string())?;
            }
            opened.power_up_debug_port().map_err(|e| e.to_string())?;
            let idcode = opened.read_dp_idcode().map_err(|e| e.to_string())?;
            let halted = read_word(&mut opened, DHCSR)? & (1 << 17) != 0;
            *dap = Some(opened);
            let _ = events.send(Event::SwdState {
                connected: true,
                halted,
                idcode,
            });
            if halted {
                let _ = swd_command(dap.as_mut().unwrap(), "reg", events);
            }
        }
        Command::SwdDisconnect => disconnect(dap, events),
        Command::MemoryRead {
            address,
            length,
            target,
        } => {
            let length = ph_app::memory_export::read_length(&target, address, length);
            let selected = dap.as_mut().ok_or("Connect SWD first")?;
            let bytes = read_memory_bytes(selected, address, length, work)?;
            let _ = events.send(Event::SwdMemory { address, bytes });
            let _ = events.send(Event::SwdLog(format!(
                "Read {length} bytes at 0x{address:08X}"
            )));
        }
        Command::MemoryExport { address, length } => {
            ph_app::memory_export::validate_range(address, length)?;
            let selected = dap.as_mut().ok_or("Connect SWD first")?;
            if read_word(selected, DHCSR)? & (1 << 17) == 0 {
                return Err("Halt the Work Core before exporting memory".into());
            }
            let bytes = read_memory_bytes(selected, address, length, work)?;
            let _ = events.send(Event::MemoryExport { address, bytes });
        }
        Command::Swd(text) => {
            if text.trim() == "reset" {
                disconnect(dap, events);
                link.reset_pulse(50).map_err(|e| e.to_string())?;
                let _ = events.send(Event::SwdLog("Target reset; reconnect SWD".into()));
                return Ok(());
            }
            let selected = dap.as_mut().ok_or("Connect SWD first")?;
            let result = swd_command(selected, &text, events)?;
            let _ = events.send(Event::SwdLog(result));
        }
        Command::DataSend(bytes) => {
            work.send(link, &bytes, |sent, total| {
                let _ = events.send(Event::DataProgress { sent, total });
            })?;
            let _ = events.send(Event::DataComplete);
        }
        Command::DataReceive(maximum) => {
            let bytes = work.receive(link, maximum)?;
            let _ = events.send(Event::DataProgress {
                sent: bytes.len(),
                total: bytes.len(),
            });
            let _ = events.send(Event::DataReceived(bytes));
            let _ = events.send(Event::DataComplete);
        }
        Command::Shutdown => unreachable!(),
    }
    Ok(())
}

fn disconnect(dap: &mut Option<CmsisDap>, events: &Sender<Event>) {
    if let Some(mut old) = dap.take() {
        let _ = old.disconnect();
        let _ = events.send(Event::SwdState {
            connected: false,
            halted: false,
            idcode: 0,
        });
    }
}

fn read_word(dap: &mut CmsisDap, address: u32) -> Result<u32, String> {
    Ok(dap.read_memory32(address, 1).map_err(|e| e.to_string())?[0])
}

fn wait_state(dap: &mut CmsisDap, halted: bool) -> Result<(), String> {
    for _ in 0..100 {
        if (read_word(dap, DHCSR)? & (1 << 17) != 0) == halted {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(2));
    }
    Err("SWD core state did not change".into())
}

fn read_register(dap: &mut CmsisDap, index: usize) -> Result<u32, String> {
    dap.write_memory32(DCRSR, index as u32)
        .map_err(|e| e.to_string())?;
    for _ in 0..100 {
        if read_word(dap, DHCSR)? & (1 << 16) != 0 {
            return read_word(dap, DCRDR);
        }
        thread::sleep(Duration::from_millis(1));
    }
    Err("SWD register read timed out".into())
}

fn read_memory_bytes(
    dap: &mut CmsisDap,
    address: u32,
    length: usize,
    work: &work_link::Link,
) -> Result<Vec<u8>, String> {
    ph_app::memory_export::validate_range(address, length)?;
    let prefix = (address & 3) as usize;
    let aligned = address & !3;
    let words = (prefix + length).div_ceil(4);
    let mut bytes = Vec::with_capacity(words * 4);
    for offset in (0..words).step_by(64) {
        work.check_cancelled()?;
        let values = dap
            .read_memory32(aligned + offset as u32 * 4, (words - offset).min(64))
            .map_err(|e| e.to_string())?;
        bytes.extend(values.iter().flat_map(|word| word.to_le_bytes()));
    }
    Ok(bytes[prefix..prefix + length].to_vec())
}

fn parse_u32(input: &str) -> Result<u32, String> {
    let value = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"));
    match value {
        Some(value) => u32::from_str_radix(value, 16),
        None => input.parse(),
    }
    .map_err(|_| format!("Invalid number: {input}"))
}

fn swd_command(dap: &mut CmsisDap, text: &str, events: &Sender<Event>) -> Result<String, String> {
    let words: Vec<_> = text.split_whitespace().collect();
    let halted = read_word(dap, DHCSR)? & (1 << 17) != 0;
    match words.as_slice() {
        ["help"] => Ok("halt | resume | reg [index] [value] | mdw address | mww address value | dp | ap address".into()),
        ["halt"] => {
            dap.write_memory32(DHCSR, DBGKEY | 3).map_err(|e| e.to_string())?;
            wait_state(dap, true)?;
            let _ = events.send(Event::SwdState {
                connected: true,
                halted: true,
                idcode: 0,
            });
            let _ = swd_command(dap, "reg", events)?;
            Ok("Target halted".into())
        }
        ["resume"] => {
            dap.write_memory32(DHCSR, DBGKEY | 1).map_err(|e| e.to_string())?;
            wait_state(dap, false)?;
            let _ = events.send(Event::SwdState {
                connected: true,
                halted: false,
                idcode: 0,
            });
            Ok("Target running".into())
        }
        ["reg"] if halted => {
            let mut values = [0; 17];
            for (index, slot) in values.iter_mut().enumerate() {
                *slot = read_register(dap, index)?;
            }
            let _ = events.send(Event::SwdRegisters(values));
            Ok("Registers refreshed".into())
        }
        ["reg", index] if halted => {
            let index: usize = index.parse().map_err(|_| "Invalid register index")?;
            if index > 16 {
                return Err("Register index must be 0..16".into());
            }
            let value = read_register(dap, index)?;
            Ok(format!("R{index} = 0x{value:08X}"))
        }
        ["reg", index, value] if halted => {
            let index: usize = index.parse().map_err(|_| "Invalid register index")?;
            if index > 15 {
                return Err("Register index must be 0..15 for writes".into());
            }
            let value = parse_u32(value)?;
            dap.write_memory32(DCRDR, value).map_err(|e| e.to_string())?;
            dap.write_memory32(DCRSR, index as u32 | (1 << 16)).map_err(|e| e.to_string())?;
            let _ = swd_command(dap, "reg", events)?;
            Ok(format!("R{index} = 0x{value:08X}"))
        }
        ["mdw", address] => {
            let address = parse_u32(address)?;
            if address & 3 != 0 {
                return Err("Address must be 4-byte aligned".into());
            }
            let value = read_word(dap, address)?;
            let _ = events.send(Event::SwdMemory {
                address,
                bytes: value.to_le_bytes().to_vec(),
            });
            Ok(format!("0x{address:08X}: {value:08X}"))
        }
        ["mdw", address, count] => {
            let address = parse_u32(address)?;
            let count: usize = count.parse().map_err(|_| "Invalid word count")?;
            if address & 3 != 0 || !(1..=64).contains(&count) {
                return Err("Read requires aligned address and 1..64 words".into());
            }
            let words = dap.read_memory32(address,count).map_err(|e| e.to_string())?;
            let bytes = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            let _ = events.send(Event::SwdMemory { address, bytes });
            Ok(format!("Read {count} words at 0x{address:08X}"))
        }
        ["mww", address, value] if halted => {
            let address = parse_u32(address)?;
            let value = parse_u32(value)?;
            dap.write_memory32(address, value).map_err(|e| e.to_string())?;
            let _ = events.send(Event::SwdMemory {
                address,
                bytes: value.to_le_bytes().to_vec(),
            });
            Ok(format!("0x{address:08X}: {value:08X}"))
        }
        ["dp"] => Ok(format!("DPIDR = 0x{:08X}", dap.read_dp_idcode().map_err(|e| e.to_string())?)),
        ["ap", address] => {
            let address = parse_u32(address)? as u8;
            Ok(format!("AP[0x{address:02X}] = 0x{:08X}", dap.read_ap(address).map_err(|e| e.to_string())?))
        }
        _ if !halted => Err("Halt the Work Core before reading or writing registers and memory".into()),
        _ => Err("Unknown SWD command; type help".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_u32;

    #[test]
    #[ignore = "Requires an explicitly selected probe and silent Work firmware; samples UART/SPI, timeout and cancellation without flashing"]
    fn connected_pretrigger_spi_and_recovery() {
        use ph_app_link::PhBoardLink;
        use std::time::{Duration, Instant};

        let serial = std::env::var("PDBG_PROBE_SERIAL").expect("PDBG_PROBE_SERIAL");
        let mut link = PhBoardLink::open_serial(serial).unwrap();
        assert!(link.info().unwrap().contains("capture-pretrigger"));
        link.uart_config(0, 115200, 8, 1, 0).unwrap();
        // At 100 kHz the ring wraps while waiting, including during timeout.
        let id = link
            .start_capture_triggered(32768, 100_000, 1, 13, 100)
            .unwrap();
        let start = Instant::now();
        loop {
            match link.poll_capture_status(id) {
                Ok(None) => {
                    assert!(start.elapsed() < Duration::from_secs(2));
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => {
                    assert!(error.to_string().contains("timed out"));
                    break;
                }
                result => panic!("A silent GP13 must time out: {result:?}"),
            }
        }
        let id = link
            .start_capture_triggered(32768, 20_000_000, 2, 13, 1000)
            .unwrap();
        std::thread::sleep(Duration::from_millis(30));
        link.cancel_capture().unwrap();
        assert!(link
            .poll_capture_status(id)
            .unwrap_err()
            .to_string()
            .contains("cancelled"));
        assert!(link
            .start_capture_triggered(3, 2_000_000, 1, 13, 1000)
            .is_err());
        // Immediate captures still work after a wrapped-ring timeout/cancel.
        for count in [1, 4, 1023, 32768] {
            assert_eq!(
                link.capture(count, 2_000_000).unwrap().samples.len(),
                count as usize
            );
        }

        // Leave 16 ms of history for the USB round trips between UART and
        // SPI commands. Both signals are sampled at >= 4 samples per bit.
        link.spi_config(50_000, 0).unwrap();
        let id = link
            .start_capture_triggered(32768, 500_000, 2, 21, 1000)
            .unwrap();
        std::thread::sleep(Duration::from_millis(25));
        link.uart_write(0, b"hi\n", Duration::from_secs(1)).unwrap();
        link.spi_transfer(&vec![0xa7; 2048]).unwrap();
        let status = link
            .poll_capture_status(id)
            .unwrap()
            .expect("Capture completes during SPI");
        assert_eq!(status.trigger_index, Some(8192));
        let capture = link
            .read_capture(id, status.count, status.elapsed_us)
            .unwrap();
        let spi = crate::logic::decode_spi(
            &capture.samples,
            status.sample_hz,
            [18, 20, 19, 21],
            0,
            false,
            8,
        );
        assert!(spi.len() > 100, "SPI decoded {} bytes", spi.len());
        assert!(spi
            .iter()
            .all(|a| a.text.starts_with("MOSI A7") && !a.error));
        let uart =
            crate::logic::decode_uart(&capture.samples, status.sample_hz, 13, 115200, 8, 0, 1);
        let bytes: Vec<_> = uart
            .iter()
            .map(|a| a.text.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(bytes, ["0x68", "0x69", "0x0A"]);
        assert!(uart.iter().all(|a| !a.error));
        eprintln!("SPI CS edge: {} MOSI A7 bytes plus complete UART hi-newline; wrapped timeout/cancel and immediate recovery passed", spi.len());
    }
    #[test]
    #[ignore = "Requires an explicitly selected probe with blink or another silent Work firmware; does not program Flash"]
    fn connected_plain_uart_console_trigger() {
        use super::{Command, Event, Session};
        use std::time::{Duration, Instant};
        let serial = std::env::var("PDBG_PROBE_SERIAL").expect("PDBG_PROBE_SERIAL");
        let session = Session::start(serial, 115200, false);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(event) = session.rx.recv_timeout(Duration::from_millis(50)) {
                match event {
                    Event::Error { operation, message } => panic!("{operation}: {message}"),
                    Event::UartRx(bytes) if !bytes.is_empty() => {
                        panic!("The plain Work fixture must remain silent")
                    }
                    Event::Trace {
                        direction: "TX", ..
                    } => {
                        panic!("Passive observation must not inject UART/SPI traffic")
                    }
                    Event::WorkSupport { verified: true, .. } => {
                        panic!("The plain Work fixture must not advertise the Harness protocol")
                    }
                    _ => {}
                }
            }
        }
        let cases = std::iter::once((0, 2_000_000)).chain((0..2).flat_map(|_| {
            crate::logic::SAMPLE_RATES
                .into_iter()
                .filter(|rate| *rate >= 500_000)
                .flat_map(|rate| [(1, rate), (2, rate)])
        }));
        for (trigger, requested_rate) in cases {
            session
                .send(Command::Capture {
                    count: 32768,
                    rate: requested_rate,
                    trigger,
                    pin: 13,
                    timeout_ms: 10000,
                })
                .unwrap();
            if trigger != 0 {
                std::thread::sleep(Duration::from_millis(100));
            }
            // No sacrificial prefix: the trigger must retain the first frame,
            // including its start bit when triggering on a later rising edge.
            let payload = b"hi\n".to_vec();
            session.send(Command::DataSend(payload.clone())).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let (mut sent, mut complete, mut captured) = (false, false, false);
            while !(sent && complete && captured) {
                let event = session
                    .rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .expect("Plain Console send and capture must complete");
                match event {
                    Event::UartTx(bytes) => {
                        assert_eq!(bytes, payload, "Console must send the plain bytes once");
                        sent = true;
                    }
                    Event::DataComplete => complete = true,
                    Event::Capture {
                        samples,
                        rate,
                        trigger_index,
                    } => {
                        assert_eq!(samples.len(), 32768);
                        assert_eq!(rate, requested_rate);
                        assert_eq!(trigger_index, if trigger == 0 { None } else { Some(8192) });
                        if let Some(index) = trigger_index {
                            assert_eq!(samples[index as usize] & (1 << 13) != 0, trigger == 1);
                        }
                        let decoded =
                            crate::logic::decode_uart(&samples, rate, 13, 115200, 8, 0, 1);
                        let expected: Vec<_> =
                            payload.iter().map(|byte| format!("0x{byte:02X}")).collect();
                        let actual: Vec<_> = decoded
                            .iter()
                            .map(|annotation| {
                                annotation
                                    .text
                                    .split_whitespace()
                                    .next()
                                    .unwrap()
                                    .to_owned()
                            })
                            .collect();
                        assert_eq!(actual, expected, "GP13 edge={trigger}");
                        assert!(decoded.iter().rev().take(expected.len()).all(|a| !a.error));
                        captured = true;
                    }
                    Event::WorkSupport { verified: true, .. } => {
                        panic!("A plain Console send must not establish a Harness protocol")
                    }
                    Event::Error { operation, message } => panic!("{operation}: {message}"),
                    _ => {}
                }
            }
            eprintln!("Work GP13 {requested_rate} Hz edge={trigger}: 32768 samples, exact hi-newline decoded; no Harness protocol");
        }
    }
    #[test]
    #[ignore = "Requires an explicitly selected probe with a Work Console echo firmware; checks all baud presets and UART triggers"]
    fn connected_baud_presets_and_console_trigger() {
        use super::{Command, Event, Session};
        use std::time::{Duration, Instant};
        fn wait(session: &Session, mut done: impl FnMut(Event) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(14);
            loop {
                let event = session
                    .rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap();
                if let Event::Error { operation, message } = &event {
                    panic!("{operation}: {message}");
                }
                if done(event) {
                    return;
                }
            }
        }
        let serial = std::env::var("PDBG_PROBE_SERIAL").expect("PDBG_PROBE_SERIAL");
        let session = Session::start(serial, 115200, false);
        for round in 0..3 {
            for baud in crate::pages::BAUD_PRESETS {
                session.send(Command::Baud(baud)).unwrap();
                let mut actual = 0;
                wait(&session, |event| {
                    if let Event::ConsoleState { baud, .. } = event {
                        actual = baud;
                    }
                    matches!(event, Event::BaudComplete)
                });
                assert!(
                    actual.abs_diff(baud) <= baud / 50,
                    "Requested {baud}, actual {actual}"
                );
                let payload = format!("preset-{round}-{baud}").into_bytes();
                session.send(Command::DataSend(payload.clone())).unwrap();
                let (mut echo, mut complete) = (false, false);
                wait(&session, |event| {
                    match event {
                        Event::UartRx(bytes) if bytes == payload => echo = true,
                        Event::DataComplete => complete = true,
                        _ => {}
                    }
                    echo && complete
                });
                eprintln!("round={round} requested={baud} actual={actual}: echo verified");
            }
        }
        session.send(Command::Baud(115200)).unwrap();
        wait(&session, |event| matches!(event, Event::BaudComplete));
        for pin in [12, 13] {
            session
                .send(Command::Capture {
                    count: 32768,
                    rate: 2_000_000,
                    trigger: 1,
                    pin,
                    timeout_ms: 10000,
                })
                .unwrap();
            std::thread::sleep(Duration::from_millis(100));
            let payload = format!("trigger-GP{pin}").into_bytes();
            session.send(Command::DataSend(payload.clone())).unwrap();
            let (mut echo, mut complete, mut captured) = (false, false, false);
            wait(&session, |event| {
                match event {
                    Event::UartRx(bytes) if bytes == payload => echo = true,
                    Event::DataComplete => complete = true,
                    Event::Capture {
                        samples,
                        rate,
                        trigger_index,
                    } => {
                        assert_eq!(samples.len(), 32768);
                        assert_eq!(rate, 2_000_000);
                        let index = trigger_index
                            .expect("Debug firmware must retain pre-trigger data")
                            as usize;
                        assert_ne!(
                            samples[index] & (1 << pin),
                            0,
                            "Post-trigger sample must match the rising edge"
                        );
                        assert!(
                            samples
                                .windows(2)
                                .filter(|pair| (pair[0] ^ pair[1]) & (1 << pin) != 0)
                                .count()
                                > 20
                        );
                        captured = true;
                    }
                    _ => {}
                }
                echo && complete && captured
            });
            eprintln!(
                "GP{pin} rising, 10s timeout: concurrent Console echo and 32768 samples verified"
            );
        }
    }
    #[test]
    #[ignore = "Requires a connected probe and the harness-console Work firmware"]
    fn connected_console_reconnect_preserves_baud() {
        use super::{Command, Event, Session};
        use std::time::{Duration, Instant};
        fn wait(session: &Session, mut complete: impl FnMut(&Event) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let event = session
                    .rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap();
                if let Event::Error { operation, message } = &event {
                    panic!("{operation}: {message}");
                }
                if complete(&event) {
                    return;
                }
            }
        }
        let serial = std::env::var("PDBG_PROBE_SERIAL").expect("PDBG_PROBE_SERIAL");
        let first = Session::start(serial.clone(), 115200, false);
        first
            .send(Command::DataSend(b"first-on-ready".to_vec()))
            .unwrap();
        let mut initial_echo = false;
        let mut initial_complete = false;
        wait(&first, |event| {
            match event {
                Event::UartRx(bytes) if bytes == b"first-on-ready" => initial_echo = true,
                Event::DataComplete => initial_complete = true,
                _ => {}
            }
            initial_echo && initial_complete
        });
        first.send(Command::Baud(921600)).unwrap();
        let mut saved_baud = 0;
        wait(&first, |event| {
            if let Event::ConsoleState { baud, .. } = event {
                saved_baud = *baud;
            }
            matches!(event, Event::BaudComplete)
        });
        assert!(saved_baud.abs_diff(921600) < 20000);
        drop(first);
        let reopened = Session::start(serial.clone(), saved_baud, true);
        reopened
            .send(Command::DataSend(b"reconnect-check".to_vec()))
            .unwrap();
        let mut echoed = false;
        let mut finished = false;
        wait(&reopened, |event| {
            match event {
                Event::UartRx(bytes) if bytes == b"reconnect-check" => echoed = true,
                Event::DataComplete => finished = true,
                _ => {}
            }
            echoed && finished
        });
        reopened.send(Command::Baud(300)).unwrap();
        wait(&reopened, |event| matches!(event, Event::BaudComplete));
        let low_rate_payload = vec![0x5a; 128];
        reopened
            .send(Command::DataSend(low_rate_payload.clone()))
            .unwrap();
        let mut low_rate_echo = false;
        let mut low_rate_complete = false;
        wait(&reopened, |event| {
            match event {
                Event::UartRx(bytes) if bytes == &low_rate_payload => low_rate_echo = true,
                Event::DataComplete => low_rate_complete = true,
                _ => {}
            }
            low_rate_echo && low_rate_complete
        });
        reopened.send(Command::Baud(115200)).unwrap();
        wait(&reopened, |event| matches!(event, Event::BaudComplete));
        drop(reopened);
        let fresh = Session::start(serial, 115200, false);
        wait(&fresh, |event| {
            matches!(event, Event::ConsoleState { framed: true, .. })
        });
        eprintln!("Immediate first send, silent discovery, 921600-baud reconnect and 128-byte echo at300baud verified");
    }
    #[test]
    #[ignore = "Requires a connected probe and harness-signals Work firmware; runs stream mode and cancels a 16 MiB send"]
    fn connected_stream_baud_and_transfer_cancel() {
        use super::{Command, Event, Session};
        use std::time::{Duration, Instant};
        fn wait(session: &Session, mut complete: impl FnMut(&Event) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let event = session
                    .rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap();
                if let Event::Error { operation, message } = &event {
                    panic!("{operation}: {message}");
                }
                if complete(&event) {
                    return;
                }
            }
        }
        let serial = std::env::var("PDBG_PROBE_SERIAL").expect("PDBG_PROBE_SERIAL");
        let mut session = Session::start(serial.clone(), 115200, false);
        session
            .send(Command::DataSend(b"stream-on".to_vec()))
            .unwrap();
        wait(&session, |event| matches!(event, Event::DataComplete));
        std::thread::sleep(Duration::from_millis(100));
        session.send(Command::Baud(921600)).unwrap();
        wait(&session, |event| matches!(event, Event::BaudComplete));
        session.send(Command::Baud(115200)).unwrap();
        wait(&session, |event| matches!(event, Event::BaudComplete));
        session
            .send(Command::DataSend(b"stream-off".to_vec()))
            .unwrap();
        wait(&session, |event| matches!(event, Event::DataComplete));
        while session.rx.try_recv().is_ok() {}
        session
            .send(Command::DataSend(vec![0xa7; 16 * 1024 * 1024]))
            .unwrap();
        wait(
            &session,
            |event| matches!(event, Event::DataProgress { sent, .. } if *sent > 0),
        );
        let started = Instant::now();
        session.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "USB worker did not stop promptly"
        );
        assert!(!session
            .rx
            .try_iter()
            .any(|event| matches!(event, Event::DataComplete)));
        drop(session);
        let reopened = Session::start(serial, 115200, true);
        let probe = b"after-cancel";
        reopened.send(Command::DataSend(probe.to_vec())).unwrap();
        let mut echoed = false;
        let mut finished = false;
        wait(&reopened, |event| {
            match event {
                Event::UartRx(bytes) if bytes == probe => echoed = true,
                Event::DataComplete => finished = true,
                _ => {}
            }
            echoed && finished
        });
        eprintln!(
            "Baud change under saturated output and bounded 16 MiB cancellation/reopen verified"
        );
    }
    #[test]
    fn parses_swd_numbers_without_silent_fallback() {
        assert_eq!(parse_u32("0x20000000").unwrap(), 0x2000_0000);
        assert_eq!(parse_u32("123").unwrap(), 123);
        assert!(parse_u32("0xGG").is_err());
    }
    #[test]
    #[ignore = "Reads the connected Work Core through SWD and temporarily halts core 0"]
    fn connected_memory_range_export() {
        use super::*;
        let serial = std::env::var("PDBG_PROBE_SERIAL").unwrap();
        let session = Session::start(serial, 115200, false);
        session
            .send(Command::SwdConnect {
                target: "rp2040".into(),
                core: 0,
            })
            .unwrap();
        session.send(Command::Swd("halt".into())).unwrap();
        session
            .send(Command::MemoryExport {
                address: 0x10000001,
                length: 257,
            })
            .unwrap();
        let mut registers = false;
        let started = Instant::now();
        loop {
            assert!(started.elapsed() < Duration::from_secs(10));
            match session.rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                Event::SwdRegisters(_) => registers = true,
                Event::MemoryExport { address, bytes } => {
                    assert_eq!(address, 0x10000001);
                    assert_eq!(bytes.len(), 257);
                    assert!(registers);
                    use ph_app::memory_export::Format;
                    for format in [Format::Binary, Format::IntelHex, Format::Csv] {
                        std::fs::write(
                            format!("/tmp/picoharness-memory.{}", format.extension()),
                            format.encode(address, &bytes),
                        )
                        .unwrap();
                    }
                    break;
                }
                Event::Error { operation, message } => panic!("{operation}: {message}"),
                _ => {}
            }
        }
        session.send(Command::Swd("resume".into())).unwrap();
        loop {
            if let Event::SwdState {
                connected: true,
                halted: false,
                ..
            } = session.rx.recv_timeout(Duration::from_secs(2)).unwrap()
            {
                break;
            }
        }
    }

    #[test]
    #[ignore = "Requires a connected RP2040 probe; temporarily halts and resumes core 0"]
    fn connected_invalid_memory_then_resume() {
        use super::*;
        let session = Session::start(std::env::var("PDBG_PROBE_SERIAL").unwrap(), 115200, false);
        session
            .send(Command::SwdConnect {
                target: "rp2040".into(),
                core: 0,
            })
            .unwrap();
        session.send(Command::Swd("halt".into())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match session
                .rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
            {
                Event::SwdRegisters(_) => break,
                Event::Error { operation, message } => panic!("{operation}: {message}"),
                _ => {}
            }
        }
        session
            .send(Command::Swd("mdw 0xfffffff0 4".into()))
            .unwrap();
        loop {
            if let Event::Error { operation, message } =
                session.rx.recv_timeout(Duration::from_secs(3)).unwrap()
            {
                assert_eq!(operation, "SWD");
                assert!(message.contains("ACK 0x04"), "{message}");
                break;
            }
        }
        session
            .send(Command::Swd("mdw 0x20000000 4".into()))
            .unwrap();
        session.send(Command::Swd("resume".into())).unwrap();
        let mut memory_recovered = false;
        loop {
            match session.rx.recv_timeout(Duration::from_secs(3)).unwrap() {
                Event::SwdMemory {
                    address: 0x20000000,
                    bytes,
                } => memory_recovered = bytes.len() == 16,
                Event::SwdState {
                    connected: true,
                    halted: false,
                    ..
                } => break,
                Event::Error { operation, message } => panic!("{operation}: {message}"),
                _ => {}
            }
        }
        assert!(memory_recovered);
    }
}
