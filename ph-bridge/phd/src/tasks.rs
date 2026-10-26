//! Daemon-owned tasks survive the initiating socket. Their explicit IDs permit
//! status/cancellation from another client without replaying the operation.
use crate::runtime::{argument, internal, Board, RpcResult, Runtime};
use ph_protocol::RpcError;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

pub(crate) struct Upload {
    pub file: tempfile::NamedTempFile,
    expected: u64,
    written: u64,
    pub ready: bool,
}
impl Upload {
    pub fn new(params: &Value) -> RpcResult<Self> {
        let expected: u64 = argument(params, "length")?;
        if expected == 0 || expected > 64 * 1024 * 1024 {
            return Err(RpcError::invalid("image length must be 1..67108864 bytes"));
        }
        let extension: String = argument(params, "extension")?;
        let extension = extension.to_ascii_lowercase();
        if !matches!(extension.as_str(), "bin" | "uf2" | "elf") {
            return Err(RpcError::invalid("image extension must be bin, uf2 or elf"));
        }
        let file = tempfile::Builder::new()
            .prefix("phd-upload-")
            .suffix(&format!(".{extension}"))
            .tempfile()
            .map_err(|e| RpcError::new(-32020, "file_error", e.to_string()))?;
        Ok(Self {
            file,
            expected,
            written: 0,
            ready: false,
        })
    }
    pub fn write(&mut self, offset: u64, data: &[u8]) -> RpcResult<()> {
        if self.ready || offset != self.written || self.written + data.len() as u64 > self.expected
        {
            return Err(RpcError::invalid(
                "upload offset/length mismatch or upload already finished",
            ));
        }
        self.file
            .write_all(data)
            .map_err(|e| RpcError::new(-32020, "file_error", e.to_string()))?;
        self.written += data.len() as u64;
        Ok(())
    }
    pub fn finish(&mut self) -> RpcResult<()> {
        if self.written != self.expected {
            return Err(RpcError::invalid("upload is incomplete"));
        }
        self.file
            .flush()
            .map_err(|e| RpcError::new(-32020, "file_error", e.to_string()))?;
        self.ready = true;
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct Tasks {
    records: HashMap<String, Task>,
}
struct Task {
    serial: String,
    kind: String,
    snapshot: Arc<Mutex<Snapshot>>,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
struct Snapshot {
    state: &'static str,
    error: Option<String>,
    result: Value,
    cursor: u64,
    events: VecDeque<Value>,
}
impl Snapshot {
    fn event(&mut self, event: Value) {
        self.cursor += 1;
        self.events
            .push_back(json!({"seq":self.cursor,"event":event}));
        if self.events.len() > 128 {
            self.events.pop_front();
        }
    }
}
struct Lease {
    board: Arc<Mutex<Board>>,
    owner: u64,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut board) = self.board.lock() {
            if board.owner == Some(self.owner) {
                board.owner = None;
                board.suspended = false;
                board.link = None;
            }
        }
    }
}
impl Tasks {
    pub fn list(&self) -> RpcResult<Value> {
        let mut records = Vec::new();
        for (id, task) in &self.records {
            let snapshot = task.snapshot.lock().map_err(|_| internal())?;
            records.push(
                json!({"id":id,"serial":task.serial,"kind":task.kind,"state":snapshot.state}),
            );
        }
        records.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        Ok(Value::Array(records))
    }
    pub fn status(&self, id: &str, cursor: u64) -> RpcResult<Value> {
        let task = self.records.get(id).ok_or_else(|| {
            RpcError::new(
                -32004,
                "task_not_found",
                "task is unknown; it may belong to a previous daemon instance",
            )
        })?;
        let state = task.snapshot.lock().map_err(|_| internal())?;
        let oldest = state
            .events
            .front()
            .and_then(|v| v["seq"].as_u64())
            .unwrap_or(state.cursor + 1);
        Ok(
            json!({"id":id,"state":state.state,"error":state.error,"result":state.result,"cursor":state.cursor,
            "gap":cursor.saturating_add(1)<oldest,"events":state.events.iter().filter(|e|e["seq"].as_u64().unwrap_or(0)>cursor).collect::<Vec<_>>()}),
        )
    }
    pub fn cancel(&self, id: &str) -> RpcResult<()> {
        let task = self
            .records
            .get(id)
            .ok_or_else(|| RpcError::new(-32004, "task_not_found", "unknown task"))?;
        task.cancel.store(true, Ordering::Relaxed);
        Ok(())
    }
    pub fn shutdown(&mut self) {
        for task in self.records.values() {
            task.cancel.store(true, Ordering::Relaxed);
        }
        for task in self.records.values_mut() {
            if let Some(worker) = task.worker.take() {
                let _ = worker.join();
            }
        }
    }
}
pub(crate) fn start(
    runtime: &Arc<Runtime>,
    board: Arc<Mutex<Board>>,
    upload: Upload,
    params: &Value,
) -> RpcResult<String> {
    let kind: String = argument(params, "kind")?;
    if !upload.ready {
        return Err(RpcError::invalid("finish the image upload first"));
    }
    if !matches!(kind.as_str(), "work_flash" | "debug_flash") {
        return Err(RpcError::invalid(
            "task kind must be work_flash or debug_flash",
        ));
    }
    // Validate target and image before changing device state or claiming a
    // lease. The worker repeats validation before invoking external tooling.
    let target = if kind == "work_flash" {
        let name: String = argument(params, "target")?;
        Some(ph_device::firmware::Target::parse(&name).map_err(RpcError::invalid)?)
    } else {
        None
    };
    if let Some(target) = target {
        ph_device::firmware::Image::read(upload.file.path(), target).map_err(RpcError::invalid)?;
    }
    let mut tasks = runtime.tasks.lock().map_err(|_| internal())?;
    if tasks.records.len() >= 64 {
        let old = tasks
            .records
            .iter()
            .find(|(_, task)| task.worker.as_ref().is_some_and(|w| w.is_finished()))
            .map(|(id, _)| id.clone());
        if let Some(id) = old {
            tasks.records.remove(&id);
        } else {
            return Err(RpcError::busy("daemon task limit reached"));
        }
    }
    use rand::RngCore;
    let owner = rand::rngs::OsRng.next_u64() | 1 << 63;
    let id = format!("{}-{owner:016x}", runtime.instance);
    let serial = {
        let mut board = board.lock().map_err(|_| internal())?;
        if board.owner.is_some() {
            return Err(RpcError::busy("probe already has an exclusive owner"));
        }
        board.lease(owner)?;
        board.suspended = true;
        board.link = None;
        board.serial.clone()
    };
    let lease = Lease { board, owner };
    let snapshot = Arc::new(Mutex::new(Snapshot {
        state: "running",
        error: None,
        result: Value::Null,
        cursor: 0,
        events: VecDeque::new(),
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let state = snapshot.clone();
    let cancelled = cancel.clone();
    let runtime = runtime.clone();
    let params = params.clone();
    let task_serial = serial.clone();
    let task_kind = kind.clone();
    let worker = thread::spawn(move || {
        let _lease = lease;
        let event = |event: Value| {
            if let Ok(mut snapshot) = state.lock() {
                snapshot.event(event);
            }
        };
        let result = (|| -> Result<Value, String> {
            if kind == "debug_flash" {
                let picotool = params["picotool"].as_str().unwrap_or("picotool");
                let info = ph_device::debug_update::update(
                    &serial,
                    upload.file.path(),
                    picotool,
                    &cancelled,
                    |phase| event(json!({"phase":phase})),
                )?;
                return Ok(json!({"info":info}));
            }
            let job = ph_device::firmware::Job::start(ph_device::firmware::Config {
                file: upload.file.path().to_owned(),
                serial,
                target: target.unwrap(),
                openocd: params["openocd"].as_str().unwrap_or("openocd").into(),
                scripts: params["scripts"].as_str().unwrap_or("").into(),
                reset_after: params["reset_after"].as_bool().unwrap_or(true),
            });
            loop {
                if cancelled.load(Ordering::Relaxed) || runtime.stop.load(Ordering::Relaxed) {
                    cancelled.store(true, Ordering::Relaxed);
                    job.cancel();
                }
                match job.rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(ph_device::firmware::Event::Phase(phase)) => event(json!({"phase":phase})),
                    Ok(ph_device::firmware::Event::Log(log)) => event(json!({"log":log})),
                    Ok(ph_device::firmware::Event::Image { format, bytes }) => {
                        event(json!({"image":{"format":format,"bytes":bytes}}))
                    }
                    Ok(ph_device::firmware::Event::Done(result)) => {
                        result?;
                        return Ok(Value::Null);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => return Err("programming worker disconnected".into()),
                }
            }
        })();
        // Return ownership before advertising completion to a polling caller.
        drop(_lease);
        if let Ok(mut snapshot) = state.lock() {
            match result {
                Ok(value) => {
                    snapshot.state = "succeeded";
                    snapshot.result = value;
                }
                Err(error) => {
                    snapshot.state = if cancelled.load(Ordering::Relaxed) {
                        "cancelled"
                    } else {
                        "failed"
                    };
                    snapshot.error = Some(error);
                }
            }
        }
    });
    tasks.records.insert(
        id.clone(),
        Task {
            serial: task_serial,
            kind: task_kind,
            snapshot,
            cancel,
            worker: Some(worker),
        },
    );
    Ok(id)
}
