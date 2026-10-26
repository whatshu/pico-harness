//! `PhBoardLink` — the Debug-core control link, now carried by the PH protocol.
//!
//! The board used to expose a private PDBG vendor interface; the firmware
//! replaced it with the PH protocol on the CDC `PH Terminal` (see
//! `ph-bridge/proto/ph.proto` and `doc/ARCHITECTURE.md`). This type keeps the call
//! surface the desktop app already uses and forwards it to `ph_bridge`.
//!
//! Channels that moved to dedicated interfaces are rejected with
//! [`Error::InvalidArgument`] rather than silently mis-driving the board:
//!   - Work console  -> PH `ConsoleTx` / `ConsoleData` messages
//!   - bulk transfers -> vendor `PH Terminal Bulk` (route `WORK_SPI`)
//!   - logic capture  -> SUMP on the Logic Analyzer CDC (libsigrok `ols`)

use ph_bridge::client::SerialTransport;
use ph_bridge::{PhClient, PhError};

use crate::error::{Error, Result};
use crate::protocol::{
    CaptureResult, CaptureStatus, Command, Measurement, PowerStatus, UartStatus,
};
use crate::usb::{self, DeviceSelector};

fn map(error: PhError) -> Error {
    Error::UnexpectedResponse {
        command: 0,
        detail: error.to_string(),
    }
}

/// Capabilities that are no longer reachable through the control link.
fn moved(channel: &str) -> Error {
    Error::InvalidArgument(format!(
        "{channel} moved off the control link; see doc/ARCHITECTURE.md"
    ))
}

pub struct PhBoardLink {
    client: PhClient<SerialTransport>,
    pub serial: Option<String>,
    tty: String,
    pending: Vec<ph_bridge::msg::PhMessage>,
    bulk_writer: Option<ph_bridge::BulkWriter>,
}

impl PhBoardLink {
    pub fn is_usable(&self) -> bool {
        self.client.is_usable()
    }

    /// Daemon-only access to the firmware contract. The daemon assigns the
    /// sequence so callers sharing a device cannot collide on firmware IDs.
    pub fn ph_request(
        &mut self,
        mut msg: ph_bridge::msg::PhMessage,
    ) -> Result<ph_bridge::msg::PhMessage> {
        msg.seq = self.client.next_seq();
        self.client.request(&msg).map_err(map)
    }

    pub fn ph_events(&mut self) -> Result<Vec<ph_bridge::msg::PhMessage>> {
        let mut events = self.drain_events();
        events.extend(self.client.poll_events().map_err(map)?);
        Ok(events)
    }

    /// Drain events already collected by a transaction without blocking on
    /// serial input. The daemon can publish these while it still owns the lock.
    pub fn drain_events(&mut self) -> Vec<ph_bridge::msg::PhMessage> {
        let mut events = std::mem::take(&mut self.pending);
        events.extend(self.client.drain_events());
        events
    }

    /// Coordination and payload use the same selected board. Keeping the
    /// complete transaction here prevents interleaved Bulk begins.
    pub fn bulk_write(&mut self, data: &[u8], route: ph_bridge::msg::Route) -> Result<(bool, u32)> {
        let crc = crate::protocol::crc32c(data);
        // The daemon owns this interface for the lifetime of the control
        // handle. Re-enumerating and claiming USB for every packet dominated
        // small transfers; firmware updates drop both handles together.
        if self.bulk_writer.is_none() {
            self.bulk_writer =
                Some(ph_bridge::BulkWriter::open(self.serial.as_deref()).map_err(map)?);
        }
        let mut deferred = Vec::new();
        let result = (|| {
            self.client
                .bulk_tx_begin(route, data.len() as u32, crc)
                .map_err(map)?;
            self.bulk_writer
                .as_mut()
                .unwrap()
                .write(data)
                .map_err(map)?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
            loop {
                if std::time::Instant::now() >= deadline {
                    return Err(Error::Timeout);
                }
                let mut completion = None;
                for event in self.client.poll_events().map_err(map)? {
                    if let Some(ph_bridge::msg::ph_message::Body::BulkTxDone(ref done)) = event.body
                    {
                        completion = Some(if done.ok && done.crc_recv == crc {
                            Ok((true, crc))
                        } else {
                            Err(Error::InvalidArgument(format!(
                                "Bulk CRC failed: expected {crc:08x}, received {:08x}",
                                done.crc_recv
                            )))
                        });
                    } else {
                        if self.pending.len() + deferred.len()
                            >= ph_bridge::client::MAX_PENDING_EVENTS
                        {
                            return Err(map(PhError::EventOverflow));
                        }
                        deferred.push(event);
                    }
                }
                // Preserve events following BulkTxDone in the same USB read.
                if let Some(result) = completion {
                    return result;
                }
            }
        })();
        if result.is_err() {
            // An errored USB handle must be rediscovered on the next explicit
            // call. Never replay a payload whose completion is uncertain.
            self.bulk_writer = None;
            self.client.invalidate();
        }
        self.pending.extend(deferred);
        result
    }

    /// Explicit port constructor for controlled transports and daemon tests.
    /// Production discovery pins the port to the selected USB serial first.
    pub fn open_port(tty: String, serial: Option<String>) -> Result<Self> {
        let client = PhClient::open_serial(&tty).map_err(map)?;
        Ok(Self {
            client,
            serial,
            tty,
            pending: Vec::new(),
            bulk_writer: None,
        })
    }

    fn connect(serial: Option<String>) -> Result<Self> {
        let devices = ph_bridge::device::scan();
        let chosen = devices
            .into_iter()
            .find(|d| match (&serial, &d.serial) {
                (Some(want), Some(got)) => want == got,
                (Some(_), None) => false,
                (None, _) => true,
            })
            .ok_or_else(|| {
                Error::DeviceNotFound(match &serial {
                    Some(s) => format!(" (serial {s})"),
                    None => String::new(),
                })
            })?;
        let tty = chosen
            .terminal_tty
            .ok_or_else(|| Error::DeviceNotFound(" (no PH Terminal tty)".into()))?
            .to_string_lossy()
            .into_owned();
        let client = PhClient::open_serial(&tty).map_err(map)?;
        Ok(Self {
            client,
            serial: chosen.serial,
            tty,
            pending: Vec::new(),
            bulk_writer: None,
        })
    }

    pub fn open() -> Result<Self> {
        Self::connect(None)
    }

    pub fn open_serial(serial: impl Into<String>) -> Result<Self> {
        Self::connect(Some(serial.into()))
    }

    pub fn open_with_selector(selector: DeviceSelector) -> Result<Self> {
        Self::connect(selector.serial.clone())
    }

    /// The control port this link is bound to (`/dev/ttyACM*`).
    pub fn port(&self) -> &str {
        &self.tty
    }

    /// USB topology of the probe this link is bound to. The control link is a
    /// serial port, so the bus/port chain is read back from the enumerator.
    pub fn usb_location(&self) -> Result<usb::Location> {
        for device in rusb::devices()?.iter() {
            let descriptor = device.device_descriptor()?;
            if descriptor.vendor_id() != usb::USB_VENDOR_ID
                || descriptor.product_id() != usb::USB_PRODUCT_ID
            {
                continue;
            }
            let serial = device
                .open()
                .ok()
                .and_then(|handle| handle.read_serial_number_string_ascii(&descriptor).ok());
            if serial == self.serial {
                return Ok(usb::Location {
                    bus: device.bus_number(),
                    ports: device.port_numbers().unwrap_or_default(),
                });
            }
        }
        Err(Error::DeviceNotFound(String::new()))
    }

    pub fn interface_number(&self) -> u8 {
        1 // CDC "PH Terminal" control interface
    }

    // ---- control plane ---------------------------------------------------

    pub fn info(&mut self) -> Result<String> {
        Ok(self.client.get_info().map_err(map)?.info)
    }

    pub fn status(&mut self) -> Result<PowerStatus> {
        let s = self.client.get_status().map_err(map)?;
        Ok(PowerStatus {
            power_on: s.power_enabled,
            power_good: s.power_good,
            alert: s.ina_alert,
            power_fault: s.work_fault,
            work_3v3_mv: s.vbus_mv as u16,
        })
    }

    pub fn set_power(&mut self, enabled: bool) -> Result<()> {
        self.client.power_set(enabled).map_err(map)
    }

    pub fn reset_pulse(&mut self, milliseconds: u16) -> Result<()> {
        self.client
            .reset_pulse(u32::from(milliseconds))
            .map_err(map)
    }

    pub fn measure(&mut self) -> Result<Measurement> {
        let m = self.client.measure().map_err(map)?;
        Ok(Measurement {
            shunt_uv: f64::from(m.shunt_uv),
            bus_mv: f64::from(m.vbus_uv) / 1000.0,
            current_ma: f64::from(m.current_ua) / 1000.0,
            power_mw: f64::from(m.power_uw) / 1000.0,
        })
    }

    pub fn enter_debug_bootsel(&mut self) -> Result<()> {
        self.client.debug_bootsel().map_err(map)
    }

    /// Short Work console write (PH `ConsoleTx`, up to 128 bytes).
    pub fn console_write(&mut self, data: &[u8]) -> Result<()> {
        self.client.console_tx(data).map_err(map)
    }

    /// Work UART baud through the pico_harness baud transaction.
    pub fn console_baud(&mut self, baud: u32) -> Result<u32> {
        self.client.term_config(baud, 8, 1, 0).map_err(map)
    }

    /// Console bridge status from the Debug core:
    /// `(mode, actual_baud, rx_available, tx_space)` where mode 1 = harness.
    pub fn term_status(&mut self) -> Result<(i32, u32, u32, u32)> {
        let st = self.client.term_status().map_err(map)?;
        Ok((st.mode, st.actual_baud, st.rx_available, st.tx_space))
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

    pub fn uart_read(&mut self, _channel: u8, _maximum: u16) -> Result<Vec<u8>> {
        use ph_bridge::msg::ph_message;
        let mut out = Vec::new();
        for event in self
            .client
            .recv_events_short(std::time::Duration::from_millis(6))
            .map_err(map)?
        {
            if let Some(ph_message::Body::ConsoleData(d)) = event.body {
                out.extend_from_slice(&d.data);
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

    pub fn capture(&mut self, _samples: u32, _frequency: u32) -> Result<CaptureResult> {
        Err(moved("logic capture (use SUMP on the Logic Analyzer CDC)"))
    }

    pub fn capture_triggered(
        &mut self,
        _samples: u32,
        _frequency: u32,
        _edge: u8,
        _pin: u8,
        _timeout_ms: u16,
    ) -> Result<CaptureResult> {
        Err(moved("logic capture (use SUMP on the Logic Analyzer CDC)"))
    }

    pub fn start_capture(&mut self, _samples: u32, _frequency: u32) -> Result<u32> {
        Err(moved("logic capture (use SUMP on the Logic Analyzer CDC)"))
    }

    pub fn start_capture_triggered(
        &mut self,
        _samples: u32,
        _frequency: u32,
        _edge: u8,
        _pin: u8,
        _timeout_ms: u16,
    ) -> Result<u32> {
        Err(moved("logic capture (use SUMP on the Logic Analyzer CDC)"))
    }

    pub fn poll_capture(&mut self, _capture_id: u32) -> Result<Option<(u32, u32, u32)>> {
        Err(moved("logic capture (use SUMP on the Logic Analyzer CDC)"))
    }

    pub fn poll_capture_status(&mut self, _capture_id: u32) -> Result<Option<CaptureStatus>> {
        Err(moved("logic capture (use SUMP on the Logic Analyzer CDC)"))
    }

    pub fn read_capture(
        &mut self,
        _capture_id: u32,
        _captured: u32,
        _elapsed_us: u32,
    ) -> Result<CaptureResult> {
        Err(moved("logic capture (use SUMP on the Logic Analyzer CDC)"))
    }

    pub fn cancel_capture(&mut self) -> Result<()> {
        Err(moved("logic capture (use SUMP on the Logic Analyzer CDC)"))
    }
}
