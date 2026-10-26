//! Validated firmware images and asynchronous daemon-backed Work programming.
use crate::RpcClient;
pub use ph_image::{Image, Segment, Target};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
    time::Duration,
};

#[derive(Clone)]
pub struct Config {
    pub file: PathBuf,
    pub serial: String,
    pub target: Target,
    pub openocd: String,
    pub scripts: String,
    pub reset_after: bool,
}
#[derive(Debug)]
pub enum Event {
    Phase(String),
    Log(String),
    Image { format: String, bytes: usize },
    Done(Result<(), String>),
}
pub struct Job {
    pub rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Job {
    pub fn start(config: Config) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = cancel.clone();
        let worker = thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let mut rpc = RpcClient::from_env().map_err(|e| e.to_string())?;
                rpc.call::<Value>("device.open", json!({"serial":config.serial}))
                    .map_err(|e| e.to_string())?;
                let upload = rpc.upload(&config.file).map_err(|e| e.to_string())?;
                let task:Value=rpc.call("task.start",json!({"kind":"work_flash","upload":upload,"target":config.target.name(),
                    "openocd":config.openocd,"scripts":config.scripts,"reset_after":config.reset_after})).map_err(|e|e.to_string())?;
                let id = task["id"].as_str().ok_or("phd omitted task ID")?;
                poll_task(&mut rpc, id, &stopped, |event| {
                    if let Some(phase) = event["phase"].as_str() {
                        let _ = tx.send(Event::Phase(phase.into()));
                    }
                    if let Some(log) = event["log"].as_str() {
                        let _ = tx.send(Event::Log(log.into()));
                    }
                    if let Some(image) = event.get("image") {
                        let _ = tx.send(Event::Image {
                            format: image["format"].as_str().unwrap_or("").into(),
                            bytes: image["bytes"].as_u64().unwrap_or(0) as usize,
                        });
                    }
                })?;
                Ok(())
            })();
            let _ = tx.send(Event::Done(result));
        });
        Self {
            rx,
            cancel,
            worker: Some(worker),
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(crate) fn poll_task(
    rpc: &mut RpcClient,
    id: &str,
    cancel: &AtomicBool,
    mut event: impl FnMut(&Value),
) -> Result<Value, String> {
    let mut cursor = 0u64;
    let mut cancellation_sent = false;
    loop {
        if cancel.load(Ordering::Relaxed) && !cancellation_sent {
            rpc.call::<Value>("task.cancel", json!({"id":id}))
                .map_err(|e| e.to_string())?;
            cancellation_sent = true;
        }
        let status: Value = rpc
            .call("task.status", json!({"id":id,"cursor":cursor}))
            .map_err(|e| e.to_string())?;
        if let Some(events) = status["events"].as_array() {
            for entry in events {
                event(&entry["event"]);
            }
        }
        cursor = status["cursor"].as_u64().unwrap_or(cursor);
        match status["state"].as_str() {
            Some("running") => thread::sleep(Duration::from_millis(50)),
            Some("succeeded") => return Ok(status["result"].clone()),
            _ => return Err(status["error"].as_str().unwrap_or("phd task failed").into()),
        }
    }
}
