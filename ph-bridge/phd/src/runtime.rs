use base64::{engine::general_purpose::STANDARD, Engine};
use ph_bridge::msg::{ph_message::Body, PhMessage};
use ph_protocol::{DeviceInfo, EventBatch, RpcError};
use prost::Message;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

pub type RpcResult<T> = Result<T, RpcError>;
const EVENT_HISTORY: usize = 1024;

#[derive(Default)]
pub struct Events {
    next: u64,
    entries: VecDeque<(u64, Value)>,
}
impl Events {
    pub fn push(&mut self, event: Value) {
        self.next += 1;
        self.entries.push_back((self.next, event));
        if self.entries.len() > EVENT_HISTORY {
            self.entries.pop_front();
        }
    }
    pub fn cursor(&self) -> u64 {
        self.next
    }
    pub fn read(&self, instance: &str, cursor: u64, limit: usize) -> EventBatch {
        let oldest = self.entries.front().map_or(self.next + 1, |(id, _)| *id);
        let gap = cursor > self.next || cursor.saturating_add(1) < oldest;
        let cursor = if gap {
            oldest.saturating_sub(1)
        } else {
            cursor
        };
        let entries: Vec<_> = self
            .entries
            .iter()
            .filter(|(id, _)| *id > cursor)
            .take(limit)
            .collect();
        EventBatch {
            instance: instance.into(),
            cursor: entries.last().map_or(cursor, |(id, _)| *id),
            gap,
            events: entries.into_iter().map(|(_, v)| v.clone()).collect(),
        }
    }
    pub fn ph(&mut self, event: PhMessage) {
        let mut value = json!({"ph":STANDARD.encode(event.encode_to_vec())});
        if let Some(Body::ConsoleData(data)) = &event.body {
            value["console"] = json!(STANDARD.encode(&data.data));
        }
        if let Some(Body::UartTrace(trace)) = &event.body {
            value["uart_trace"] = json!({
                "data":STANDARD.encode(&trace.data), "tx":trace.tx,
                "baud":trace.baud, "timestamp_us":trace.timestamp_us,
                "dropped":trace.dropped
            });
        }
        self.push(value);
    }
}

pub struct Board {
    pub serial: String,
    pub link: Option<ph_device::PhBoardLink>,
    pub events: Events,
    /// One SWD / OpenOCD / programming owner per board. Queries can coexist;
    /// conflicting mutations must acquire the same lease.
    pub owner: Option<u64>,
    pub suspended: bool,
}
impl Board {
    pub fn link(&mut self) -> RpcResult<&mut ph_device::PhBoardLink> {
        if self.suspended {
            return Err(RpcError::busy("probe is rebooting for an update"));
        }
        if self.link.as_ref().is_some_and(|link| !link.is_usable()) {
            self.link = None;
            self.events.push(
                json!({"disconnected":"previous control transaction failed; reopening the device"}),
            );
        }
        if self.link.is_none() {
            self.link =
                Some(ph_device::PhBoardLink::open_serial(&self.serial).map_err(hardware_error)?);
        }
        Ok(self.link.as_mut().unwrap())
    }
    pub fn lease(&mut self, owner: u64) -> RpcResult<()> {
        match self.owner {
            Some(existing) if existing != owner => Err(RpcError::busy(
                "device is held by another SWD, flash or debug session",
            )),
            _ => {
                self.owner = Some(owner);
                Ok(())
            }
        }
    }
    pub fn check_mutation(&self, owner: u64) -> RpcResult<()> {
        if self.owner.is_some_and(|existing| existing != owner) {
            Err(RpcError::busy(
                "device has an exclusive operation in progress",
            ))
        } else {
            Ok(())
        }
    }
}

pub struct Runtime {
    pub instance: String,
    pub token: String,
    pub stop: AtomicBool,
    boards: Mutex<HashMap<String, Arc<Mutex<Board>>>>,
    pub(crate) tasks: Mutex<crate::tasks::Tasks>,
    #[cfg(test)]
    pub(crate) fixtures: Mutex<Option<Vec<DeviceInfo>>>,
    pub(crate) debug_ports: Mutex<HashMap<u16, String>>,
}
impl Runtime {
    pub fn new(token: String) -> Arc<Self> {
        use rand::RngCore;
        let instance = format!("{:016x}", rand::rngs::OsRng.next_u64());
        Arc::new(Self {
            instance,
            token,
            stop: AtomicBool::new(false),
            boards: Mutex::new(HashMap::new()),
            tasks: Mutex::new(Default::default()),
            #[cfg(test)]
            fixtures: Mutex::new(None),
            debug_ports: Mutex::new(HashMap::new()),
        })
    }
    pub fn devices(&self) -> RpcResult<Vec<DeviceInfo>> {
        #[cfg(test)]
        if let Some(devices) = self.fixtures.lock().map_err(|_| internal())?.as_ref() {
            return Ok(devices.clone());
        }
        let ports = ph_bridge::device::scan();
        Ok(ph_device::enumerate_devices()
            .map_err(hardware_error)?
            .into_iter()
            .map(|d| {
                let terminal = ports
                    .iter()
                    .find(|p| p.serial == d.serial)
                    .and_then(|p| p.terminal_tty.as_ref())
                    .map(|p| p.to_string_lossy().into_owned());
                DeviceInfo {
                    serial: d.serial,
                    bus: d.bus,
                    address: d.address,
                    product: d.product,
                    access_error: d.access_error,
                    terminal,
                }
            })
            .collect())
    }
    pub fn select(&self, params: &Value) -> RpcResult<(Arc<Mutex<Board>>, DeviceInfo)> {
        for key in ["serial", "port"] {
            if let Some(value) = params.get(key).filter(|v| !v.is_null()) {
                if value.as_str().is_none_or(str::is_empty) {
                    return Err(RpcError::invalid(format!(
                        "{key} must be a nonempty string"
                    )));
                }
            }
        }
        let serial = params.get("serial").and_then(Value::as_str);
        let port = params.get("port").and_then(Value::as_str);
        let devices: Vec<_> = self
            .devices()?
            .into_iter()
            .filter(|d| {
                serial.is_none_or(|s| d.serial.as_deref() == Some(s))
                    && port.is_none_or(|p| d.terminal.as_deref() == Some(p))
            })
            .collect();
        if devices.len() > 1 {
            return Err(RpcError::invalid(
                "multiple probes found; select a serial number",
            ));
        }
        let info = devices.into_iter().next().ok_or_else(|| {
            RpcError::new(
                -32004,
                "device_not_found",
                "selected PicoHarness probe is not connected",
            )
        })?;
        let serial = info
            .serial
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                RpcError::invalid(
                    "probe has no readable serial number; check USB access permissions",
                )
            })?;
        let board = self
            .boards
            .lock()
            .map_err(|_| internal())?
            .entry(serial.clone())
            .or_insert_with(|| {
                Arc::new(Mutex::new(Board {
                    serial,
                    link: None,
                    events: Events::default(),
                    owner: None,
                    suspended: false,
                }))
            })
            .clone();
        Ok((board, info))
    }
    /// One pump services open boards. It never blocks on a board performing a
    /// transaction, and every subscriber sees its own cursor in the same ring.
    pub fn pump(self: &Arc<Self>) -> thread::JoinHandle<()> {
        let runtime = self.clone();
        thread::spawn(move || {
            while !runtime.stop.load(Ordering::Relaxed) {
                let boards: Vec<_> = match runtime.boards.lock() {
                    Ok(map) => map.values().cloned().collect(),
                    Err(_) => break,
                };
                for board in boards {
                    if let Ok(mut board) = board.try_lock() {
                        if let Some(link) = board.link.as_mut() {
                            match link.ph_events() {
                                Ok(events) => {
                                    for event in events {
                                        board.events.ph(event);
                                    }
                                }
                                Err(error) => {
                                    board.link = None;
                                    board.events.push(json!({"disconnected":error.to_string()}));
                                }
                            }
                        }
                    }
                }
                thread::sleep(Duration::from_millis(10));
            }
        })
    }
}
pub fn hardware_error(error: impl std::fmt::Display) -> RpcError {
    RpcError::new(-32010, "device_error", error.to_string())
}
pub fn internal() -> RpcError {
    RpcError::new(-32603, "internal", "daemon state is unavailable")
}
pub fn argument<T: serde::de::DeserializeOwned>(params: &Value, name: &str) -> RpcResult<T> {
    serde_json::from_value(
        params
            .get(name)
            .cloned()
            .ok_or_else(|| RpcError::invalid(format!("missing {name}")))?,
    )
    .map_err(|e| RpcError::invalid(format!("invalid {name}: {e}")))
}

#[cfg(test)]
mod tests;
