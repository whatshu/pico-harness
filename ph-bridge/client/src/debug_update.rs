//! Debug update jobs transfer their image to the daemon host before rebooting.
use crate::{firmware::poll_task, RpcClient};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
};
pub enum Event {
    Phase(String),
    Done(Result<String, String>),
}
pub struct Job {
    pub rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Job {
    pub fn start(serial: String, file: PathBuf, picotool: String) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = cancel.clone();
        let worker = thread::spawn(move || {
            let result = (|| -> Result<String, String> {
                let mut rpc = RpcClient::from_env().map_err(|e| e.to_string())?;
                rpc.call::<Value>("device.open", json!({"serial":serial}))
                    .map_err(|e| e.to_string())?;
                let upload = rpc.upload(&file).map_err(|e| e.to_string())?;
                let task: Value = rpc
                    .call(
                        "task.start",
                        json!({"kind":"debug_flash","upload":upload,"picotool":picotool}),
                    )
                    .map_err(|e| e.to_string())?;
                let id = task["id"].as_str().ok_or("phd omitted task ID")?;
                let result = poll_task(&mut rpc, id, &stopped, |event| {
                    if let Some(phase) = event["phase"].as_str() {
                        let _ = tx.send(Event::Phase(phase.into()));
                    }
                })?;
                Ok(result["info"].as_str().unwrap_or("").into())
            })();
            let _ = tx.send(Event::Done(result));
        });
        Self {
            rx,
            cancel,
            worker: Some(worker),
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
