//! OpenOCD lifetime and bounded diagnostic output, owned by phd.
use ph_protocol::DebugConfig;
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read},
    net::TcpListener,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub(crate) fn reader(input: impl Read + Send + 'static, tx: SyncSender<String>) {
    thread::spawn(move || {
        let mut input = BufReader::new(input);
        loop {
            // A misbehaving tool may never emit a newline. Read bounded
            // fragments as diagnostics rather than allocating an unbounded line.
            let mut bytes = Vec::new();
            match input.by_ref().take(8192).read_until(b'\n', &mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&bytes)
                        .trim_end_matches(['\r', '\n'])
                        .to_owned();
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

pub struct DebugServer {
    process: Process,
    lines: Receiver<String>,
    pub port: u16,
    initial: Vec<String>,
}
impl DebugServer {
    pub fn start(config: &DebugConfig) -> Result<Self, String> {
        if config.serial.is_empty()
            || !config
                .serial
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
        {
            return Err("select a valid probe serial number".into());
        }
        if !matches!(config.target.as_str(), "rp2040" | "rp2350") {
            return Err("unsupported target".into());
        }
        let mut allocation = None;
        for base in (3333u16..10000).step_by(5) {
            if let Ok(sockets) = (base..base + 4)
                .map(|p| TcpListener::bind(("127.0.0.1", p)))
                .collect::<std::io::Result<Vec<_>>>()
            {
                allocation = Some((base, sockets));
                break;
            }
        }
        let (port, sockets) = allocation.ok_or("no free OpenOCD ports")?;
        let mut command = Command::new(&config.openocd);
        if !config.scripts.is_empty() {
            command.args(["-s", &config.scripts]);
        }
        command.args(["-f","interface/cmsis-dap.cfg","-c",&format!(
            "adapter serial {}; adapter speed 1000; bindto 127.0.0.1; gdb_port {port}; tcl_port disabled; telnet_port disabled",config.serial)]);
        if config.target == "rp2040" {
            command.args(["-c", "set USE_CORE 0"]);
        }
        command.args(["-f", &format!("target/{}.cfg", config.target), "-c", "init"]);
        drop(sockets);
        let mut process = Process(
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("OpenOCD: {e}"))?,
        );
        let (tx, rx) = mpsc::sync_channel(128);
        reader(
            process.0.stdout.take().ok_or("OpenOCD stdout missing")?,
            tx.clone(),
        );
        reader(process.0.stderr.take().ok_or("OpenOCD stderr missing")?, tx);
        let deadline = Instant::now() + Duration::from_secs(10);
        let needle = format!("Listening on port {port} for gdb");
        let mut initial = VecDeque::new();
        while Instant::now() < deadline {
            if let Ok(line) = rx.recv_timeout(Duration::from_millis(50)) {
                let ready = line.contains(&needle);
                initial.push_back(line);
                if initial.len() > 64 {
                    initial.pop_front();
                }
                if ready {
                    return Ok(Self {
                        process,
                        lines: rx,
                        port,
                        initial: initial.into_iter().collect(),
                    });
                }
            }
            if let Some(status) = process.0.try_wait().map_err(|e| e.to_string())? {
                return Err(format!(
                    "OpenOCD exited ({status}): {}",
                    initial.into_iter().collect::<Vec<_>>().join("\n")
                ));
            }
        }
        Err("OpenOCD startup timed out".into())
    }
    pub fn poll(&mut self) -> Result<(bool, Vec<String>), String> {
        let mut logs = std::mem::take(&mut self.initial);
        logs.extend(self.lines.try_iter().take(128));
        Ok((
            self.process
                .0
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_some(),
            logs,
        ))
    }
}

/// Bounded, cancellable external tooling. Dropping Process always kills and
/// reaps the child, including validation failures and daemon shutdown.
pub fn run_tool(command: &mut Command, cancel: &AtomicBool) -> Result<(), String> {
    let mut process = Process(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?,
    );
    let (tx, rx) = mpsc::sync_channel(64);
    reader(
        process.0.stdout.take().ok_or("tool stdout missing")?,
        tx.clone(),
    );
    reader(process.0.stderr.take().ok_or("tool stderr missing")?, tx);
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut recent = VecDeque::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("operation cancelled".into());
        }
        if Instant::now() > deadline {
            return Err("external tool timed out".into());
        }
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(20)) {
            recent.push_back(line);
            if recent.len() > 32 {
                recent.pop_front();
            }
        }
        if let Some(status) = process.0.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!(
                    "tool failed ({status}): {}",
                    recent.into_iter().collect::<Vec<_>>().join("\n")
                ))
            };
        }
    }
}
