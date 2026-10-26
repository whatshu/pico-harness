use crate::runtime::{argument, hardware_error, internal, Board, RpcResult, Runtime};
use base64::{engine::general_purpose::STANDARD, Engine};
use ph_bridge::msg::{ph_message::Body, PhMessage, Route};
use ph_protocol::{Request, Response, RpcError, MAX_DATA, VERSION};
use prost::Message;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    io::{self, BufReader, Write},
    net::TcpStream,
    sync::{atomic::Ordering, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub(crate) struct Session {
    id: u64,
    runtime: Arc<Runtime>,
    selected: Option<Arc<Mutex<Board>>>,
    dap: Option<ph_device::CmsisDap>,
    authenticated: bool,
    shutdown: bool,
    uploads: std::collections::HashMap<String, crate::tasks::Upload>,
    debug: Option<ph_device::debug::DebugServer>,
    tunnel: Option<TcpStream>,
    logic: Option<crate::logic::Job>,
    next_logic_id: u32,
}
impl Session {
    pub fn new(id: u64, runtime: Arc<Runtime>) -> Self {
        Self {
            id,
            runtime,
            selected: None,
            dap: None,
            authenticated: false,
            shutdown: false,
            uploads: Default::default(),
            debug: None,
            tunnel: None,
            logic: None,
            next_logic_id: 0,
        }
    }
    pub fn run(mut self, stream: TcpStream) -> io::Result<()> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let mut stream = BufReader::new(stream);
        while let Some(frame) = ph_protocol::read_frame(&mut stream)? {
            let (id, result) = match serde_json::from_slice::<Request>(&frame) {
                Ok(request) if request.jsonrpc == "2.0" => {
                    (request.id, self.dispatch(&request.method, &request.params))
                }
                Ok(request) => (
                    request.id,
                    Err(RpcError::new(
                        -32600,
                        "invalid_request",
                        "expected JSON-RPC 2.0",
                    )),
                ),
                Err(_) => (
                    0,
                    Err(RpcError::new(
                        -32700,
                        "parse_error",
                        "invalid request envelope",
                    )),
                ),
            };
            let response = match result {
                Ok(value) => Response {
                    jsonrpc: "2.0".into(),
                    id,
                    result: Some(value),
                    error: None,
                },
                Err(error) => Response {
                    jsonrpc: "2.0".into(),
                    id,
                    result: None,
                    error: Some(error),
                },
            };
            let mut bytes = serde_json::to_vec(&response)?;
            bytes.push(b'\n');
            if bytes.len() > ph_protocol::MAX_FRAME {
                return Err(io::Error::other("response exceeds frame limit"));
            }
            stream.get_mut().write_all(&bytes)?;
            if let Some(remote) = self.tunnel.take() {
                remote.set_read_timeout(None)?;
                stream.get_mut().set_read_timeout(None)?;
                let buffered = stream.buffer().to_vec();
                let mut remote = remote;
                remote.write_all(&buffered)?;
                return relay(stream.into_inner(), remote);
            }
            if !self.authenticated {
                break;
            }
            if self.shutdown {
                self.runtime.stop.store(true, Ordering::Relaxed);
                break;
            }
            // Idle sessions expire, so a vanished client cannot retain a lease
            // indefinitely. Active debuggers poll status on this connection.
            stream
                .get_mut()
                .set_read_timeout(Some(Duration::from_secs(90)))?;
        }
        Ok(())
    }
    fn logic(&mut self, board: &mut Board, method: &str, params: &Value) -> RpcResult<Value> {
        if method == "logic.start" {
            let options = ph_bridge::sump::Options {
                count: argument(params, "count")?,
                channels: if params.get("channels").is_some() { argument(params, "channels")? } else { 0 },
                rate: argument(params, "rate")?,
                edge: argument(params, "trigger")?,
                pin: argument(params, "pin")?,
                timeout_ms: argument(params, "timeout_ms")?,
            };
            options
                .validate()
                .map_err(|e| RpcError::invalid(e.to_string()))?;
            if self.dap.is_some()
                || self.debug.is_some()
                || self.logic.as_ref().is_some_and(|job| job.running())
            {
                return Err(RpcError::busy(
                    "close SWD/debug or cancel the active Logic capture first",
                ));
            }
            if board.suspended {
                return Err(RpcError::busy("probe is rebooting for an update"));
            }
            board.check_mutation(self.id)?;
            if options.edge != 0
                && !board
                    .link()?
                    .info()
                    .map_err(hardware_error)?
                    .split(';')
                    .any(|field| field == "sump2")
            {
                return Err(RpcError::invalid("edge capture requires probe firmware with sump2; upgrade Debug firmware or select no trigger"));
            }
            if (options.channels != 0 || options.count > ph_protocol::logic::LEGACY_SAMPLES)
                && !board.link()?.info().map_err(hardware_error)?.split(';').any(|field| field == "pc1")
            {
                return Err(RpcError::invalid("selected-channel/deeper capture requires pc1 Debug firmware"));
            }
            let port = ph_bridge::device::scan()
                .into_iter()
                .find(|device| device.serial.as_deref() == Some(&board.serial))
                .and_then(|device| device.la_tty)
                .ok_or_else(|| {
                    RpcError::new(
                        -32004,
                        "device_not_found",
                        "selected probe has no Logic Analyzer CDC",
                    )
                })?;
            let next_id = self.next_logic_id.checked_add(1).ok_or_else(internal)?;
            board.lease(self.id)?;
            self.logic.take();
            self.next_logic_id = next_id;
            match crate::logic::Job::start(self.next_logic_id, port, options) {
                Ok(job) => self.logic = Some(job),
                Err(error) => {
                    board.owner = None;
                    return Err(error);
                }
            }
            return value(self.next_logic_id);
        }
        if method == "logic.cancel" {
            self.logic.take(); // cancel, reset SUMP and join before releasing ownership
            if board.owner == Some(self.id) && self.dap.is_none() && self.debug.is_none() {
                board.owner = None;
            }
            return Ok(Value::Null);
        }
        if !matches!(method, "logic.status" | "logic.read") {
            return Err(RpcError::new(
                -32601,
                "method_not_found",
                "unknown Logic operation",
            ));
        }
        let id: u32 = argument(params, "capture_id")?;
        let job = self
            .logic
            .as_mut()
            .filter(|job| job.id == id)
            .ok_or_else(|| RpcError::invalid("capture does not belong to this connection"))?;
        let status = job.poll();
        if !job.running()
            && board.owner == Some(self.id)
            && self.dap.is_none()
            && self.debug.is_none()
        {
            board.owner = None;
        }
        let status = status?;
        if method == "logic.status" {
            return value(status);
        }
        let status = status.ok_or_else(|| RpcError::busy("capture is still running"))?;
        let offset: usize = if params.get("offset").is_some() { argument(params, "offset")? } else { 0 };
        let limit: usize = if params.get("limit").is_some() { argument(params, "limit")? } else { 32768 };
        value(ph_protocol::board::CaptureResult {
            capture_id: id,
            elapsed_us: status.elapsed_us,
            samples: job.samples(offset, limit.min(32768))?,
        })
    }
    fn dispatch(&mut self, method: &str, params: &Value) -> RpcResult<Value> {
        if !params.is_object() {
            return Err(RpcError::invalid("params must be an object"));
        }
        if method == "hello" {
            if self.authenticated {
                return Err(RpcError::invalid("session is already authenticated"));
            }
            let version: u32 = argument(params, "version")?;
            let token: String = argument(params, "token")?;
            if !crate::credentials::matches(&self.runtime.token, &token) {
                return Err(RpcError::new(
                    -32001,
                    "unauthorized",
                    "invalid daemon token",
                ));
            }
            if version != VERSION {
                return Err(RpcError::new(
                    -32002,
                    "version_mismatch",
                    format!("phd requires protocol {VERSION}"),
                ));
            }
            self.authenticated = true;
            return Ok(json!({"version":VERSION,"instance":self.runtime.instance,"server":"phd"}));
        }
        if !self.authenticated {
            return Err(RpcError::new(
                -32001,
                "unauthorized",
                "authenticate with hello first",
            ));
        }
        match method {
            "daemon.health" => {
                return Ok(
                    json!({"version":VERSION,"instance":self.runtime.instance,"status":"ready","package_version":env!("CARGO_PKG_VERSION")}),
                )
            }
            "daemon.stop" => {
                self.shutdown = true;
                return Ok(Value::Null);
            }
            "device.list" => return value(self.runtime.devices()?),
            "device.open" => {
                if self.dap.is_some() || self.debug.is_some() || self.logic.is_some() {
                    return Err(RpcError::busy(
                        "close SWD/debug before selecting another device",
                    ));
                }
                let (board, info) = self.runtime.select(params)?;
                let cursor = {
                    let mut board = board.lock().map_err(|_| internal())?;
                    board.link()?;
                    board.events.cursor()
                };
                self.selected = Some(board);
                return Ok(
                    json!({"serial":info.serial,"terminal":info.terminal,"cursor":cursor,"instance":self.runtime.instance}),
                );
            }
            "device.bootloader" => {
                let location: ph_protocol::Location = argument(params, "location")?;
                let product_id = argument(params, "product_id")?;
                return value(
                    ph_device::usb::bootloader_at(
                        &ph_device::usb::Location {
                            bus: location.bus,
                            ports: location.ports,
                        },
                        product_id,
                    )
                    .map_err(hardware_error)?,
                );
            }
            "device.capabilities" => {
                return Ok(
                    json!({"control":true,"console":true,"bulk_write":true,"bulk_read":false,"swd":true,"logic_capture":true,"logic_transport":"sump","logic_trigger_requires":"sump2","logic_max_samples":1310720,"logic_buffer_bytes":163840,"logic_max_samples_all_channels":40960,"logic_packed_requires":"pc1","logic_max_trigger_rate":20000000,"logic_max_trigger_timeout_ms":5000,"max_bulk_bytes":MAX_DATA}),
                )
            }
            "events.read" => return self.events(params),
            "upload.begin" => {
                if self.uploads.len() >= 2 {
                    return Err(RpcError::busy("at most two unfinished uploads per session"));
                }
                let upload = crate::tasks::Upload::new(params)?;
                let id = upload
                    .file
                    .path()
                    .file_name()
                    .ok_or_else(internal)?
                    .to_string_lossy()
                    .into_owned();
                self.uploads.insert(id.clone(), upload);
                return Ok(json!({"id":id}));
            }
            "upload.write" | "upload.finish" => {
                let id: String = argument(params, "id")?;
                let upload = self
                    .uploads
                    .get_mut(&id)
                    .ok_or_else(|| RpcError::invalid("upload does not belong to this session"))?;
                if method == "upload.write" {
                    upload.write(argument(params, "offset")?, &data(params)?)?;
                } else {
                    upload.finish()?;
                }
                return Ok(Value::Null);
            }
            "task.list" => return self.runtime.tasks.lock().map_err(|_| internal())?.list(),
            "task.status" => {
                let id: String = argument(params, "id")?;
                let cursor = params["cursor"].as_u64().unwrap_or(0);
                return self
                    .runtime
                    .tasks
                    .lock()
                    .map_err(|_| internal())?
                    .status(&id, cursor);
            }
            "task.cancel" => {
                let id: String = argument(params, "id")?;
                self.runtime
                    .tasks
                    .lock()
                    .map_err(|_| internal())?
                    .cancel(&id)?;
                return Ok(Value::Null);
            }
            "task.start" => {
                let selected = self
                    .selected
                    .clone()
                    .ok_or_else(|| RpcError::invalid("select the target probe first"))?;
                let id: String = argument(params, "upload")?;
                let upload = self
                    .uploads
                    .remove(&id)
                    .ok_or_else(|| RpcError::invalid("unknown image upload"))?;
                let id = crate::tasks::start(&self.runtime, selected, upload, params)?;
                return Ok(json!({"id":id,"instance":self.runtime.instance}));
            }
            "debug.tunnel" => {
                let port: u16 = argument(params, "port")?;
                if !self
                    .runtime
                    .debug_ports
                    .lock()
                    .map_err(|_| internal())?
                    .contains_key(&port)
                {
                    return Err(RpcError::invalid("debug endpoint is no longer active"));
                }
                self.tunnel = Some(
                    TcpStream::connect_timeout(
                        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
                        Duration::from_secs(3),
                    )
                    .map_err(hardware_error)?,
                );
                return Ok(Value::Null);
            }
            _ => {}
        }
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| RpcError::invalid("select a probe with device.open first"))?;
        let mut board = selected.lock().map_err(|_| internal())?;
        if method.starts_with("logic.") {
            return self.logic(&mut board, method, params);
        }
        if self.logic.as_ref().is_some_and(|job| job.running())
            && !matches!(
                method,
                "board.info"
                    | "board.status"
                    | "board.measure"
                    | "console.status"
                    | "device.location"
                    | "events.read"
            )
        {
            return Err(RpcError::busy(
                "cancel or finish Logic capture before mutating the device",
            ));
        }
        if method.starts_with("debug.") {
            return self.debug(&mut board, method, params);
        }
        if method.starts_with("swd.") {
            return self.swd(&mut board, method, params);
        }
        let query = matches!(
            method,
            "board.info"
                | "board.status"
                | "board.measure"
                | "console.status"
                | "device.location"
                | "ph.request"
        );
        if !query {
            board.check_mutation(self.id)?;
        }
        let result = match method {
            "board.info" => value(board.link()?.info().map_err(hardware_error)?),
            "board.status" => value(board.link()?.status().map_err(hardware_error)?),
            "board.measure" => value(board.link()?.measure().map_err(hardware_error)?),
            "board.power" => value(
                board
                    .link()?
                    .set_power(argument(params, "enabled")?)
                    .map_err(hardware_error)?,
            ),
            "board.reset" => {
                let milliseconds: u16 = argument(params, "milliseconds")?;
                if milliseconds == 0 || milliseconds > 1000 {
                    return Err(RpcError::invalid("reset duration must be 1..1000 ms"));
                }
                value(
                    board
                        .link()?
                        .reset_pulse(milliseconds)
                        .map_err(hardware_error)?,
                )
            }
            "board.bootsel" => {
                board
                    .link()?
                    .enter_debug_bootsel()
                    .map_err(hardware_error)?;
                board.link = None;
                Ok(Value::Null)
            }
            "device.location" => value(board.link()?.usb_location().map_err(hardware_error)?),
            "console.write" => {
                let data = data(params)?;
                if data.len() > 128 {
                    return Err(RpcError::invalid("console writes are limited to 128 bytes"));
                }
                value(board.link()?.console_write(&data).map_err(hardware_error)?)
            }
            "console.configure" => value(
                board
                    .link()?
                    .console_baud(argument(params, "baud")?)
                    .map_err(hardware_error)?,
            ),
            "console.status" => value(board.link()?.term_status().map_err(hardware_error)?),
            "bulk.write" => {
                let data = data(params)?;
                let route = match params
                    .get("route")
                    .and_then(Value::as_str)
                    .unwrap_or("work_spi")
                {
                    "debug" => Route::Debug,
                    "work_spi" => Route::WorkSpi,
                    _ => return Err(RpcError::invalid("route must be debug or work_spi")),
                };
                if data.is_empty() || (route == Route::Debug && data.len() > 4096) {
                    return Err(RpcError::invalid(
                        "Bulk requires 1..4096 bytes for Debug, or 1..1048576 for Work",
                    ));
                }
                let (_, crc) = board
                    .link()?
                    .bulk_write(&data, route)
                    .map_err(hardware_error)?;
                Ok(json!({"ok":true,"bytes":data.len(),"crc32c":crc}))
            }
            "ph.request" => {
                let encoded = data(params)?;
                if encoded.len() > 512 {
                    return Err(RpcError::invalid("PH message exceeds firmware frame limit"));
                }
                let message = PhMessage::decode(encoded.as_slice())
                    .map_err(|e| RpcError::invalid(e.to_string()))?;
                if message.body.is_none()
                    || matches!(
                        message.body,
                        Some(
                            Body::BulkTxBegin(_)
                                | Body::BulkRxBegin(_)
                                | Body::BulkTxDone(_)
                                | Body::BulkRxDone(_)
                                | Body::BulkRxAvail(_)
                                | Body::ConsoleData(_)
                        )
                    )
                {
                    return Err(RpcError::invalid("use bulk.write for complete Bulk transactions; events cannot be submitted as requests"));
                }
                if ph_bridge::framing::encode(&message).len() > ph_bridge::PH_MSG_MAX {
                    return Err(RpcError::invalid("PH frame exceeds firmware limit"));
                }
                if !matches!(
                    message.body,
                    Some(
                        Body::GetInfo(_)
                            | Body::GetStatus(_)
                            | Body::Measure(_)
                            | Body::TermStatus(_)
                    )
                ) {
                    board.check_mutation(self.id)?;
                }
                let seq = message.seq;
                let mut response = board.link()?.ph_request(message).map_err(hardware_error)?;
                response.seq = seq;
                Ok(json!({"data":STANDARD.encode(response.encode_to_vec())}))
            }
            _ => Err(RpcError::new(
                -32601,
                "method_not_found",
                format!("unknown method {method}"),
            )),
        };
        // Publish events collected during this transaction before releasing
        // the board. Busy callers must not starve subscribers by repeatedly
        // acquiring the lock ahead of the background pump.
        if let Some(link) = board.link.as_mut() {
            let events = link.drain_events();
            for event in events {
                board.events.ph(event);
            }
        }
        result
    }
    fn events(&self, params: &Value) -> RpcResult<Value> {
        let selected = self
            .selected
            .as_ref()
            .ok_or_else(|| RpcError::invalid("select a probe first"))?;
        let cursor: u64 = argument(params, "cursor")?;
        let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(128);
        let wait = params.get("wait_ms").and_then(Value::as_u64).unwrap_or(0);
        if limit == 0 || limit > 256 || wait > 1000 {
            return Err(RpcError::invalid(
                "limit must be 1..256 and wait_ms 0..1000",
            ));
        }
        let deadline = Instant::now() + Duration::from_millis(wait);
        loop {
            let batch = selected.lock().map_err(|_| internal())?.events.read(
                &self.runtime.instance,
                cursor,
                limit as usize,
            );
            if !batch.events.is_empty() || batch.gap || Instant::now() >= deadline {
                return value(batch);
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn swd(&mut self, board: &mut Board, method: &str, params: &Value) -> RpcResult<Value> {
        if method == "swd.open" {
            if self.dap.is_some() || self.debug.is_some() {
                return Err(RpcError::busy(
                    "SWD/debug is already open on this connection",
                ));
            }
            board.lease(self.id)?;
            match ph_device::CmsisDap::open_serial(&board.serial) {
                Ok(dap) => {
                    let size = dap.packet_size();
                    self.dap = Some(dap);
                    return value(size);
                }
                Err(error) => {
                    board.owner = None;
                    return Err(hardware_error(error));
                }
            }
        }
        if method == "swd.disconnect" {
            let result = self
                .dap
                .take()
                .map(|mut dap| dap.disconnect())
                .transpose()
                .map_err(hardware_error);
            if board.owner == Some(self.id) {
                board.owner = None;
            }
            result?;
            return Ok(Value::Null);
        }
        let dap = self
            .dap
            .as_mut()
            .ok_or_else(|| RpcError::invalid("open SWD on this connection first"))?;
        match method {
            "swd.packet_size" => value(dap.packet_size()),
            "swd.info" => value(dap.info(argument(params, "id")?).map_err(hardware_error)?),
            "swd.connect_swd" => value(
                dap.connect_swd(argument(params, "clock_hz")?)
                    .map_err(hardware_error)?,
            ),
            "swd.connect_swd_adiv6" => value(
                dap.connect_swd_adiv6(argument(params, "clock_hz")?, argument(params, "ap_base")?)
                    .map_err(hardware_error)?,
            ),
            "swd.connect_swd_core" => value(
                dap.connect_swd_core(argument(params, "clock_hz")?, argument(params, "core")?)
                    .map_err(hardware_error)?,
            ),
            "swd.set_clock" => value(
                dap.set_clock(argument(params, "hz")?)
                    .map_err(hardware_error)?,
            ),
            "swd.swj_sequence" => value(
                dap.swj_sequence(
                    argument(params, "clocks")?,
                    &argument::<Vec<u8>>(params, "data")?,
                )
                .map_err(hardware_error)?,
            ),
            "swd.reset_target" => {
                let duration: u32 = argument(params, "milliseconds")?;
                if duration > 1000 {
                    return Err(RpcError::invalid("SWD reset must be at most 1000 ms"));
                }
                value(dap.reset_target(duration).map_err(hardware_error)?)
            }
            "swd.set_reset" => value(
                dap.set_reset(argument(params, "asserted")?)
                    .map_err(hardware_error)?,
            ),
            "swd.transfer" => value(
                dap.transfer(&argument::<Vec<ph_device::TransferRequest>>(
                    params,
                    "transfers",
                )?)
                .map_err(hardware_error)?,
            ),
            "swd.read_dp" => value(
                dap.read_dp(argument(params, "address")?)
                    .map_err(hardware_error)?,
            ),
            "swd.write_dp" => value(
                dap.write_dp(argument(params, "address")?, argument(params, "value")?)
                    .map_err(hardware_error)?,
            ),
            "swd.read_ap" => value(
                dap.read_ap(argument(params, "address")?)
                    .map_err(hardware_error)?,
            ),
            "swd.write_ap" => value(
                dap.write_ap(argument(params, "address")?, argument(params, "value")?)
                    .map_err(hardware_error)?,
            ),
            "swd.read_dp_idcode" => value(dap.read_dp_idcode().map_err(hardware_error)?),
            "swd.power_up_debug_port" => value(dap.power_up_debug_port().map_err(hardware_error)?),
            "swd.clear_sticky_errors" => value(dap.clear_sticky_errors().map_err(hardware_error)?),
            "swd.read_memory32" => {
                let words: usize = argument(params, "words")?;
                if words == 0 || words > 16384 {
                    return Err(RpcError::invalid("one SWD read requires 1..16384 words"));
                }
                let address: u32 = argument(params, "address")?;
                if address & 3 != 0 || address.checked_add((words as u32 - 1) * 4).is_none() {
                    return Err(RpcError::invalid(
                        "SWD memory range must be aligned and fit in the 32-bit address space",
                    ));
                }
                value(dap.read_memory32(address, words).map_err(hardware_error)?)
            }
            "swd.write_memory32" => value(
                dap.write_memory32(argument(params, "address")?, argument(params, "value")?)
                    .map_err(hardware_error)?,
            ),
            _ => Err(RpcError::new(
                -32601,
                "method_not_found",
                format!("unknown SWD method {method}"),
            )),
        }
    }
    fn debug(&mut self, board: &mut Board, method: &str, params: &Value) -> RpcResult<Value> {
        match method {
            "debug.start" => {
                if self.debug.is_some() || self.dap.is_some() {
                    return Err(RpcError::busy(
                        "close this session's SWD/debug resource first",
                    ));
                }
                let mut config: ph_protocol::DebugConfig =
                    serde_json::from_value(params.clone())
                        .map_err(|e| RpcError::invalid(e.to_string()))?;
                if config.serial != board.serial {
                    return Err(RpcError::invalid(
                        "debug serial must match the selected probe",
                    ));
                }
                config.serial = board.serial.clone();
                board.lease(self.id)?;
                let server = match ph_device::debug::DebugServer::start(&config) {
                    Ok(server) => server,
                    Err(error) => {
                        board.owner = None;
                        return Err(hardware_error(error));
                    }
                };
                let port = server.port;
                self.runtime
                    .debug_ports
                    .lock()
                    .map_err(|_| internal())?
                    .insert(port, board.serial.clone());
                self.debug = Some(server);
                Ok(json!({"port":port}))
            }
            "debug.status" => {
                let server = self
                    .debug
                    .as_mut()
                    .ok_or_else(|| RpcError::invalid("start a debug server first"))?;
                let (exited, logs) = server.poll().map_err(hardware_error)?;
                Ok(json!({"exited":exited,"logs":logs}))
            }
            "debug.stop" => {
                self.stop_debug();
                if board.owner == Some(self.id) {
                    board.owner = None;
                }
                Ok(Value::Null)
            }
            _ => Err(RpcError::new(
                -32601,
                "method_not_found",
                format!("unknown debug method {method}"),
            )),
        }
    }
    fn stop_debug(&mut self) {
        if let Some(server) = self.debug.take() {
            if let Ok(mut ports) = self.runtime.debug_ports.lock() {
                ports.remove(&server.port);
            }
            drop(server);
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        // Close the actual USB handle before publishing the lease as free.
        self.logic.take();
        self.dap.take();
        self.stop_debug();
        if let Some(board) = &self.selected {
            if let Ok(mut board) = board.lock() {
                if board.owner == Some(self.id) {
                    board.owner = None;
                }
            }
        }
    }
}
fn value(value: impl Serialize) -> RpcResult<Value> {
    serde_json::to_value(value).map_err(|_| internal())
}
fn data(params: &Value) -> RpcResult<Vec<u8>> {
    let encoded: String = argument(params, "data")?;
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|e| RpcError::invalid(format!("invalid base64 data: {e}")))?;
    if bytes.len() > MAX_DATA {
        return Err(RpcError::invalid("payload exceeds 1 MiB"));
    }
    Ok(bytes)
}

fn relay(mut client: TcpStream, mut remote: TcpStream) -> io::Result<()> {
    use std::net::Shutdown;
    let mut from_client = client.try_clone()?;
    let mut to_remote = remote.try_clone()?;
    let up = thread::spawn(move || {
        let result = io::copy(&mut from_client, &mut to_remote);
        let _ = from_client.shutdown(Shutdown::Both);
        let _ = to_remote.shutdown(Shutdown::Both);
        result
    });
    let result = io::copy(&mut remote, &mut client);
    let _ = remote.shutdown(Shutdown::Both);
    let _ = client.shutdown(Shutdown::Both);
    let _ = up.join();
    result.map(|_| ())
}

#[cfg(test)]
mod logic_tests {
    use super::*;
    fn fixture(pending: bool) -> (Session, Arc<Mutex<Board>>) {
        let runtime = Runtime::new("fixture-token".into());
        let board = Arc::new(Mutex::new(Board {
            serial: "fixture".into(),
            link: None,
            events: Default::default(),
            owner: Some(7),
            suspended: false,
        }));
        let mut session = Session::new(7, runtime);
        session.authenticated = true;
        session.selected = Some(board.clone());
        session.logic = Some(crate::logic::Job::fixture(pending));
        (session, board)
    }
    #[test]
    fn completed_capture_has_typed_status_gpio_samples_and_private_id() {
        let (mut session, board) = fixture(false);
        let mut other = Session::new(8, session.runtime.clone());
        other.authenticated = true;
        other.selected = Some(board.clone());
        assert_eq!(
            other
                .dispatch("logic.read", &json!({"capture_id":1}))
                .unwrap_err()
                .kind,
            "invalid_argument"
        );
        other.dispatch("logic.cancel", &json!({})).unwrap();
        assert_eq!(board.lock().unwrap().owner, Some(7));
        assert_eq!(
            session
                .dispatch("logic.status", &json!({"capture_id":99}))
                .unwrap_err()
                .kind,
            "invalid_argument"
        );
        let status = session
            .dispatch("logic.status", &json!({"capture_id":1}))
            .unwrap();
        assert_eq!(status["sample_hz"], 2_000_000);
        assert_eq!(status["count"], 4);
        assert_eq!(board.lock().unwrap().owner, None);
        let data = session
            .dispatch("logic.read", &json!({"capture_id":1}))
            .unwrap();
        assert_eq!(data["samples"], json!([32, 32, 32, 32]));
        let page = session.dispatch("logic.read", &json!({"capture_id":1,"offset":1,"limit":2})).unwrap();
        assert_eq!(page["samples"], json!([32,32]));
        for bad in [json!({"capture_id":1,"offset":99}), json!({"capture_id":1,"offset":-1}),
                    json!({"capture_id":1,"limit":0})] {
            assert_eq!(session.dispatch("logic.read", &bad).unwrap_err().kind, "invalid_argument");
        }
        session.dispatch("logic.cancel", &json!({})).unwrap();
        assert!(session.logic.is_none());
    }
    #[test]
    fn active_capture_rejects_mutations_and_cancel_or_disconnect_releases_lease() {
        for disconnect in [false, true] {
            let (mut session, board) = fixture(true);
            assert_eq!(
                session
                    .dispatch("board.power", &json!({"enabled":false}))
                    .unwrap_err()
                    .kind,
                "busy"
            );
            assert_eq!(
                session.dispatch("swd.open", &json!({})).unwrap_err().kind,
                "busy"
            );
            assert!(session
                .dispatch("logic.status", &json!({"capture_id":1}))
                .unwrap()
                .is_null());
            let start = Instant::now();
            if disconnect {
                drop(session);
            } else {
                session.dispatch("logic.cancel", &json!({})).unwrap();
            }
            assert!(start.elapsed() < Duration::from_secs(1));
            assert_eq!(board.lock().unwrap().owner, None);
        }
    }
}
