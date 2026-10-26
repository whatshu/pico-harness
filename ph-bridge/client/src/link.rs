//! Desktop-compatible board control over a phd socket session.
use crate::protocol::{
    CaptureResult, CaptureStatus, Command, Measurement, PowerStatus, UartStatus,
};
use crate::{DeviceSelector, Error, Result, RpcClient};
use serde_json::{json, Value};
fn moved(channel: &str) -> Error {
    Error::InvalidArgument(format!(
        "{channel} is not supported; query phd capabilities"
    ))
}
#[derive(Clone, Debug)]
pub struct UartTrace {
    pub data: Vec<u8>,
    pub tx: bool,
    pub baud: u32,
    pub timestamp_us: u64,
    pub dropped: u32,
}
pub struct PhBoardLink {
    pub(crate) rpc: RpcClient,
    pub serial: Option<String>,
    tty: String,
    cursor: u64,
    uart_trace: Vec<UartTrace>,
}
impl PhBoardLink {
    fn connect(serial: Option<String>) -> Result<Self> {
        let mut rpc = RpcClient::from_env()?;
        let opened: Value = rpc.call("device.open", json!({"serial":serial}))?;
        Ok(Self {
            serial: opened["serial"].as_str().map(str::to_owned),
            tty: opened["terminal"].as_str().unwrap_or("").into(),
            cursor: opened["cursor"].as_u64().unwrap_or(0),
            rpc,
            uart_trace: Vec::new(),
        })
    }
    pub fn open() -> Result<Self> {
        Self::connect(None)
    }
    pub fn open_serial(serial: impl Into<String>) -> Result<Self> {
        Self::connect(Some(serial.into()))
    }
    pub fn open_with_selector(selector: DeviceSelector) -> Result<Self> {
        Self::connect(selector.serial)
    }
    pub fn port(&self) -> &str {
        &self.tty
    }
    pub fn usb_location(&self) -> Result<crate::usb::Location> {
        let mut rpc = RpcClient::from_env()?;
        rpc.call::<Value>("device.open", json!({"serial":self.serial}))?;
        rpc.call("device.location", json!({}))
    }
    pub fn interface_number(&self) -> u8 {
        1
    }
    pub fn info(&mut self) -> Result<String> {
        self.rpc.call("board.info", json!({}))
    }
    pub fn status(&mut self) -> Result<PowerStatus> {
        self.rpc.call("board.status", json!({}))
    }
    pub fn set_power(&mut self, enabled: bool) -> Result<()> {
        self.rpc.call("board.power", json!({"enabled":enabled}))
    }
    pub fn reset_pulse(&mut self, milliseconds: u16) -> Result<()> {
        self.rpc
            .call("board.reset", json!({"milliseconds":milliseconds}))
    }
    pub fn measure(&mut self) -> Result<Measurement> {
        self.rpc.call("board.measure", json!({}))
    }
    pub fn enter_debug_bootsel(&mut self) -> Result<()> {
        self.rpc.call("board.bootsel", json!({}))
    }
    pub fn console_write(&mut self, data: &[u8]) -> Result<()> {
        use base64::Engine;
        self.rpc.call(
            "console.write",
            json!({"data":base64::engine::general_purpose::STANDARD.encode(data)}),
        )
    }
    pub fn console_baud(&mut self, baud: u32) -> Result<u32> {
        self.rpc.call("console.configure", json!({"baud":baud}))
    }
    pub fn term_status(&mut self) -> Result<(i32, u32, u32, u32)> {
        self.rpc.call("console.status", json!({}))
    }
    // ---- moved to dedicated interfaces ----------------------------------

    pub fn command(&mut self, _command: Command, _payload: &[u8]) -> Result<Vec<u8>> {
        Err(moved("the raw PDBG command channel"))
    }

    pub fn set_bootsel(&mut self, _asserted: bool) -> Result<()> {
        Err(moved("work BOOTSEL hold"))
    }

    pub fn enter_bootsel(&mut self) -> Result<()> {
        Err(moved("work BOOTSEL entry"))
    }

    pub fn gpio_read(&mut self) -> Result<u32> {
        Err(moved("GPIO snapshot"))
    }

    pub fn uart_config(
        &mut self,
        _channel: u8,
        baud: u32,
        _data_bits: u8,
        _stop_bits: u8,
        _parity: u8,
    ) -> Result<u32> {
        // The Work console is a single PH channel; honour the baud request.
        self.console_baud(baud)
    }

    pub fn uart_status(&mut self, _channel: u8) -> Result<UartStatus> {
        Err(moved("UART fifo status"))
    }

    pub fn uart_write_some(&mut self, _channel: u8, data: &[u8]) -> Result<usize> {
        self.console_write(data)?;
        Ok(data.len())
    }

    pub fn uart_write(
        &mut self,
        _channel: u8,
        data: &[u8],
        _timeout: std::time::Duration,
    ) -> Result<usize> {
        self.console_write(data)?;
        Ok(data.len())
    }

    pub fn take_uart_trace(&mut self) -> Vec<UartTrace> {
        std::mem::take(&mut self.uart_trace)
    }

    pub fn uart_read(&mut self, _channel: u8, _maximum: u16) -> Result<Vec<u8>> {
        let batch: ph_protocol::EventBatch = self.rpc.call(
            "events.read",
            serde_json::json!({"cursor":self.cursor,"limit":128,"wait_ms":6}),
        )?;
        self.cursor = batch.cursor;
        if batch.gap {
            return Err(crate::Error::Configuration(
                "console history overflowed; read again from the returned cursor".into(),
            ));
        }
        let mut out = Vec::new();
        use base64::Engine;
        for event in batch.events {
            if !event["uart_trace"].is_null() {
                let trace = &event["uart_trace"];
                self.uart_trace.push(UartTrace {
                    data: base64::engine::general_purpose::STANDARD
                        .decode(trace["data"].as_str().unwrap_or(""))
                        .map_err(|e| crate::Error::Configuration(e.to_string()))?,
                    tx: trace["tx"].as_bool().unwrap_or(false),
                    baud: trace["baud"].as_u64().unwrap_or(0) as u32,
                    timestamp_us: trace["timestamp_us"].as_u64().unwrap_or(0),
                    dropped: trace["dropped"].as_u64().unwrap_or(0) as u32,
                });
            }
            if let Some(encoded) = event["console"].as_str() {
                out.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .map_err(|e| crate::Error::Configuration(e.to_string()))?,
                );
            }
        }
        Ok(out)
    }

    pub fn spi_config(&mut self, _baud: u32, _mode: u8) -> Result<u32> {
        Err(moved("SPI configuration"))
    }

    pub fn spi_transfer(&mut self, _data: &[u8]) -> Result<Vec<u8>> {
        Err(moved("SPI transfers (use the PH Terminal Bulk data plane)"))
    }

    pub fn capture(&mut self, samples: u32, frequency: u32) -> Result<CaptureResult> {
        self.capture_triggered(samples, frequency, 0, 0, 1000)
    }
    pub fn capture_triggered(
        &mut self,
        samples: u32,
        frequency: u32,
        edge: u8,
        pin: u8,
        timeout_ms: u16,
    ) -> Result<CaptureResult> {
        let id = self.start_capture_triggered(samples, frequency, edge, pin, timeout_ms)?;
        loop {
            match self.poll_capture_status(id) {
                Ok(Some(_)) => return self.read_capture(id, 0, 0),
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(error) => {
                    let _ = self.cancel_capture();
                    return Err(error);
                }
            }
        }
    }
    pub fn start_capture(&mut self, samples: u32, frequency: u32) -> Result<u32> {
        self.start_capture_triggered(samples, frequency, 0, 0, 1000)
    }
    pub fn start_capture_triggered(
        &mut self,
        samples: u32,
        frequency: u32,
        edge: u8,
        pin: u8,
        timeout_ms: u16,
    ) -> Result<u32> {
        self.rpc.call("logic.start",json!({"count":samples,"rate":frequency,"trigger":edge,"pin":pin,"timeout_ms":timeout_ms}))
    }
    pub fn poll_capture(&mut self, id: u32) -> Result<Option<(u32, u32, u32)>> {
        Ok(self
            .poll_capture_status(id)?
            .map(|status| (status.sample_hz, status.count, status.elapsed_us)))
    }
    pub fn poll_capture_status(&mut self, id: u32) -> Result<Option<CaptureStatus>> {
        self.rpc.call("logic.status", json!({"capture_id":id}))
    }
    pub fn read_capture(&mut self, id: u32, captured: u32, elapsed_us: u32) -> Result<CaptureResult> {
        let (count, elapsed_us) = if captured == 0 {
            let status = self.poll_capture_status(id)?.ok_or_else(|| Error::Configuration("capture is still running".into()))?;
            (status.count, status.elapsed_us)
        } else { (captured, elapsed_us) };
        let mut samples = Vec::with_capacity(count as usize);
        while samples.len() < count as usize {
            let limit = (count as usize - samples.len()).min(32768);
            let page: CaptureResult = self.rpc.call("logic.read", json!({"capture_id":id,"offset":samples.len(),"limit":limit}))?;
            if page.capture_id != id || page.samples.len() != limit {
                return Err(Error::Configuration("incomplete capture page".into()));
            }
            samples.extend(page.samples);
        }
        Ok(CaptureResult { capture_id: id, elapsed_us, samples })
    }
    pub fn start_capture_selected(&mut self, samples: u32, frequency: u32, edge: u8,
                                  pin: u8, timeout_ms: u16, channels: u32) -> Result<u32> {
        self.rpc.call("logic.start", json!({"count":samples,"rate":frequency,"trigger":edge,"pin":pin,"timeout_ms":timeout_ms,"channels":channels}))
    }
    pub fn cancel_capture(&mut self) -> Result<()> {
        self.rpc.call("logic.cancel", json!({}))
    }
}
