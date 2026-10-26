//! Connection-owned SUMP worker. USB IO never holds the board's control mutex.
use crate::runtime::{hardware_error, RpcResult};
use ph_bridge::sump::{self, Capture, Options};
use ph_protocol::board::CaptureStatus;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

pub(crate) struct Job {
    pub id: u32,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<RpcResult<Capture>>>,
    result: Option<RpcResult<Capture>>,
}
impl Job {
    pub fn start(id: u32, port: PathBuf, options: Options) -> RpcResult<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let worker = thread::Builder::new()
            .name("phd-sump".into())
            .spawn(move || sump::capture(&port, options, &cancelled).map_err(hardware_error))
            .map_err(hardware_error)?;
        Ok(Self {
            id,
            cancel,
            worker: Some(worker),
            result: None,
        })
    }
    pub fn running(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }
    pub fn poll(&mut self) -> RpcResult<Option<CaptureStatus>> {
        if self.running() {
            return Ok(None);
        }
        if let Some(worker) = self.worker.take() {
            self.result = Some(worker.join().unwrap_or_else(|_| {
                Err(ph_protocol::RpcError::new(
                    -32603,
                    "internal",
                    "SUMP worker panicked",
                ))
            }));
        }
        let capture = self
            .result
            .as_ref()
            .expect("finished job has a result")
            .as_ref()
            .map_err(Clone::clone)?;
        Ok(Some(CaptureStatus {
            sample_hz: capture.sample_hz,
            count: capture.samples.len() as u32,
            elapsed_us: ((capture.samples.len() as u64 * 1_000_000 / capture.sample_hz as u64)
                .max(1)) as u32,
            trigger_index: capture.trigger_index,
        }))
    }
    pub fn samples(&self, offset: usize, limit: usize) -> RpcResult<Vec<u32>> {
        let capture = self.result.as_ref()
            .ok_or_else(|| ph_protocol::RpcError::busy("capture is still running"))?
            .as_ref().map_err(Clone::clone)?;
        if limit == 0 || offset > capture.samples.len() {
            return Err(ph_protocol::RpcError::invalid("invalid capture page"));
        }
        Ok(capture.samples[offset..offset.saturating_add(limit).min(capture.samples.len())].to_vec())
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

#[cfg(test)]
impl Job {
    pub(crate) fn fixture(pending: bool) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        if pending {
            Self {
                id: 1,
                cancel,
                worker: Some(thread::spawn(move || {
                    while !cancelled.load(Ordering::Relaxed) {
                        thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(ph_protocol::RpcError::invalid("cancelled fixture"))
                })),
                result: None,
            }
        } else {
            Self {
                id: 1,
                cancel,
                worker: None,
                result: Some(Ok(Capture {
                    samples: vec![1 << 5; 4],
                    sample_hz: 2_000_000,
                    trigger_index: None,
                })),
            }
        }
    }
}
