//! Per-device UI state. Measurements and operations come from the USB worker.
use crate::{
    gdb::{self, mi::Value},
    live,
    logic::Logic,
};
use std::collections::{BTreeMap, VecDeque};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Overview,
    Console,
    Swd,
    Gdb,
    Logic,
    Firmware,
}
impl Page {
    pub const ALL: [Self; 6] = [
        Self::Overview,
        Self::Console,
        Self::Swd,
        Self::Gdb,
        Self::Logic,
        Self::Firmware,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Console => "Console",
            Self::Swd => "SWD",
            Self::Gdb => "GDB",
            Self::Logic => "Logic",
            Self::Firmware => "Firmware",
        }
    }
}
#[derive(Clone)]
pub struct Log {
    pub time: f64,
    pub direction: String,
    pub text: String,
    pub raw: Option<Vec<u8>>,
}
pub struct Trace {
    pub time: f64,
    pub bus: &'static str,
    pub direction: &'static str,
    pub bytes: Vec<u8>,
    pub decoded: String,
}
#[derive(Clone, Copy, PartialEq)]
pub enum TaskKind {
    Send,
    Receive,
}
pub struct Task {
    pub kind: TaskKind,
    pub started: f64,
    pub finished_at: Option<f64>,
    pub progress: f32,
    pub status: String,
    pub total: usize,
    pub transferred: usize,
}
impl Task {
    pub fn finish(&mut self, status: &str, now: f64) {
        self.status = status.into();
        self.finished_at.get_or_insert(now);
    }

    pub fn speed_kb_s(&self, now: f64) -> f64 {
        let elapsed = self.finished_at.unwrap_or(now) - self.started;
        self.transferred as f64 / elapsed.max(0.1) / 1000.
    }
}
pub struct DebugView {
    pub config: gdb::Config,
    pub session: Option<gdb::Session>,
    pub state: String,
    pub generation: u64,
    pub data: BTreeMap<String, Value>,
    pub logs: VecDeque<String>,
    pub openocd: VecDeque<String>,
    pub error: String,
    pub source: String,
    pub source_path: String,
    pub source_line: usize,
    pub follow: bool,
    pub source_search: String,
    pub console: String,
    pub bottom: usize,
    pub inspector: usize,
    pub watches: Vec<String>,
    pub watch_input: String,
    pub memory_address: String,
    pub memory_count: usize,
    pub memory_write: String,
    pub register_index: String,
    pub register_write: String,
    pub expression: String,
    pub assignment: String,
    pub condition: String,
    pub previous_registers: BTreeMap<String, String>,
    pub changed_registers: Vec<String>,
    pub variable_ids: Vec<String>,
}
impl Default for DebugView {
    fn default() -> Self {
        Self {
            config: Default::default(),
            session: None,
            state: "Disconnected".into(),
            generation: 0,
            data: BTreeMap::new(),
            logs: VecDeque::new(),
            openocd: VecDeque::new(),
            error: String::new(),
            source: String::new(),
            source_path: String::new(),
            source_line: 0,
            follow: true,
            source_search: String::new(),
            console: String::new(),
            bottom: 0,
            inspector: 0,
            watches: vec![],
            watch_input: String::new(),
            memory_address: "0x20000000".into(),
            memory_count: 64,
            memory_write: String::new(),
            register_index: "0".into(),
            register_write: String::new(),
            expression: String::new(),
            assignment: String::new(),
            condition: String::new(),
            previous_registers: BTreeMap::new(),
            changed_registers: vec![],
            variable_ids: vec![],
        }
    }
}
impl DebugView {
    pub fn start(&mut self) {
        self.error.clear();
        self.data.clear();
        self.source.clear();
        self.source_path.clear();
        self.generation = 0;
        self.follow = true;
        self.state = "Connecting".into();
        self.session = Some(gdb::Session::start(self.config.clone()));
    }
    pub fn breakpoint(&self, command: String) {
        if let Some(session) = &self.session {
            session.send(gdb::Request::Breakpoint(command));
        }
    }
    pub fn apply_stack(&mut self, value: &Value) {
        if self.follow {
            if let Some(frame) = value.get("stack").and_then(|v| v.items().first()) {
                self.focus_frame(frame.get("frame").unwrap_or(frame));
            }
        }
    }
    pub fn execute(&self, cmd: impl Into<String>) {
        if let Some(s) = &self.session {
            s.send(gdb::Request::Execute(cmd.into()));
        }
    }
    pub fn console_command(&self, cmd: &str) {
        self.execute(format!("-interpreter-exec console {}", gdb::mi::quote(cmd)));
    }
    pub fn query(&self, key: &str, cmd: String) {
        if let Some(s) = &self.session {
            s.send(gdb::Request::Query {
                key: key.into(),
                command: cmd,
                generation: self.generation,
            });
        }
    }
    pub fn memory_read_count(&self) -> usize {
        let text = self.memory_address.trim();
        let address = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            Some(hex) => u64::from_str_radix(hex, 16),
            None => text.parse(),
        };
        // Match SRAM_BASE/SRAM_END in the Pico SDK addressmap.h for each target.
        // Crossing this boundary makes some OpenOCD versions return zeros for
        // the entire read, including the valid bytes below the boundary.
        let ram_end = match self.config.target.as_str() {
            "rp2040" => 0x2004_2000,
            "rp2350" => 0x2008_2000,
            _ => return self.memory_count,
        };
        match address {
            Ok(address) if (0x2000_0000..ram_end).contains(&address) => {
                self.memory_count.min((ram_end - address) as usize)
            }
            _ => self.memory_count,
        }
    }
    pub fn read_memory(&self) {
        self.query(
            "memory",
            format!(
                "-data-read-memory-bytes {} {}",
                gdb::mi::quote(self.memory_address.trim()),
                self.memory_read_count()
            ),
        );
    }
    pub fn refresh(&self) {
        if let Some(session) = &self.session {
            session.send(gdb::Request::Refresh);
            self.read_memory();
        }
    }
    pub fn field(&self, key: &str, field: &str) -> &[Value] {
        self.data
            .get(key)
            .and_then(|v| v.get(field))
            .map(Value::items)
            .unwrap_or(&[])
    }
    pub fn read_source(&mut self, path: &str) {
        let resolved = if !self.config.source_from.is_empty() {
            path.strip_prefix(&self.config.source_from)
                .map(|suffix| format!("{}{suffix}", self.config.source_to))
                .unwrap_or(path.into())
        } else {
            path.into()
        };
        let p = std::path::Path::new(&resolved);
        let fallback =
            std::path::Path::new(&self.config.source_to).join(p.file_name().unwrap_or_default());
        let file = if p.exists() {
            p.to_path_buf()
        } else {
            fallback
        };
        self.source_path = path.into();
        self.source = std::fs::read_to_string(file).unwrap_or_default();
        if self.source.len() > 2_000_000 {
            self.source
                .truncate(self.source.floor_char_boundary(2_000_000));
        }
    }
    pub fn poll(&mut self) -> Vec<String> {
        let mut audit = vec![];
        let mut received = vec![];
        if let Some(s) = &self.session {
            received.extend(s.rx.try_iter().take(1000));
        }
        for event in received {
            match event {
                gdb::Event::State { state, generation } => {
                    self.state = state.clone();
                    self.generation = generation;
                    audit.push(format!("GDB {state}"));
                    if state == "stopped" {
                        let ids = std::mem::take(&mut self.variable_ids);
                        for id in ids {
                            self.query(
                                "var_delete",
                                format!("-var-delete {}", gdb::mi::quote(&id)),
                            );
                        }
                        self.data.retain(|key, _| {
                            !key.starts_with("var:") && !key.starts_with("children:")
                        });
                        self.read_memory();
                        for (i, w) in self.watches.iter().enumerate() {
                            self.query(
                                &format!("watch:{i}"),
                                format!("-data-evaluate-expression {}", gdb::mi::quote(w)),
                            );
                        }
                    }
                }
                gdb::Event::Data {
                    key,
                    value,
                    generation,
                } => {
                    if key != "files" && generation != self.generation {
                        continue;
                    }
                    if key == "stack" {
                        self.apply_stack(&value);
                    }
                    if key == "registers" {
                        self.changed_registers.clear();
                        for reg in value
                            .get("register-values")
                            .map(Value::items)
                            .unwrap_or(&[])
                        {
                            let name = reg.field("number").to_string();
                            let val = reg.field("value").to_string();
                            if self
                                .previous_registers
                                .get(&name)
                                .is_some_and(|v| v != &val)
                            {
                                self.changed_registers.push(name.clone());
                            }
                            self.previous_registers.insert(name, val);
                        }
                    }
                    if key.starts_with("var:") && !value.field("name").is_empty() {
                        self.variable_ids.push(value.field("name").into());
                    }
                    self.data.insert(key, value);
                }
                gdb::Event::Log(s) => {
                    if !s.trim().is_empty() {
                        audit.push(s.clone());
                        push(&mut self.logs, s, 2000);
                    }
                }
                gdb::Event::OpenOcd(s) => push(&mut self.openocd, s, 1000),
                gdb::Event::Error(e) => {
                    self.error = e.clone();
                    audit.push(e.clone());
                    push(&mut self.logs, format!("ERROR: {e}"), 2000);
                }
                gdb::Event::Closed => {
                    self.state = "Disconnected".into();
                    self.session = None;
                    audit.push("GDB disconnected; previous values are stale".into());
                }
                gdb::Event::ServerReady(endpoint) => {
                    self.config.endpoint = endpoint;
                    self.state = "Server running".into();
                }
            }
        }
        audit
    }
    pub fn focus_frame(&mut self, frame: &Value) {
        self.source_line = frame.field("line").parse().unwrap_or(0);
        let path = frame.field("fullname");
        let path = if path.is_empty() {
            frame.field("file")
        } else {
            path
        };
        if !path.is_empty() && self.source_path != path {
            self.read_source(path);
        }
        if self.source.is_empty() {
            self.query(
                "assembly",
                "-data-disassemble -s \"$pc\" -e \"$pc+128\" -- 0".into(),
            );
        }
    }
}
pub fn push<T>(queue: &mut VecDeque<T>, v: T, max: usize) {
    queue.push_back(v);
    while queue.len() > max {
        queue.pop_front();
    }
}
pub struct FirmwareView {
    pub job: Option<picoharness_desktop::firmware::Job>,
    pub status: String,
    pub error: String,
    pub logs: VecDeque<String>,
    pub format: String,
    pub bytes: usize,
}
impl Default for FirmwareView {
    fn default() -> Self {
        Self {
            job: None,
            status: "Ready".into(),
            error: String::new(),
            logs: VecDeque::new(),
            format: String::new(),
            bytes: 0,
        }
    }
}

pub struct Device {
    pub id: String,
    pub name: String,
    pub online: bool,
    pub page: Page,
    pub power: bool,
    pub running: bool,
    pub swd: bool,
    pub core: usize,
    pub fault: bool,
    pub heartbeat: bool,
    pub voltage: f64,
    pub current: f64,
    pub watts: f64,
    pub tick_at: f64,
    pub monitor: picoharness_desktop::telemetry::Monitor,
    pub logs: VecDeque<Log>,
    pub events: VecDeque<Log>,
    pub audit: Vec<Log>,
    pub rx: u64,
    pub tx: u64,
    pub errors: u32,
    pub input: String,
    pub history: crate::command_history::CommandHistory,
    pub search: String,
    pub hex: bool,
    pub paused: bool,
    pub newline: usize,
    pub console_framed: bool,
    pub work_verified: bool,
    pub work_bulk_ready: bool,
    pub baud: u32,
    pub baud_draft: u32,
    pub baud_pending: bool,
    pub trace_open: bool,
    pub trace: VecDeque<Trace>,
    pub trace_size: usize,
    trace_decoded_size: usize,
    trace_tx: crate::work_link::TraceDecoder,
    trace_rx: crate::work_link::TraceDecoder,
    pub received: Vec<u8>,
    pub task: Option<Task>,
    pub firmware: String,
    pub flash: FirmwareView,
    pub debug_update_job: Option<picoharness_desktop::debug_update::Job>,
    pub debug_image: String,
    pub picotool: String,
    pub debug_update_status: String,
    pub reset_after: bool,
    pub swd_tab: usize,
    pub swd_input: String,
    pub swd_logs: VecDeque<String>,
    pub registers: [u32; 17],
    pub previous: [u32; 17],
    pub core_registers: [[u32; 17]; 2],
    pub registers_valid: [bool; 2],
    pub reg_drafts: [String; 17],
    pub reg_search: String,
    pub mem_address: String,
    pub mem_read_length: usize,
    pub memory: Vec<u8>,
    pub mem_edit: String,
    pub mem_offset: usize,
    pub mem_export_length: usize,
    pub mem_export_format: picoharness_desktop::memory_export::Format,
    pub mem_export_pending: Option<(
        std::path::PathBuf,
        picoharness_desktop::memory_export::Format,
    )>,
    pub logic: Logic,
    pub debugger: DebugView,
    pub live_session: Option<live::Session>,
    pub live_ready: bool,
    pub live_info: String,
    pub live_error: String,
    pub live_retry_at: f64,
    pub swd_idcode: u32,
    pub swd_pending: bool,
    pub live_memory_address: Option<u32>,
}
impl Device {
    pub fn base(id: String, name: String) -> Self {
        Self {
            id,
            name,
            online: false,
            page: Page::Overview,
            power: false,
            running: false,
            swd: false,
            core: 0,
            fault: false,
            heartbeat: false,
            voltage: 0.,
            current: 0.,
            watts: 0.,
            tick_at: 0.,
            monitor: Default::default(),
            logs: VecDeque::new(),
            events: VecDeque::new(),
            audit: vec![],
            rx: 0,
            tx: 0,
            errors: 0,
            input: String::new(),
            history: Default::default(),
            search: String::new(),
            hex: false,
            paused: false,
            newline: 1,
            console_framed: false,
            work_verified: false,
            work_bulk_ready: false,
            baud: 115200,
            baud_draft: 115200,
            baud_pending: false,
            trace_open: false,
            trace: VecDeque::new(),
            trace_size: 0,
            trace_decoded_size: 0,
            trace_tx: Default::default(),
            trace_rx: Default::default(),
            received: vec![],
            task: None,
            firmware: String::new(),
            flash: FirmwareView::default(),
            debug_update_job: None,
            debug_image: String::new(),
            picotool: "picotool".into(),
            debug_update_status: String::new(),
            reset_after: true,
            swd_tab: 0,
            swd_input: String::new(),
            swd_logs: VecDeque::new(),
            registers: [0; 17],
            previous: [0; 17],
            core_registers: [[0; 17]; 2],
            registers_valid: [false; 2],
            reg_drafts: std::array::from_fn(|_| String::new()),
            reg_search: String::new(),
            mem_address: "0x20000000".into(),
            mem_read_length: 256,
            memory: Vec::new(),
            mem_edit: String::new(),
            mem_offset: 0,
            mem_export_length: 256,
            mem_export_format: Default::default(),
            mem_export_pending: None,
            logic: Default::default(),
            debugger: Default::default(),
            live_session: None,
            live_ready: false,
            live_info: String::new(),
            live_error: String::new(),
            live_retry_at: 0.,
            swd_idcode: 0,
            swd_pending: false,
            live_memory_address: None,
        }
    }
    pub fn start_firmware(&mut self, now: f64) -> Result<(), String> {
        use picoharness_desktop::firmware::{Config, Job, Target};
        let start = || -> Result<Config, String> {
            if self.flash.job.is_some()
                || self.debug_update_job.is_some()
                || self.task.as_ref().is_some_and(|t| t.status == "Running")
            {
                return Err("A device task is already running".into());
            }
            if !self.online {
                return Err("The selected device is disconnected".into());
            }
            if self.debugger.session.is_some() {
                return Err("Disconnect GDB before programming this device".into());
            }
            if self.swd || self.swd_pending {
                return Err("Disconnect SWD before programming this device".into());
            }
            let target = Target::parse(&self.debugger.config.target)?;
            if self.debugger.config.serial.is_empty() {
                return Err(
                    "Select a USB probe with a valid serial number to flash its Work Core".into(),
                );
            }
            Ok(Config {
                file: self.firmware.clone().into(),
                serial: self.debugger.config.serial.clone(),
                target,
                openocd: self.debugger.config.openocd.clone(),
                scripts: self.debugger.config.scripts.clone(),
                reset_after: self.reset_after,
            })
        };
        match start() {
            Ok(config) => {
                self.stop_live(now);
                self.flash = FirmwareView::default();
                self.flash.status = "Validating".into();
                self.flash.job = Some(Job::start(config));
                self.event(now, "Firmware programming started");
                Ok(())
            }
            Err(error) => {
                if self.flash.job.is_none() {
                    self.flash.status = "Failed".into();
                    self.flash.error = error.clone();
                }
                Err(error)
            }
        }
    }
    pub fn cancel_firmware(&mut self) {
        if let Some(job) = &self.flash.job {
            job.cancel();
            self.flash.status = "Cancelling".into();
        }
    }
    fn poll_firmware(&mut self, now: f64) {
        use picoharness_desktop::firmware::Event;
        let events: Vec<_> = self
            .flash
            .job
            .as_ref()
            .map(|job| job.rx.try_iter().take(1000).collect())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Phase(phase) => {
                    if self.flash.status != "Cancelling" {
                        self.flash.status = phase.into();
                    }
                    self.event(now, &format!("Firmware: {phase}"));
                }
                Event::Log(line) => {
                    push(&mut self.flash.logs, line.clone(), 2000);
                    self.event(now, &line);
                }
                Event::Image { format, bytes } => {
                    self.flash.format = format.into();
                    self.flash.bytes = bytes;
                }
                Event::Done(result) => {
                    self.flash.job = None;
                    match result {
                        Ok(()) => {
                            self.baud = 115200;
                            self.baud_draft = 115200;
                            self.console_framed = false;
                            self.flash.status = "Complete".into();
                            self.event(now, "Firmware verified successfully");
                        }
                        Err(error) => {
                            self.errors += 1;
                            self.flash.status = if self.flash.status == "Cancelling" {
                                "Cancelled"
                            } else {
                                "Failed"
                            }
                            .into();
                            self.flash.error = error.clone();
                            self.event(now, &error);
                        }
                    }
                }
            }
        }
    }
    pub fn accept_firmware_drop(
        &mut self,
        files: &[std::path::PathBuf],
        now: f64,
    ) -> Result<(), String> {
        if files.len() != 1 {
            return Err("Drop exactly one UF2, ELF or BIN file".into());
        }
        let path = &files[0];
        if !path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            ["uf2", "elf", "bin"]
                .iter()
                .any(|extension| e.eq_ignore_ascii_case(extension))
        }) {
            return Err("Drop a UF2, ELF or BIN firmware file".into());
        }
        self.page = Page::Firmware;
        if self.flash.job.is_some() || self.task.as_ref().is_some_and(|t| t.status == "Running") {
            return Err("A device task is already running".into());
        }
        self.firmware = path.to_string_lossy().into_owned();
        self.start_firmware(now)
    }
    pub fn accept_file_drop(
        &mut self,
        files: &[std::path::PathBuf],
        now: f64,
    ) -> Result<&'static str, String> {
        let debug_elf = self.page != Page::Firmware
            && files.first().is_some_and(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extension.eq_ignore_ascii_case("elf")
                            || extension.eq_ignore_ascii_case("axf")
                    })
            });
        if !debug_elf {
            self.accept_firmware_drop(files, now)?;
            return Ok("Firmware programming started");
        }
        if files.len() != 1 {
            return Err("Drop exactly one ELF file".into());
        }
        if !self.online || self.debugger.config.serial.is_empty() {
            return Err("Connect a device before debugging".into());
        }
        if self.flash.job.is_some()
            || self.debug_update_job.is_some()
            || self.debugger.session.is_some()
            || self
                .task
                .as_ref()
                .is_some_and(|task| task.status == "Running")
        {
            return Err("A device task is already running".into());
        }
        use std::io::Read;
        let mut magic = [0u8; 4];
        std::fs::File::open(&files[0])
            .and_then(|mut file| file.read_exact(&mut magic))
            .map_err(|error| format!("ELF: {error}"))?;
        if &magic != b"\x7fELF" {
            return Err("Selected file is not ELF".into());
        }
        self.stop_live(now);
        self.page = Page::Gdb;
        self.debugger.config.elf = files[0].display().to_string();
        self.debugger.config.managed = true;
        self.debugger.config.server_only = false;
        self.debugger.start();
        Ok("GDB debugging started")
    }
    pub fn event(&mut self, now: f64, text: &str) {
        let e = Log {
            time: now,
            direction: "EVENT".into(),
            text: text.into(),
            raw: None,
        };
        push(&mut self.events, e.clone(), 500);
        self.audit.push(e);
    }
    pub fn serial_bytes(&mut self, now: f64, dir: &str, bytes: Vec<u8>) {
        if dir == "RX" {
            self.rx += bytes.len() as u64;
            self.received.extend_from_slice(&bytes);
            if self.received.len() > 16 * 1024 * 1024 {
                self.received
                    .drain(..self.received.len() - 16 * 1024 * 1024);
            }
        } else {
            self.tx += bytes.len() as u64;
        }
        let e = Log {
            time: now,
            direction: dir.into(),
            text: String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).into_owned(),
            raw: Some(bytes[..bytes.len().min(4096)].to_vec()),
        };
        push(&mut self.logs, e.clone(), 5000);
        self.audit.push(e);
    }
    pub fn state(&self) -> &str {
        if self.flash.job.is_some() {
            return &self.flash.status;
        }
        if self.debugger.session.is_some() {
            return &self.debugger.state;
        }
        if !self.online {
            "Disconnected"
        } else if !self.live_error.is_empty() && !self.live_ready {
            "Connection error"
        } else if !self.live_ready || self.swd_pending {
            "Connecting"
        } else if self.fault {
            "Power fault"
        } else if !self.power {
            "Power off"
        } else if self.swd && !self.running {
            "Halted"
        } else {
            "Power on"
        }
    }
    pub fn measurement_fresh(&self, now: f64) -> bool {
        self.online
            && self.live_ready
            && self
                .monitor
                .live
                .back()
                .is_some_and(|s| now - s.elapsed_s <= 2.)
    }
    pub fn available(&self) -> bool {
        self.flash.job.is_none() && self.online && self.live_ready && self.power && !self.fault
    }
    pub fn tick(&mut self, now: f64) {
        self.poll_firmware(now);
        self.poll_live(now);
        for log in self.debugger.poll() {
            self.event(now, &log);
        }
    }

    pub fn start_live_data_send(&mut self, bytes: Vec<u8>, now: f64) {
        let total = bytes.len();
        self.start_live_data(TaskKind::Send, total, live::Command::DataSend(bytes), now);
    }

    pub fn start_live_data_receive(&mut self, maximum: usize, now: f64) {
        self.start_live_data(
            TaskKind::Receive,
            maximum,
            live::Command::DataReceive(maximum),
            now,
        );
    }

    fn start_live_data(&mut self, kind: TaskKind, total: usize, command: live::Command, now: f64) {
        self.task = Some(Task {
            kind,
            started: now,
            finished_at: None,
            progress: 0.,
            status: "Running".into(),
            total,
            transferred: 0,
        });
        if let Err(error) = self.request_live(command) {
            if let Some(task) = &mut self.task {
                task.finish("Failed", now);
            }
            self.live_error = error;
        }
    }

    pub fn request_live(&self, command: live::Command) -> Result<(), String> {
        self.live_session
            .as_ref()
            .ok_or("USB session is connecting")?
            .send(command)
    }

    pub fn stop_live(&mut self, now: f64) {
        if let Some(mut session) = self.live_session.take() {
            session.shutdown();
            for event in session.rx.try_iter() {
                if let live::Event::ConsoleState { baud, framed } = event {
                    self.baud = baud;
                    self.console_framed = framed;
                }
            }
        }
        if let Some(task) = &mut self.task {
            if task.status == "Running" {
                task.finish("Cancelled", now);
            }
        }
        self.live_ready = false;
        self.swd = false;
        self.swd_pending = false;
        self.baud_pending = false;
        self.baud_draft = crate::pages::baud_preset(self.baud);
        self.mem_export_pending = None;
        self.logic.capturing = None;
        self.trace_tx = Default::default();
        self.trace_rx = Default::default();
    }

    fn poll_live(&mut self, now: f64) {
        let updates: Vec<_> = self
            .debug_update_job
            .as_ref()
            .map(|job| job.rx.try_iter().collect())
            .unwrap_or_default();
        for update in updates {
            use picoharness_desktop::debug_update::Event;
            match update {
                Event::Phase(text) => self.debug_update_status = text,
                Event::Done(result) => {
                    self.debug_update_job = None;
                    match result {
                        Ok(info) => {
                            self.baud = 115200;
                            self.baud_draft = 115200;
                            self.console_framed = false;
                            self.debug_update_status = "Debug Core update complete".into();
                            self.live_info = info;
                        }
                        Err(error) => self.debug_update_status = error,
                    }
                    self.live_retry_at = now + 0.5;
                }
            }
        }
        if self.flash.job.is_some() || self.debug_update_job.is_some() {
            return;
        }
        let mut disconnected = false;
        let mut events = Vec::new();
        if let Some(session) = &self.live_session {
            loop {
                match session.rx.try_recv() {
                    Ok(event) => events.push(event),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for event in events {
            match event {
                live::Event::Ready(info) => {
                    self.live_info = info;
                    self.live_ready = true;
                    self.live_error.clear();
                    self.event(now, "USB probe ready");
                }
                live::Event::Status(status) => {
                    if self.power != status.power_on {
                        self.event(
                            now,
                            if status.power_on {
                                "Work power enabled"
                            } else {
                                "Work power disabled"
                            },
                        );
                    }
                    self.power = status.power_on;
                    self.fault = status.power_fault;
                    self.heartbeat = status.power_good;
                    self.voltage = status.work_3v3_mv as f64 / 1000.;
                    if !status.power_good {
                        self.swd = false;
                    }
                }
                live::Event::Measurement {
                    value,
                    captured_at,
                    timestamp_unix_ms,
                } => {
                    if self.live_error.starts_with("INA237:") {
                        self.live_error.clear();
                    }
                    self.current = value.current_ma;
                    self.watts = value.power_mw;
                    self.tick_at = (now - captured_at.elapsed().as_secs_f64()).max(0.);
                    self.monitor.push(picoharness_desktop::telemetry::Sample {
                        timestamp_unix_ms,
                        elapsed_s: self.tick_at,
                        bus_voltage_v: value.bus_mv / 1000.,
                        work_3v3_v: self.voltage,
                        current_ma: value.current_ma,
                        power_mw: value.power_mw,
                        power_on: self.power,
                        power_fault: self.fault,
                    });
                }
                live::Event::MeasurementError(message) => {
                    self.live_error = format!("INA237: {message}");
                }
                live::Event::UartRx(bytes) => self.serial_bytes(now, "RX", bytes),
                live::Event::UartTx(bytes) => self.serial_bytes(now, "TX", bytes),
                live::Event::ConsoleState { framed, baud } => {
                    self.console_framed = framed;
                    let draft_matches_previous =
                        self.baud_draft == crate::pages::baud_preset(self.baud);
                    self.baud = baud;
                    if !self.baud_pending && draft_matches_previous {
                        self.baud_draft = crate::pages::baud_preset(baud);
                    }
                }
                live::Event::WorkSupport {
                    verified,
                    bulk_ready,
                } => {
                    self.work_verified = verified;
                    self.work_bulk_ready = bulk_ready;
                }
                live::Event::BaudComplete => {
                    self.baud_draft = crate::pages::baud_preset(self.baud);
                    self.baud_pending = false;
                }
                live::Event::Trace {
                    bus,
                    direction,
                    bytes,
                } => {
                    let decoder = if direction == "TX" {
                        &mut self.trace_tx
                    } else {
                        &mut self.trace_rx
                    };
                    let decoded = decoder.describe(bus, &bytes);
                    self.trace_size += bytes.len();
                    self.trace_decoded_size += decoded.len();
                    self.trace.push_back(Trace {
                        time: now,
                        bus,
                        direction,
                        bytes,
                        decoded,
                    });
                    while self.trace_size > 2 * 1024 * 1024
                        || self.trace_decoded_size > 2 * 1024 * 1024
                        || self.trace.len() > 4000
                    {
                        if let Some(trace) = self.trace.pop_front() {
                            self.trace_size -= trace.bytes.len();
                            self.trace_decoded_size -= trace.decoded.len();
                        }
                    }
                }
                live::Event::Capture {
                    samples,
                    rate,
                    trigger_index,
                } => {
                    if self.logic.capturing.is_some() {
                        self.logic.set_capture(samples, rate);
                        self.logic.trigger_index = trigger_index;
                        self.logic.capturing = None;
                        self.event(now, "Logic capture complete");
                    }
                }
                live::Event::SwdState {
                    connected,
                    halted,
                    idcode,
                } => {
                    if connected {
                        self.swd_pending = false;
                    }
                    self.swd = connected;
                    self.running = connected && !halted;
                    if idcode != 0 {
                        self.swd_idcode = idcode;
                    }
                    self.event(
                        now,
                        if connected {
                            "SWD connected"
                        } else {
                            "SWD disconnected"
                        },
                    );
                }
                live::Event::SwdRegisters(values) => {
                    self.previous = self.registers;
                    self.registers = values;
                    self.core_registers[self.core] = values;
                    self.registers_valid[self.core] = true;
                    self.reg_drafts = std::array::from_fn(|i| format!("{:08X}", values[i]));
                }
                live::Event::SwdMemory { address, bytes } => {
                    self.live_memory_address = Some(address);
                    self.memory = bytes;
                }
                live::Event::MemoryExport { address, bytes } => {
                    if let Some((path, format)) = self.mem_export_pending.take() {
                        match std::fs::write(&path, format.encode(address, &bytes)) {
                            Ok(()) => self.event(
                                now,
                                &format!(
                                    "Memory exported: {} bytes to {}",
                                    bytes.len(),
                                    path.display()
                                ),
                            ),
                            Err(error) => self.live_error = format!("Memory export: {error}"),
                        }
                    }
                }
                live::Event::SwdLog(line) => push(&mut self.swd_logs, line, 1000),
                live::Event::DataProgress { sent, total } => {
                    if let Some(task) = &mut self.task {
                        task.transferred = sent;
                        task.total = total;
                        task.progress = if total == 0 {
                            0.
                        } else {
                            sent as f32 / total as f32
                        };
                    }
                }
                live::Event::DataReceived(bytes) => {
                    self.rx += bytes.len() as u64;
                    self.received = bytes;
                }
                live::Event::DataComplete => {
                    if let Some(task) = &mut self.task {
                        task.finish("Complete", now);
                        task.progress = 1.;
                    }
                    self.event(now, "Data transfer complete");
                }
                live::Event::Error { operation, message } => {
                    if operation == "Baud" {
                        self.baud_pending = false;
                        self.baud_draft = crate::pages::baud_preset(self.baud);
                    }
                    if operation == "Memory export" {
                        self.mem_export_pending = None;
                    }
                    self.errors += 1;
                    self.live_error = format!("{operation}: {message}");
                    self.event(now, &self.live_error.clone());
                    if operation == "Data" {
                        if let Some(task) = &mut self.task {
                            task.finish("Failed", now);
                        }
                    }
                    if operation == "Logic" {
                        self.logic.capturing = None;
                    }
                    if operation.starts_with("SWD") {
                        push(&mut self.swd_logs, self.live_error.clone(), 1000);
                    }
                    if operation == "SWD connect" {
                        self.swd_pending = false;
                        self.swd = false;
                    }
                }
            }
        }
        if disconnected {
            self.stop_live(now);
            self.online = false;
            self.live_retry_at = now + 2.;
        }
        if self.live_session.is_none()
            && self.online
            && self.debugger.session.is_none()
            && !self.debugger.config.serial.is_empty()
            && now >= self.live_retry_at
        {
            self.live_session = Some(live::Session::start(
                self.debugger.config.serial.clone(),
                self.baud,
                self.console_framed,
            ));
            self.live_retry_at = now + 2.;
        }
    }
}

#[cfg(test)]
mod firmware_drop_tests {
    use super::*;
    #[test]
    fn elf_drop_is_distinct_from_firmware_programming() {
        let mut device = Device::base("test".into(), "Probe".into());
        assert!(device
            .accept_file_drop(&["image.elf".into()], 0.)
            .unwrap_err()
            .contains("Connect"));
        assert!(device.page == Page::Overview);
        assert!(device.flash.job.is_none());
        assert!(device.debugger.session.is_none());
        device.page = Page::Firmware;
        assert!(device
            .accept_file_drop(&["image.elf".into()], 0.)
            .unwrap_err()
            .contains("disconnected"));
        assert!(device.page == Page::Firmware);
        assert!(device
            .accept_file_drop(&["a.elf".into(), "b.elf".into()], 0.)
            .is_err());
        assert!(device.debugger.session.is_none());
        assert!(gdb::Config::default().managed);
    }
    #[test]
    fn gdb_follow_accepts_an_unwrapped_stack_frame() {
        let path = std::env::temp_dir().join("picoharness-follow.c");
        std::fs::write(&path, "int main(void) {\n    return 0;\n}\n").unwrap();
        let record = gdb::mi::parse(&format!(
            "^done,stack=[{{level=\"0\",fullname={},line=\"2\"}}]",
            gdb::mi::quote(&path.display().to_string())
        ))
        .unwrap()
        .unwrap();
        let mut view = DebugView::default();
        view.apply_stack(&record.data);
        assert_eq!(view.source_line, 2);
        assert!(!view.source.is_empty());
        std::fs::remove_file(path).unwrap();
    }
    #[cfg(feature = "ui-perf")]
    #[test]
    fn gdb_memory_read_stops_at_the_selected_targets_ram_end() {
        let mut view = DebugView::default();
        let (session, _, commands) = gdb::Session::test_source();
        view.session = Some(session);
        view.memory_address = " 0x20041FD0 ".into();
        view.read_memory();
        assert!(
            matches!(commands.try_recv().unwrap(), gdb::Request::Query { command, .. }
            if command == "-data-read-memory-bytes \"0x20041FD0\" 48")
        );
        view.config.target = "rp2350".into();
        view.memory_address = "0X20081FF0".into();
        view.read_memory();
        assert!(
            matches!(commands.try_recv().unwrap(), gdb::Request::Query { command, .. }
            if command == "-data-read-memory-bytes \"0X20081FF0\" 16")
        );
        view.memory_address = "0x10000100".into();
        view.read_memory();
        assert!(
            matches!(commands.try_recv().unwrap(), gdb::Request::Query { command, .. }
            if command == "-data-read-memory-bytes \"0x10000100\" 64")
        );
    }
    #[test]
    fn console_status_does_not_erase_a_baud_edit() {
        let mut device = Device::base("test".into(), "Probe".into());
        let (session, events) = live::Session::performance_source();
        device.live_session = Some(session);
        device.baud_draft = 921600;
        events
            .send(live::Event::ConsoleState {
                framed: true,
                baud: 115200,
            })
            .unwrap();
        device.tick(1.);
        assert_eq!(
            device.baud_draft, 921600,
            "an unchanged status must preserve the user's draft for Apply"
        );
    }
    #[test]
    fn console_uses_presets_for_hardware_divider_results() {
        let mut device = Device::base("test".into(), "Probe".into());
        let (session, events) = live::Session::performance_source();
        device.live_session = Some(session);
        events
            .send(live::Event::ConsoleState {
                framed: true,
                baud: 115207,
            })
            .unwrap();
        device.tick(1.);
        assert_eq!(device.baud, 115207);
        assert_eq!(device.baud_draft, 115200);
        device.baud_pending = true;
        events
            .send(live::Event::ConsoleState {
                framed: true,
                baud: 920810,
            })
            .unwrap();
        events.send(live::Event::BaudComplete).unwrap();
        device.tick(2.);
        assert_eq!(device.baud_draft, 921600);
        assert!(!device.baud_pending);
    }
    #[test]
    fn firmware_drop_validates_selection_without_hardware_writes() {
        let mut d = Device::base("test".into(), "Probe".into());
        assert!(d
            .accept_firmware_drop(&["image.uf2".into()], 0.)
            .unwrap_err()
            .contains("disconnected"));
        assert!(d.page == Page::Firmware);
        assert!(d.flash.job.is_none());
        assert!(d.accept_firmware_drop(&["image.png".into()], 0.).is_err());
        assert!(d
            .accept_firmware_drop(&["a.uf2".into(), "b.uf2".into()], 0.)
            .is_err());
        d.online = true;
        assert!(d.start_firmware(0.).unwrap_err().contains("serial number"));
    }
    #[test]
    fn new_device_has_no_invented_observations() {
        let mut d = Device::base("test".into(), "Probe".into());
        d.tick(20.);
        assert!(d.monitor.live.is_empty());
        assert!(!d.measurement_fresh(20.));
        assert!(d.memory.is_empty());
        assert_eq!(d.registers, [0; 17]);
        assert_eq!(d.voltage, 0.);
    }
}
