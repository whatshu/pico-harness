use std::time::{Duration, Instant};

use rusb::{DeviceHandle, GlobalContext};

use crate::error::{Error, Result};
use crate::protocol::{
    CaptureResult, Command, Frame, Measurement, PowerStatus, UartStatus, HEADER_SIZE, MAX_PAYLOAD,
};
use crate::usb::{self, DeviceSelector};

const PDBG_INTERFACE_CLASS: u8 = 0xff;
const PDBG_INTERFACE_SUBCLASS: u8 = 0x50;
const PDBG_INTERFACE_PROTOCOL: u8 = 0x01;
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(2000);

pub struct PdbgLink {
    handle: DeviceHandle<GlobalContext>,
    interface: u8,
    endpoint_out: u8,
    endpoint_in: u8,
    sequence: u32,
    receive_buffer: Vec<u8>,
    pub serial: Option<String>,
}

impl PdbgLink {
    pub fn usb_location(&self) -> Result<usb::Location> {
        let device = self.handle.device();
        Ok(usb::Location {
            bus: device.bus_number(),
            ports: device.port_numbers()?,
        })
    }
    pub fn enter_debug_bootsel(&mut self) -> Result<()> {
        self.command_ok(Command::DebugEnterBootsel, b"BOOT")
            .map(|_| ())
    }
    pub fn open() -> Result<Self> {
        Self::open_with_selector(DeviceSelector::any())
    }

    pub fn open_serial(serial: impl Into<String>) -> Result<Self> {
        Self::open_with_selector(DeviceSelector::serial(serial))
    }

    pub fn open_with_selector(selector: DeviceSelector) -> Result<Self> {
        let opened = usb::open_selector(
            &selector,
            PDBG_INTERFACE_CLASS,
            PDBG_INTERFACE_SUBCLASS,
            PDBG_INTERFACE_PROTOCOL,
        )?;
        Ok(Self {
            handle: opened.handle,
            interface: opened.interface,
            endpoint_out: opened.endpoint_out,
            endpoint_in: opened.endpoint_in,
            sequence: 0,
            receive_buffer: Vec::with_capacity(HEADER_SIZE + MAX_PAYLOAD + 4),
            serial: opened.serial,
        })
    }

    pub fn interface_number(&self) -> u8 {
        self.interface
    }

    pub fn command(&mut self, command: Command, payload: &[u8]) -> Result<Vec<u8>> {
        self.command_timeout(command, payload, DEFAULT_TIMEOUT)
    }

    pub fn command_timeout(
        &mut self,
        command: Command,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let sequence = self.next_sequence();
        let request = Frame::request(command, sequence, payload).encode()?;
        self.handle
            .write_bulk(self.endpoint_out, &request, timeout)?;
        let response = self.read_frame(timeout)?;
        if response.sequence != sequence {
            return Err(Error::UnexpectedResponse {
                command: command.into(),
                detail: format!("sequence {}, expected {sequence}", response.sequence),
            });
        }
        if response.command != command.response() {
            return Err(Error::UnexpectedResponse {
                command: command.into(),
                detail: format!("command 0x{:02x}", response.command),
            });
        }
        Ok(response.payload)
    }

    pub fn info(&mut self) -> Result<String> {
        let payload = self.command_ok(Command::GetInfo, &[])?;
        let end = payload
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(payload.len());
        Ok(String::from_utf8_lossy(&payload[..end]).into_owned())
    }

    pub fn status(&mut self) -> Result<PowerStatus> {
        let payload = self.command(Command::GetStatus, &[])?;
        PowerStatus::decode(&payload).map_err(|error| match error {
            crate::protocol::ProtocolError::DeviceStatus(status) => Error::DeviceStatus {
                command: Command::GetStatus.into(),
                status,
            },
            other => Error::Protocol(other),
        })
    }

    pub fn set_power(&mut self, enabled: bool) -> Result<()> {
        self.command_ok(Command::PowerSet, &[enabled as u8])
            .map(|_| ())
    }

    pub fn reset_pulse(&mut self, milliseconds: u16) -> Result<()> {
        if milliseconds == 0 || milliseconds > 1000 {
            return Err(Error::InvalidArgument(
                "reset pulse must be 1..1000 ms".into(),
            ));
        }
        self.command_ok(Command::ResetPulse, &milliseconds.to_le_bytes())
            .map(|_| ())
    }

    pub fn set_bootsel(&mut self, asserted: bool) -> Result<()> {
        self.command_ok(Command::BootselSet, &[asserted as u8])
            .map(|_| ())
    }

    pub fn enter_bootsel(&mut self) -> Result<()> {
        self.command_ok(Command::EnterBootsel, &[]).map(|_| ())
    }

    pub fn measure(&mut self) -> Result<Measurement> {
        let payload = self.command(Command::Measure, &[])?;
        Measurement::decode(&payload).map_err(|error| match error {
            crate::protocol::ProtocolError::DeviceStatus(status) => Error::DeviceStatus {
                command: Command::Measure.into(),
                status,
            },
            other => Error::Protocol(other),
        })
    }

    pub fn gpio_read(&mut self) -> Result<u32> {
        let payload = self.command_ok(Command::GpioRead, &[])?;
        if payload.len() != 4 {
            return Err(Error::UnexpectedResponse {
                command: Command::GpioRead.into(),
                detail: format!("{} GPIO bytes", payload.len()),
            });
        }
        Ok(u32::from_le_bytes(payload.try_into().unwrap()))
    }

    pub fn uart_config(
        &mut self,
        channel: u8,
        baud: u32,
        data_bits: u8,
        stop_bits: u8,
        parity: u8,
    ) -> Result<u32> {
        if channel != 0
            || !(5..=8).contains(&data_bits)
            || !(1..=2).contains(&stop_bits)
            || parity > 2
        {
            return Err(Error::InvalidArgument(
                "unsupported UART configuration".into(),
            ));
        }
        let mut request = Vec::with_capacity(8);
        request.push(channel);
        request.extend_from_slice(&baud.to_le_bytes());
        request.extend_from_slice(&[data_bits, stop_bits, parity]);
        let payload = self.command_ok(Command::UartConfig, &request)?;
        if payload.len() != 4 {
            return Err(Error::UnexpectedResponse {
                command: Command::UartConfig.into(),
                detail: format!("{} response bytes", payload.len()),
            });
        }
        Ok(u32::from_le_bytes(payload.try_into().unwrap()))
    }

    pub fn uart_status(&mut self, channel: u8) -> Result<UartStatus> {
        let payload = self.command_ok(Command::UartStatus, &[channel])?;
        if payload.len() != 8 {
            return Err(Error::UnexpectedResponse {
                command: Command::UartStatus.into(),
                detail: format!("{} response bytes", payload.len()),
            });
        }
        Ok(UartStatus {
            rx_available: u16::from_le_bytes([payload[0], payload[1]]),
            tx_space: u16::from_le_bytes([payload[2], payload[3]]),
            rx_overruns: u32::from_le_bytes(payload[4..8].try_into().unwrap()),
        })
    }

    /// Queue one bounded UART chunk without retrying a full transmit ring.
    /// The returned count lets callers enforce their own deadline and cancellation.
    pub fn uart_write_some(&mut self, channel: u8, data: &[u8]) -> Result<usize> {
        let size = data.len().min(MAX_PAYLOAD - 1);
        let mut request = Vec::with_capacity(1 + size);
        request.push(channel);
        request.extend_from_slice(&data[..size]);
        let payload = self.command_ok(Command::UartWrite, &request)?;
        if payload.len() != 2 {
            return Err(Error::UnexpectedResponse {
                command: Command::UartWrite.into(),
                detail: format!("{} response bytes", payload.len()),
            });
        }
        let accepted = u16::from_le_bytes([payload[0], payload[1]]) as usize;
        if accepted > size {
            return Err(Error::UnexpectedResponse {
                command: Command::UartWrite.into(),
                detail: "UART accepted more bytes than submitted".into(),
            });
        }
        Ok(accepted)
    }

    pub fn uart_write(&mut self, channel: u8, data: &[u8], timeout: Duration) -> Result<usize> {
        let deadline = Instant::now() + timeout;
        let mut written = 0;
        while written < data.len() {
            let end = (written + MAX_PAYLOAD - 1).min(data.len());
            let accepted = self.uart_write_some(channel, &data[written..end])?;
            written += accepted;
            if accepted == 0 {
                if Instant::now() >= deadline {
                    return Err(Error::Timeout);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        Ok(written)
    }

    pub fn uart_read(&mut self, channel: u8, maximum: u16) -> Result<Vec<u8>> {
        if maximum as usize > MAX_PAYLOAD - 1 {
            return Err(Error::InvalidArgument(
                "UART read is larger than PDBG payload".into(),
            ));
        }
        let mut request = Vec::with_capacity(3);
        request.push(channel);
        request.extend_from_slice(&maximum.to_le_bytes());
        self.command_ok(Command::UartRead, &request)
    }

    pub fn spi_config(&mut self, baud: u32, mode: u8) -> Result<u32> {
        if mode > 3 {
            return Err(Error::InvalidArgument("SPI mode must be 0..3".into()));
        }
        let mut request = Vec::with_capacity(5);
        request.extend_from_slice(&baud.to_le_bytes());
        request.push(mode);
        let payload = self.command_ok(Command::SpiConfig, &request)?;
        if payload.len() != 4 {
            return Err(Error::UnexpectedResponse {
                command: Command::SpiConfig.into(),
                detail: format!("{} response bytes", payload.len()),
            });
        }
        Ok(u32::from_le_bytes(payload.try_into().unwrap()))
    }

    pub fn spi_transfer(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        if data.is_empty() || data.len() > MAX_PAYLOAD - 1 {
            return Err(Error::InvalidArgument(
                "SPI transfer must contain 1..4095 bytes".into(),
            ));
        }
        self.command_ok(Command::SpiTransfer, data)
    }

    pub fn capture(&mut self, samples: u32, frequency: u32) -> Result<CaptureResult> {
        self.capture_triggered(samples, frequency, 0, 5, 1000)
    }
    pub fn capture_triggered(
        &mut self,
        samples: u32,
        frequency: u32,
        edge: u8,
        pin: u8,
        timeout_ms: u16,
    ) -> Result<CaptureResult> {
        if !(1..=32768).contains(&samples) || frequency == 0 {
            return Err(Error::InvalidArgument(
                "capture size or frequency is invalid".into(),
            ));
        }
        let mut request = Vec::with_capacity(8);
        request.extend_from_slice(&samples.to_le_bytes());
        request.extend_from_slice(&frequency.to_le_bytes());
        if edge != 0 {
            request.extend([edge, pin]);
            request.extend(timeout_ms.to_le_bytes());
            request.extend([0, 0]);
        }
        let milliseconds = (samples as u64 * 2000 / frequency as u64).max(if edge == 0 {
            5000
        } else {
            timeout_ms as u64
        }) + 2000;
        let payload = self.command_ok_timeout(
            Command::Capture,
            &request,
            Duration::from_millis(milliseconds),
        )?;
        if payload.len() != 12 {
            return Err(Error::UnexpectedResponse {
                command: Command::Capture.into(),
                detail: format!("{} response bytes", payload.len()),
            });
        }
        let capture_id = u32::from_le_bytes(payload[0..4].try_into().unwrap());
        let captured = u32::from_le_bytes(payload[4..8].try_into().unwrap());
        let elapsed_us = u32::from_le_bytes(payload[8..12].try_into().unwrap());
        if captured != samples || elapsed_us == 0 {
            return Err(Error::UnexpectedResponse {
                command: Command::Capture.into(),
                detail: format!("captured {captured} samples in {elapsed_us} us"),
            });
        }
        self.read_capture(capture_id, captured, elapsed_us)
    }

    pub fn start_capture(&mut self, samples: u32, frequency: u32) -> Result<u32> {
        self.start_capture_triggered(samples, frequency, 0, 5, 1000)
    }
    pub fn start_capture_triggered(
        &mut self,
        samples: u32,
        frequency: u32,
        edge: u8,
        pin: u8,
        timeout_ms: u16,
    ) -> Result<u32> {
        if !(1..=32768).contains(&samples) || frequency < 2500 {
            return Err(Error::InvalidArgument(
                "Invalid capture size or rate".into(),
            ));
        }
        let mut request = samples.to_le_bytes().to_vec();
        request.extend(frequency.to_le_bytes());
        if edge != 0 {
            request.extend([edge, pin]);
            request.extend(timeout_ms.to_le_bytes());
            request.extend([0, 0]);
        }
        let response = self.command_ok(Command::CaptureStart, &request)?;
        Ok(u32::from_le_bytes(response.try_into().map_err(|_| {
            Error::InvalidArgument("Invalid capture ID reply".into())
        })?))
    }
    pub fn poll_capture(&mut self, capture_id: u32) -> Result<Option<(u32, u32, u32)>> {
        self.poll_capture_status(capture_id)
            .map(|status| status.map(|s| (s.sample_hz, s.count, s.elapsed_us)))
    }

    pub fn poll_capture_status(
        &mut self,
        capture_id: u32,
    ) -> Result<Option<crate::protocol::CaptureStatus>> {
        let response = self.command_ok(Command::CapturePoll, &[])?;
        if response.len() != 20 {
            return Err(Error::InvalidArgument(
                "Invalid capture status reply".into(),
            ));
        }
        let word = |i| u32::from_le_bytes(response[i..i + 4].try_into().unwrap());
        if word(0) != capture_id {
            return Err(Error::InvalidArgument("Capture was replaced".into()));
        }
        let flags = word(12);
        if flags & 16 != 0 {
            return Ok(None);
        }
        if flags & 14 != 0 {
            let message = if flags & 4 != 0 {
                "Trigger timed out; no matching edge was captured"
            } else if flags & 8 != 0 {
                "Capture cancelled"
            } else {
                "Capture stalled; select a lower sample rate"
            };
            return Err(Error::InvalidArgument(message.into()));
        }
        let count = word(16);
        let trigger_index = (flags & 32 != 0).then_some(flags >> 16);
        if trigger_index.is_some_and(|index| index >= count) {
            return Err(Error::InvalidArgument(
                "Invalid pre-trigger position".into(),
            ));
        }
        Ok(Some(crate::protocol::CaptureStatus {
            sample_hz: word(4),
            count,
            elapsed_us: word(8),
            trigger_index,
        }))
    }
    pub fn cancel_capture(&mut self) -> Result<()> {
        self.command_ok(Command::CaptureCancel, &[]).map(|_| ())
    }

    pub fn read_capture(
        &mut self,
        capture_id: u32,
        captured: u32,
        elapsed_us: u32,
    ) -> Result<CaptureResult> {
        if !(1..=32768).contains(&captured) || elapsed_us == 0 {
            return Err(Error::InvalidArgument("Invalid completed capture".into()));
        }
        let mut values = Vec::with_capacity(captured as usize);
        let mut offset = 0u32;
        while offset < captured {
            let count = (captured - offset).min(1023) as u16;
            let mut read_request = Vec::with_capacity(10);
            read_request.extend_from_slice(&capture_id.to_le_bytes());
            read_request.extend_from_slice(&offset.to_le_bytes());
            read_request.extend_from_slice(&count.to_le_bytes());
            let chunk = self.command_ok(Command::CaptureRead, &read_request)?;
            if chunk.len() != count as usize * 4 {
                return Err(Error::UnexpectedResponse {
                    command: Command::CaptureRead.into(),
                    detail: format!("{} bytes for {count} samples", chunk.len()),
                });
            }
            for sample in chunk.chunks_exact(4) {
                values.push(u32::from_le_bytes(sample.try_into().unwrap()));
            }
            offset += count as u32;
        }
        Ok(CaptureResult {
            capture_id,
            elapsed_us,
            samples: values,
        })
    }

    fn command_ok(&mut self, command: Command, payload: &[u8]) -> Result<Vec<u8>> {
        self.command_ok_timeout(command, payload, DEFAULT_TIMEOUT)
    }

    fn command_ok_timeout(
        &mut self,
        command: Command,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let response = self.command_timeout(command, payload, timeout)?;
        if response.is_empty() {
            return Err(Error::UnexpectedResponse {
                command: command.into(),
                detail: "empty status payload".into(),
            });
        }
        if response[0] != 0 {
            return Err(Error::DeviceStatus {
                command: command.into(),
                status: response[0],
            });
        }
        Ok(response[1..].to_vec())
    }

    fn next_sequence(&mut self) -> u32 {
        self.sequence = self.sequence.wrapping_add(1).max(1);
        self.sequence
    }

    fn read_frame(&mut self, timeout: Duration) -> Result<Frame> {
        let deadline = Instant::now() + timeout;
        let mut chunk = [0u8; 512];
        loop {
            if let Some(total) = Frame::total_length_from_prefix(&self.receive_buffer)? {
                if self.receive_buffer.len() >= total {
                    let packet: Vec<u8> = self.receive_buffer.drain(..total).collect();
                    return Ok(Frame::decode(&packet)?);
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout);
            }
            let count = self
                .handle
                .read_bulk(self.endpoint_in, &mut chunk, remaining)?;
            if count == 0 {
                return Err(Error::Timeout);
            }
            self.receive_buffer.extend_from_slice(&chunk[..count]);
        }
    }
}

impl Drop for PdbgLink {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(self.interface);
    }
}
