pub mod mi;
use mi::{quote, Value};
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Config {
    pub elf: String,
    pub gdb: String,
    pub endpoint: String,
    pub managed: bool,
    pub server_only: bool,
    pub openocd: String,
    pub scripts: String,
    pub target: String,
    pub serial: String,
    pub source_from: String,
    pub source_to: String,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            elf: String::new(),
            gdb: "gdb-multiarch".into(),
            endpoint: "localhost:3333".into(),
            managed: true,
            server_only: false,
            openocd: "openocd".into(),
            scripts: String::new(),
            target: "rp2040".into(),
            serial: String::new(),
            source_from: String::new(),
            source_to: String::new(),
        }
    }
}
#[derive(Clone, Debug)]
pub enum Event {
    State {
        state: String,
        generation: u64,
    },
    Data {
        key: String,
        value: Value,
        generation: u64,
    },
    Log(String),
    OpenOcd(String),
    Error(String),
    Closed,
    ServerReady(String),
}
#[derive(Debug)]
pub enum Request {
    Execute(String),
    Breakpoint(String),
    Query {
        key: String,
        command: String,
        generation: u64,
    },
    Refresh,
    Disconnect,
}
pub struct Session {
    pub tx: Sender<Request>,
    pub rx: Receiver<Event>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Session {
    #[cfg(any(test, feature = "ui-perf"))]
    pub fn test_source() -> (Self, Sender<Event>, Receiver<Request>) {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        (
            Self {
                tx,
                rx,
                worker: None,
            },
            events,
            commands,
        )
    }

    pub fn start(config: Config) -> Self {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            if let Err(e) = run(config, commands, events.clone()) {
                let _ = events.send(Event::Error(e));
            }
            let _ = events.send(Event::Closed);
        });
        Self {
            tx,
            rx,
            worker: Some(worker),
        }
    }
    pub fn send(&self, request: Request) {
        let _ = self.tx.send(request);
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.tx.send(Request::Disconnect);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Engine {
    process: Process,
    input: ChildStdin,
    lines: Receiver<String>,
    events: Sender<Event>,
    token: u64,
    generation: u64,
    stopped: bool,
    dirty: bool,
    names: Option<Value>,
}
impl Engine {
    fn breakpoint(&mut self, command: &str) -> Result<(), String> {
        let _ = self.events.send(Event::Log(format!("> {command}\n")));
        let resume = !self.stopped;
        if resume {
            self.request("-exec-interrupt --all")?;
            let deadline = Instant::now() + Duration::from_secs(3);
            while !self.stopped {
                let line = self
                    .lines
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| "Target did not stop for breakpoint update")?;
                self.line(&line);
            }
        }
        let mutation = self.request(command);
        self.query("breakpoints", "-break-list");
        if resume {
            self.request("-exec-continue")?;
        } else {
            self.dirty = true;
        }
        mutation.map(|_| ())
    }
    fn line(&mut self, line: &str) -> Option<mi::Record> {
        match mi::parse(line) {
            Ok(Some(record)) => {
                if record.kind == '*' && (record.class == "running" || record.class == "stopped") {
                    self.generation += 1;
                    self.stopped = record.class == "stopped";
                    self.dirty = self.stopped;
                    let _ = self.events.send(Event::State {
                        state: record.class.clone(),
                        generation: self.generation,
                    });
                    if self.stopped {
                        let _ = self.events.send(Event::Data {
                            key: "stop".into(),
                            value: record.data.clone(),
                            generation: self.generation,
                        });
                    }
                }
                if "~@&".contains(record.kind) {
                    let _ = self.events.send(Event::Log(record.data.text().into()));
                }
                Some(record)
            }
            Ok(None) => None,
            Err(error) => {
                let _ = self.events.send(Event::Log(format!("MI: {error}: {line}")));
                None
            }
        }
    }
    fn request(&mut self, command: &str) -> Result<Value, String> {
        if command.contains(['\n', '\r']) {
            return Err("Commands must be a single MI record".into());
        }
        self.token += 1;
        let token = self.token;
        writeln!(self.input, "{token}{command}")
            .and_then(|_| self.input.flush())
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("GDB timeout: {command}"));
            }
            let line = self
                .lines
                .recv_timeout(left)
                .map_err(|e| format!("GDB response: {e}"))?;
            if let Some(r) = self.line(&line) {
                if r.kind == '^' && r.token == Some(token) {
                    if r.class == "running" {
                        self.stopped = false;
                        self.dirty = false;
                    }
                    return if r.class == "error" {
                        Err(r.data.field("msg").into())
                    } else {
                        Ok(r.data)
                    };
                }
            }
        }
    }
    fn query(&mut self, key: &str, cmd: &str) {
        let generation = self.generation;
        match self.request(cmd) {
            Ok(value) if generation == self.generation && self.stopped => {
                let _ = self.events.send(Event::Data {
                    key: key.into(),
                    value,
                    generation,
                });
            }
            Err(e) => {
                let _ = self.events.send(Event::Log(format!("{key}: {e}")));
            }
            _ => {}
        }
    }
    fn refresh(&mut self) {
        self.dirty = false;
        if !self.stopped {
            return;
        }
        for (key, cmd) in [
            ("threads", "-thread-info"),
            ("stack", "-stack-list-frames 0 30"),
            ("locals", "-stack-list-variables --simple-values"),
            ("registers", "-data-list-register-values x"),
            ("breakpoints", "-break-list"),
        ] {
            if !self.stopped {
                break;
            }
            self.query(key, cmd);
        }
        if !self.stopped {
            return;
        }
        if let Some(names) = self.names.clone() {
            let _ = self.events.send(Event::Data {
                key: "register_names".into(),
                value: names,
                generation: self.generation,
            });
        }
    }
}
fn reader<R: std::io::Read + Send + 'static>(input: R, tx: mpsc::SyncSender<String>) {
    thread::spawn(move || {
        for line in BufReader::new(input).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}
fn start_openocd(
    config: &Config,
    events: &Sender<Event>,
) -> Result<(ph_app_link::debug::DebugServer, String), String> {
    let mut server = ph_app_link::debug::DebugServer::start(ph_app_link::DebugConfig {
        serial: config.serial.clone(),
        target: config.target.clone(),
        openocd: config.openocd.clone(),
        scripts: config.scripts.clone(),
    })
    .map_err(|error| error.to_string())?;
    let endpoint = server.endpoint.clone();
    let (_, logs) = server.poll().map_err(|error| error.to_string())?;
    for log in logs {
        let _ = events.send(Event::OpenOcd(log));
    }
    Ok((server, endpoint))
}
pub(crate) struct Lease(String);
static LEASES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
impl Lease {
    pub(crate) fn acquire(key: String) -> Result<Self, String> {
        let mut leases = LEASES
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "Session registry unavailable")?;
        if !leases.insert(key.clone()) {
            return Err(format!("Debug resource already in use: {key}"));
        }
        Ok(Self(key))
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut leases) = LEASES.get_or_init(Default::default).lock() {
            leases.remove(&self.0);
        }
    }
}
fn run(config: Config, commands: Receiver<Request>, events: Sender<Event>) -> Result<(), String> {
    let _probe_lease = if config.serial.is_empty() {
        None
    } else {
        Some(Lease::acquire(format!("usb:{}", config.serial))?)
    };

    if config.server_only {
        let (mut server, endpoint) = start_openocd(&config, &events)?;
        let _ = events.send(Event::ServerReady(endpoint));
        loop {
            match commands.recv_timeout(Duration::from_millis(50)) {
                Ok(Request::Disconnect) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Ok(())
                }
                _ => {}
            }
            let (exited, logs) = server.poll().map_err(|error| error.to_string())?;
            for log in logs {
                let _ = events.send(Event::OpenOcd(log));
            }
            if exited {
                return Err("OpenOCD process exited".into());
            }
        }
    }

    let metadata = std::fs::metadata(&config.elf).map_err(|e| format!("ELF: {e}"))?;
    if !metadata.is_file() {
        return Err("ELF path must name a file".into());
    }
    use std::io::Read;
    let mut magic = [0u8; 4];
    std::fs::File::open(&config.elf)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map_err(|e| e.to_string())?;
    if &magic != b"\x7fELF" {
        return Err("Selected file is not ELF".into());
    }
    let mut owned = None;
    let endpoint = if config.managed {
        let (p, e) = start_openocd(&config, &events)?;
        owned = Some(p);
        e
    } else {
        config.endpoint.clone()
    };
    if !endpoint
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || ".-_:[]".contains(c))
        || endpoint
            .rsplit_once(':')
            .and_then(|(_, p)| p.parse::<u16>().ok())
            .is_none()
    {
        return Err("Invalid OpenOCD host:port".into());
    }
    let _ = events.send(Event::Log(format!(
        "Connecting to {endpoint}; symbols only, no download or reset.\n"
    )));
    let _endpoint_lease = Lease::acquire(format!(
        "tcp:{}",
        endpoint.replace("localhost:", "127.0.0.1:")
    ))?;
    let mut process = Process(
        Command::new(&config.gdb)
            .args(["--quiet", "--nx", "--interpreter=mi2"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("GDB: {e}"))?,
    );
    let input = process.0.stdin.take().unwrap();
    let (tx, lines) = mpsc::sync_channel(512);
    reader(process.0.stdout.take().unwrap(), tx.clone());
    reader(process.0.stderr.take().unwrap(), tx);
    let mut engine = Engine {
        process,
        input,
        lines,
        events,
        token: 0,
        generation: 0,
        stopped: false,
        dirty: false,
        names: None,
    };
    for cmd in [
        "-gdb-set pagination off",
        "-gdb-set confirm off",
        "-gdb-set mi-async on",
        "-gdb-set breakpoint auto-hw on",
        "-gdb-set print elements 100",
        "-gdb-set auto-load off",
    ] {
        engine.request(cmd)?;
    }
    engine.request(&format!("-file-exec-and-symbols {}", quote(&config.elf)))?;
    if !config.source_to.is_empty() {
        engine.request(&format!(
            "-environment-directory {}",
            quote(&config.source_to)
        ))?;
        if !config.source_from.is_empty() {
            engine.request(&format!(
                "-interpreter-exec console {}",
                quote(&format!(
                    "set substitute-path {} {}",
                    quote(&config.source_from),
                    quote(&config.source_to)
                ))
            ))?;
        }
    }
    let files = engine.request("-file-list-exec-source-files")?;
    let _ = engine.events.send(Event::Data {
        key: "files".into(),
        value: files,
        generation: 0,
    });
    engine.request(&format!("-target-select extended-remote {}", endpoint))?;
    engine.names = Some(engine.request("-data-list-register-names")?);
    let threads = engine.request("-thread-info")?;
    let target_state = threads
        .get("threads")
        .and_then(|v| v.items().first())
        .map(|v| v.field("state"))
        .unwrap_or("unknown");
    if target_state != "running" && target_state != "stopped" {
        return Err("GDB did not report a target execution state".into());
    }
    engine.stopped = target_state == "stopped";
    engine.dirty = engine.stopped;
    engine.generation += 1;
    let _ = engine.events.send(Event::State {
        state: target_state.into(),
        generation: engine.generation,
    });
    loop {
        while let Ok(line) = engine.lines.try_recv() {
            engine.line(&line);
        }
        if engine
            .process
            .0
            .try_wait()
            .map_err(|e| e.to_string())?
            .is_some()
        {
            return Err("GDB process exited".into());
        }
        if let Some(ref mut p) = owned {
            let (exited, logs) = p.poll().map_err(|e| e.to_string())?;
            for log in logs {
                let _ = engine.events.send(Event::OpenOcd(log));
            }
            if exited {
                return Err("OpenOCD process exited".into());
            }
        }
        match commands.recv_timeout(Duration::from_millis(20)) {
            Ok(Request::Disconnect) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if !engine.stopped {
                    let _ = engine.request("-exec-interrupt --all");
                }
                let _ = engine.request("-target-detach");
                let _ = engine.request("-gdb-exit");
                break;
            }
            Ok(Request::Execute(cmd)) => {
                let _ = engine.events.send(Event::Log(format!("> {cmd}\n")));
                let generation = engine.generation;
                if let Err(e) = engine.request(&cmd) {
                    let _ = engine.events.send(Event::Error(e));
                }
                if engine.stopped && engine.generation == generation && !cmd.starts_with("-exec-") {
                    engine.generation += 1;
                    let _ = engine.events.send(Event::State {
                        state: "stopped".into(),
                        generation: engine.generation,
                    });
                }
                engine.dirty = engine.stopped;
            }
            Ok(Request::Breakpoint(command)) => {
                if let Err(error) = engine.breakpoint(&command) {
                    let _ = engine.events.send(Event::Error(error));
                }
            }
            Ok(Request::Query {
                key,
                command,
                generation,
            }) => {
                if generation == engine.generation && engine.stopped {
                    engine.query(&key, &command);
                }
            }
            Ok(Request::Refresh) => engine.dirty = engine.stopped,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if engine.dirty {
            engine.refresh();
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn split_records_async_generations_resource_lease_and_process_exit() {
        let dir = std::env::temp_dir().join(format!("ph-app-mi-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let elf = dir.join("fixture with spaces.elf");
        std::fs::write(&elf, b"\x7fELF").unwrap();
        let script = dir.join("fake-gdb");
        std::fs::write(&script,r##"#!/usr/bin/python3
import sys,re,time
def emit(s):
    s += '\n'
    for i in range(0,len(s),3):
        sys.stdout.write(s[i:i+3]);sys.stdout.flush()
for line in sys.stdin:
    m=re.match(r'(\d+)(.*)',line);token,cmd=m.groups()
    if cmd.startswith('-target-select'):
        emit(token+'^connected');emit('*stopped,reason="signal-received"')
    elif cmd.startswith('-thread-info'):
        emit(token+'^done,threads=[{id="1",state="stopped"}]')
    elif cmd.startswith('-exec-continue'):
        emit(token+'^running');emit('*running,thread-id="all"');time.sleep(.01);emit('*stopped,reason="end-stepping-range"')
    elif cmd.startswith('-data-read-memory-bytes'):
        emit(token+'^done,memory=[{begin="0x20000000",contents="5049434f"}]')
    elif cmd.startswith('-data-list-register-names'):
        emit(token+'^done,register-names=["r0","pc"]')
    elif cmd.startswith('-data-list-register-values'):
        emit(token+'^done,register-values=[{number="0",value="0x01"}]')
    elif cmd.startswith('-file-list-exec-source-files'):
        emit(token+'^done,files=[{file="main.c",fullname="/source with space/main.c"}]')
    elif cmd=='crash': sys.exit(3)
    elif cmd.startswith('-gdb-exit'):
        emit(token+'^exit');break
    else: emit(token+'^done')
"##).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = Config {
            elf: elf.display().to_string(),
            gdb: script.display().to_string(),
            endpoint: "localhost:39999".into(),
            managed: false,
            ..Default::default()
        };
        let session = Session::start(config.clone());
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut generation = 0;
        loop {
            let e = session
                .rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            match e {
                Event::State { generation: g, .. } => generation = g,
                Event::Data { key, .. } if key == "registers" => break,
                Event::Error(e) => panic!("{e}"),
                _ => {}
            }
        }
        let duplicate = Session::start(config);
        loop {
            let e = duplicate.rx.recv_timeout(Duration::from_secs(2)).unwrap();
            if let Event::Error(s) = e {
                assert!(s.contains("already in use"));
                break;
            }
        }
        drop(duplicate);
        session.send(Request::Execute("-exec-continue".into()));
        session.send(Request::Query {
            key: "stale".into(),
            command: "-data-read-memory-bytes 0x20000000 4".into(),
            generation,
        });
        loop {
            let e = session.rx.recv_timeout(Duration::from_secs(3)).unwrap();
            match e {
                Event::Data { key, .. } if key == "stale" => panic!("stale response was accepted"),
                Event::State {
                    state,
                    generation: g,
                } if state == "stopped" && g > generation => {
                    generation = g;
                    break;
                }
                _ => {}
            }
        }
        session.send(Request::Query {
            key: "memory".into(),
            command: "-data-read-memory-bytes 0x20000000 4".into(),
            generation,
        });
        loop {
            let e = session.rx.recv_timeout(Duration::from_secs(3)).unwrap();
            match e {
                Event::Data { key, value, .. } if key == "memory" => {
                    assert_eq!(
                        value.get("memory").unwrap().items()[0].field("contents"),
                        "5049434f"
                    );
                    break;
                }
                Event::Data { key, .. } if key == "stale" => panic!("stale response was accepted"),
                _ => {}
            }
        }
        session.send(Request::Execute("crash".into()));
        loop {
            if matches!(
                session.rx.recv_timeout(Duration::from_secs(3)).unwrap(),
                Event::Closed
            ) {
                break;
            }
        }
        drop(session);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
