use std::time::Duration;

use rusb::{DeviceHandle, GlobalContext};

use crate::error::{Error, Result};
use crate::usb::{self, DeviceSelector};

const DAP_INTERFACE_CLASS: u8 = 0xff;
const DAP_INTERFACE_SUBCLASS: u8 = 0x00;
const DAP_INTERFACE_PROTOCOL: u8 = 0x00;
// CMSIS-DAP v2 devices commonly start with a 64-byte full-speed packet and
// advertise a larger logical packet through DAP_Info. The dualcore firmware has
// shipped with both 64 and 512 byte configurations, so discover the size
// instead of assuming the current source-tree value.
const DEFAULT_PACKET_SIZE: usize = 64;
const COMMAND_INFO: u8 = 0x00;
const COMMAND_CONNECT: u8 = 0x02;
const COMMAND_DISCONNECT: u8 = 0x03;
const COMMAND_TRANSFER_CONFIGURE: u8 = 0x04;
const COMMAND_TRANSFER: u8 = 0x05;
const COMMAND_WRITE_ABORT: u8 = 0x08;
const COMMAND_SWJ_PINS: u8 = 0x10;
const COMMAND_SWJ_CLOCK: u8 = 0x11;
const COMMAND_SWJ_SEQUENCE: u8 = 0x12;
const COMMAND_SWD_CONFIGURE: u8 = 0x13;
const COMMAND_SWD_SEQUENCE: u8 = 0x1d;
const INFO_PACKET_SIZE: u8 = 0xff;
const PORT_SWD: u8 = 1;
const TRANSFER_APND: u8 = 1 << 0;
const TRANSFER_RNW: u8 = 1 << 1;
const TRANSFER_OK: u8 = 1;
const DP_IDCODE: u8 = 0x00;
const DP_CTRL_STAT: u8 = 0x04;
const DP_SELECT: u8 = 0x08;
// RP2040 exposes its two Cortex-M0+ debug ports through the ADIv5 SWD
// multidrop target-selection register.  The value below is the target ID
// from the RP2040 OpenOCD target description; the high nibble selects the
// core instance (0 = core 0, 1 = core 1).
const RP2040_TARGET_ID: u32 = 0x0100_2927;
const AP_CSW: u8 = 0x00;
const AP_TAR: u8 = 0x04;
const AP_DRW: u8 = 0x0c;
const ADIV6_MEM_AP_BANK: u16 = 0x0d00;
const CTRL_STAT_POWERUP_ACK: u32 = 0xa000_0000;
const CTRL_STAT_POWERUP_REQ: u32 = 0x5000_0000;
const SWJ_NRESET: u8 = 1 << 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferRequest {
    Read { ap: bool, address: u8 },
    Write { ap: bool, address: u8, value: u32 },
}

impl TransferRequest {
    pub const fn dp_read(address: u8) -> Self {
        Self::Read { ap: false, address }
    }

    pub const fn dp_write(address: u8, value: u32) -> Self {
        Self::Write {
            ap: false,
            address,
            value,
        }
    }

    pub const fn ap_read(address: u8) -> Self {
        Self::Read { ap: true, address }
    }

    pub const fn ap_write(address: u8, value: u32) -> Self {
        Self::Write {
            ap: true,
            address,
            value,
        }
    }

    fn request_byte(self) -> u8 {
        let (ap, address, read) = match self {
            Self::Read { ap, address } => (ap, address, true),
            Self::Write { ap, address, .. } => (ap, address, false),
        };
        (if ap { TRANSFER_APND } else { 0 })
            | (if read { TRANSFER_RNW } else { 0 })
            | (address & 0x0c)
    }
}

pub struct CmsisDap {
    handle: DeviceHandle<GlobalContext>,
    interface: u8,
    endpoint_out: u8,
    endpoint_in: u8,
    packet_size: usize,
    timeout: Duration,
    connected: bool,
    adiv6_ap: Option<u32>,
}

impl CmsisDap {
    pub fn open() -> Result<Self> {
        Self::open_with_selector(DeviceSelector::any())
    }

    pub fn open_serial(serial: impl Into<String>) -> Result<Self> {
        Self::open_with_selector(DeviceSelector::serial(serial))
    }

    pub fn open_with_selector(selector: DeviceSelector) -> Result<Self> {
        let opened = usb::open_selector(
            &selector,
            DAP_INTERFACE_CLASS,
            DAP_INTERFACE_SUBCLASS,
            DAP_INTERFACE_PROTOCOL,
        )?;
        let usb::OpenedInterface {
            handle,
            interface,
            endpoint_out,
            endpoint_in,
        } = opened;
        Ok(Self {
            handle,
            interface,
            endpoint_out,
            endpoint_in,
            packet_size: DEFAULT_PACKET_SIZE,
            timeout: Duration::from_millis(2000),
            connected: false,
            adiv6_ap: None,
        })
    }

    pub fn packet_size(&self) -> usize {
        self.packet_size
    }

    pub fn info(&mut self, id: u8) -> Result<Vec<u8>> {
        let response = self.command(&[COMMAND_INFO, id])?;
        if response.len() < 2 || response[0] != COMMAND_INFO {
            return Err(Error::UnexpectedResponse {
                command: COMMAND_INFO,
                detail: "invalid DAP_Info response".into(),
            });
        }
        let length = response[1] as usize;
        if response.len() < 2 + length {
            return Err(Error::UnexpectedResponse {
                command: COMMAND_INFO,
                detail: "truncated DAP_Info response".into(),
            });
        }
        Ok(response[2..2 + length].to_vec())
    }

    /// Connect to the Work core through SWD.
    ///
    /// The dualcore socket accepts both ordinary SWD targets (such as RP2350) and
    /// the RP2040's SWD multidrop target.  Try the ordinary ARM sequence first
    /// and fall back to RP2040 target selection when the first DP read does
    /// not receive an ACK.
    pub fn connect_swd(&mut self, clock_hz: u32) -> Result<()> {
        self.prepare_swd(clock_hz)?;
        match self.connect_standard_swd() {
            Ok(()) => {
                self.connected = true;
                Ok(())
            }
            Err(standard_error) => match self.connect_rp2040_target(0) {
                Ok(()) => {
                    self.connected = true;
                    Ok(())
                }
                Err(multidrop_error) => Err(Error::UnexpectedResponse {
                    command: COMMAND_TRANSFER,
                    detail: format!(
                        "ordinary SWD failed: {standard_error}; RP2040 multidrop failed: {multidrop_error}"
                    ),
                }),
            },
        }
    }

    pub fn connect_swd_adiv6(&mut self, clock_hz: u32, ap_base: u32) -> Result<()> {
        if ap_base & 0x0fff != 0 {
            return Err(Error::InvalidArgument(
                "ADIv6 AP base must be 4 KiB aligned".into(),
            ));
        }
        self.prepare_swd(clock_hz)?;
        self.connect_standard_swd()?;
        self.adiv6_ap = Some(ap_base);
        self.connected = true;
        Ok(())
    }

    /// Connect to one RP2040 Cortex-M0+ core through SWD.
    ///
    /// RP2040 starts in the SWD multidrop dormant state.  A plain
    /// JTAG-to-SWD sequence leaves the target unselected and every following
    /// DP transfer receives a WAIT/FAULT ACK.  Enter dormant explicitly,
    /// activate SWD, and write TARGETSEL before the first DP read.
    pub fn connect_swd_core(&mut self, clock_hz: u32, core: u8) -> Result<()> {
        if core > 1 {
            return Err(Error::InvalidArgument(
                "RP2040 SWD core must be 0 or 1".into(),
            ));
        }
        self.prepare_swd(clock_hz)?;
        self.connect_rp2040_target(core)?;
        self.connected = true;
        Ok(())
    }

    fn prepare_swd(&mut self, clock_hz: u32) -> Result<()> {
        self.adiv6_ap = None;
        let packet_size = self.info(INFO_PACKET_SIZE)?;
        if packet_size.len() == 2 {
            let size = u16::from_le_bytes([packet_size[0], packet_size[1]]) as usize;
            if (64..=32768).contains(&size) {
                self.packet_size = size;
            }
        }
        let response = self.command(&[COMMAND_CONNECT, PORT_SWD])?;
        if response.len() < 2 || response[0] != COMMAND_CONNECT || response[1] != PORT_SWD {
            return Err(Error::UnexpectedResponse {
                command: COMMAND_CONNECT,
                detail: "SWD port was not selected".into(),
            });
        }
        self.expect_status(&[COMMAND_SWD_CONFIGURE, 0], COMMAND_SWD_CONFIGURE)?;
        self.expect_status(
            &[COMMAND_TRANSFER_CONFIGURE, 0, 0x00, 0x00, 0x00, 0x00],
            COMMAND_TRANSFER_CONFIGURE,
        )?;
        self.set_clock(clock_hz)?;
        // PORT_SWD_SETUP in the dualcore firmware initializes nRESET low. Release
        // the Work core before asking its debug port for an IDCODE.
        self.set_reset(false)?;
        Ok(())
    }

    fn connect_standard_swd(&mut self) -> Result<()> {
        // Complete ARM JTAG-to-SWD sequence: 50 high clocks, activation
        // pattern, 50 high clocks, and two idle clocks.
        self.swj_sequence(
            136,
            &[
                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x9e, 0xe7, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0x00,
            ],
        )?;
        self.read_dp_idcode().map(|_| ())
    }

    fn connect_rp2040_target(&mut self, core: u8) -> Result<()> {
        // RP2040 implements ADIv5 SWD multidrop.  The standard JTAG-to-SWD
        // sequence is insufficient because it does not leave a multidrop
        // target selected.  These sequences are the ARM-defined bit streams
        // used by OpenOCD (bytes are transmitted LSB-first by CMSIS-DAP).
        self.swj_sequence(40, &[0xff, 0x75, 0x77, 0x77, 0x67])?;
        self.swj_sequence(
            224,
            &[
                0xff, 0x92, 0xf3, 0x09, 0x62, 0x95, 0x2d, 0x85, 0x86, 0xe9, 0xaf, 0xdd, 0xe3, 0xa2,
                0x0e, 0xbc, 0x19, 0xa0, 0xf1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00,
            ],
        )?;
        let targetsel = RP2040_TARGET_ID | ((core as u32) << 28);
        self.select_targetsel(targetsel)?;
        // A DPIDR read is the first transfer allowed after TARGETSEL and also
        // makes connection errors visible at connect time.
        self.read_dp_idcode()?;
        Ok(())
    }

    pub fn disconnect(&mut self) -> Result<()> {
        if self.connected {
            self.expect_status(&[COMMAND_DISCONNECT], COMMAND_DISCONNECT)?;
            self.connected = false;
        }
        self.adiv6_ap = None;
        Ok(())
    }

    pub fn set_clock(&mut self, hz: u32) -> Result<()> {
        if hz == 0 {
            return Err(Error::InvalidArgument("SWD clock must be non-zero".into()));
        }
        let mut request = vec![COMMAND_SWJ_CLOCK];
        request.extend_from_slice(&hz.to_le_bytes());
        self.expect_status(&request, COMMAND_SWJ_CLOCK)
    }

    pub fn swj_sequence(&mut self, clocks: u8, data: &[u8]) -> Result<()> {
        let expected_bytes = (if clocks == 0 { 256 } else { clocks as usize }).div_ceil(8);
        if data.len() != expected_bytes {
            return Err(Error::InvalidArgument(format!(
                "SWJ sequence of {clocks} clocks needs {expected_bytes} bytes"
            )));
        }
        let mut request = Vec::with_capacity(2 + data.len());
        request.extend_from_slice(&[COMMAND_SWJ_SEQUENCE, clocks]);
        request.extend_from_slice(data);
        self.expect_status(&request, COMMAND_SWJ_SEQUENCE)
    }

    /// Select an ADIv5 multidrop target using the CMSIS-DAP SWD sequence
    /// command.  DP_TARGETSEL deliberately has no normal SWD ACK, so it must
    /// be emitted as a raw sequence rather than DAP_Transfer.
    fn select_targetsel(&mut self, targetsel: u32) -> Result<()> {
        // The request byte is LSB-first and includes START, A[3:2], STOP and
        // PARK.  The address is shifted one bit in the on-wire SWD packet.
        const TARGETSEL_REQUEST: u8 = 0x99;
        let parity = (targetsel.count_ones() & 1) as u8;
        let mut request = vec![
            COMMAND_SWD_SEQUENCE,
            3, // request, ACK/turnaround capture, data + parity
            8,
            TARGETSEL_REQUEST,
            0x80 | 5, // capture turnaround, ACK and turnaround
            33,
        ];
        request.extend_from_slice(&targetsel.to_le_bytes());
        request.push(parity);
        let response = self.command(&request)?;
        if response.len() < 2 || response[0] != COMMAND_SWD_SEQUENCE {
            return Err(Error::UnexpectedResponse {
                command: COMMAND_SWD_SEQUENCE,
                detail: "invalid DAP_SWD_Sequence response".into(),
            });
        }
        if response[1] != 0 {
            return Err(Error::UnexpectedResponse {
                command: COMMAND_SWD_SEQUENCE,
                detail: format!("status 0x{:02x}", response[1]),
            });
        }
        Ok(())
    }

    pub fn reset_target(&mut self, milliseconds: u32) -> Result<()> {
        self.set_reset(true)?;
        std::thread::sleep(Duration::from_millis(milliseconds as u64));
        self.set_reset(false)
    }

    /// Set the Work-core nRESET line. `asserted` is active low at the target.
    pub fn set_reset(&mut self, asserted: bool) -> Result<()> {
        let value = if asserted { 0 } else { SWJ_NRESET };
        let request = [COMMAND_SWJ_PINS, value, SWJ_NRESET, 0, 0, 0, 0];
        self.expect_swj_pins(&request)
    }

    pub fn transfer(&mut self, transfers: &[TransferRequest]) -> Result<Vec<u32>> {
        if transfers.is_empty() || transfers.len() > 255 {
            return Err(Error::InvalidArgument(
                "DAP transfer count must be 1..255".into(),
            ));
        }
        let mut request = vec![COMMAND_TRANSFER, 0, transfers.len() as u8];
        for transfer in transfers {
            request.push(transfer.request_byte());
            if let TransferRequest::Write { value, .. } = transfer {
                request.extend_from_slice(&value.to_le_bytes());
            }
        }
        let response = self.command(&request)?;
        if response.len() < 3 || response[0] != COMMAND_TRANSFER {
            return Err(Error::UnexpectedResponse {
                command: COMMAND_TRANSFER,
                detail: format!("invalid DAP_Transfer response: {response:?}"),
            });
        }
        let completed = response[1] as usize;
        let ack = response[2];
        if ack != TRANSFER_OK {
            if ack & 7 == 4 {
                // Keep the failed operation visible, but release the DP's sticky
                // FAULT so a bad memory address cannot poison halt/resume.
                let _ = self.clear_sticky_errors();
            }
            return Err(Error::SwdAck { ack });
        }
        if completed != transfers.len() {
            return Err(Error::UnexpectedResponse {
                command: COMMAND_TRANSFER,
                detail: format!("completed {completed} of {} transfers", transfers.len()),
            });
        }
        let reads = transfers
            .iter()
            .filter(|transfer| matches!(transfer, TransferRequest::Read { .. }))
            .count();
        if response.len() != 3 + reads * 4 {
            return Err(Error::UnexpectedResponse {
                command: COMMAND_TRANSFER,
                detail: format!("{} bytes for {reads} reads", response.len()),
            });
        }
        let mut values = Vec::with_capacity(reads);
        for index in 0..reads {
            let start = 3 + index * 4;
            values.push(u32::from_le_bytes(
                response[start..start + 4].try_into().unwrap(),
            ));
        }
        Ok(values)
    }

    pub fn read_dp(&mut self, address: u8) -> Result<u32> {
        Ok(self.transfer(&[TransferRequest::dp_read(address)])?[0])
    }

    pub fn write_dp(&mut self, address: u8, value: u32) -> Result<()> {
        self.transfer(&[TransferRequest::dp_write(address, value)])
            .map(|_| ())
    }

    pub fn read_ap(&mut self, address: u8) -> Result<u32> {
        self.read_ap_address(address as u16)
    }

    pub fn write_ap(&mut self, address: u8, value: u32) -> Result<()> {
        self.write_ap_address(address as u16, value)
    }

    pub fn read_dp_idcode(&mut self) -> Result<u32> {
        self.read_dp(DP_IDCODE)
    }

    pub fn power_up_debug_port(&mut self) -> Result<u32> {
        self.clear_sticky_errors()?;
        // Another debugger can leave DPBANKSEL pointing at DLPIDR. CTRL/STAT
        // must be accessed in bank 0 before checking power acknowledgements.
        self.write_dp(DP_SELECT, 0)?;
        self.write_dp(DP_CTRL_STAT, CTRL_STAT_POWERUP_REQ)?;
        let mut last_status = 0;
        for _ in 0..100 {
            let status = self.read_dp(DP_CTRL_STAT)?;
            last_status = status;
            if status & CTRL_STAT_POWERUP_ACK == CTRL_STAT_POWERUP_ACK {
                return Ok(status);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Err(Error::UnexpectedResponse {
            command: COMMAND_TRANSFER,
            detail: format!(
                "target debug power-up acknowledgement timed out (last CTRL/STAT 0x{last_status:08x})"
            ),
        })
    }

    pub fn clear_sticky_errors(&mut self) -> Result<()> {
        // DAP_WriteABORT: DP index followed by STKCMP/ERR/WDERR/ORUN clear bits.
        self.expect_status(
            &[COMMAND_WRITE_ABORT, 0, 0x1e, 0, 0, 0],
            COMMAND_WRITE_ABORT,
        )
    }

    pub fn read_memory32(&mut self, address: u32, words: usize) -> Result<Vec<u32>> {
        if address & 3 != 0 || words == 0 {
            return Err(Error::InvalidArgument(
                "memory address must be aligned and words non-zero".into(),
            ));
        }
        self.write_ap_address(AP_CSW as u16, 0x2300_0052)?;
        let mut values = Vec::with_capacity(words);
        for index in 0..words {
            let current = address + (index as u32) * 4;
            self.write_ap_address(AP_TAR as u16, current)?;
            values.push(self.read_ap_address(AP_DRW as u16)?);
        }
        Ok(values)
    }

    pub fn write_memory32(&mut self, address: u32, value: u32) -> Result<()> {
        if address & 3 != 0 {
            return Err(Error::InvalidArgument(
                "memory address must be aligned".into(),
            ));
        }
        self.write_ap_address(AP_CSW as u16, 0x2300_0052)?;
        self.write_ap_address(AP_TAR as u16, address)?;
        self.write_ap_address(AP_DRW as u16, value)
    }

    fn read_ap_address(&mut self, address: u16) -> Result<u32> {
        self.select_ap_bank(address)?;
        Ok(self.transfer(&[TransferRequest::ap_read((address as u8) & 0x0c)])?[0])
    }

    fn write_ap_address(&mut self, address: u16, value: u32) -> Result<()> {
        self.select_ap_bank(address)?;
        self.transfer(&[TransferRequest::ap_write((address as u8) & 0x0c, value)])
            .map(|_| ())
    }

    fn select_ap_bank(&mut self, address: u16) -> Result<()> {
        let banked_address = if self.adiv6_ap.is_some() {
            ADIV6_MEM_AP_BANK + address
        } else {
            address
        };
        let select = self
            .adiv6_ap
            .map(|base| base | (banked_address as u32 & 0x0ff0))
            .unwrap_or((banked_address as u32) & 0x00f0);
        self.write_dp(DP_SELECT, select)
    }

    fn expect_status(&mut self, request: &[u8], command: u8) -> Result<()> {
        let response = self.command(request)?;
        if response.len() < 2 || response[0] != command {
            return Err(Error::UnexpectedResponse {
                command,
                detail: "invalid DAP response".into(),
            });
        }
        if response[1] != 0 {
            return Err(Error::UnexpectedResponse {
                command,
                detail: format!("status 0x{:02x}", response[1]),
            });
        }
        Ok(())
    }

    fn expect_swj_pins(&mut self, request: &[u8]) -> Result<()> {
        let response = self.command(request)?;
        if response.len() < 2 || response[0] != COMMAND_SWJ_PINS {
            return Err(Error::UnexpectedResponse {
                command: COMMAND_SWJ_PINS,
                detail: "invalid DAP_SWJ_Pins response".into(),
            });
        }
        Ok(())
    }

    fn command(&mut self, request: &[u8]) -> Result<Vec<u8>> {
        if request.len() > self.packet_size {
            return Err(Error::InvalidArgument(
                "CMSIS-DAP command exceeds packet size".into(),
            ));
        }
        let mut packet = vec![0u8; self.packet_size];
        packet[..request.len()].copy_from_slice(request);
        self.handle
            .write_bulk(self.endpoint_out, &packet, self.timeout)?;
        let expected = request.first().copied().unwrap_or(0);
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout);
            }
            let mut response = vec![0u8; self.packet_size];
            let count = self
                .handle
                .read_bulk(self.endpoint_in, &mut response, remaining)?;
            response.truncate(count);
            if response.first().copied() == Some(expected) {
                return Ok(response);
            }
            // A device that was previously driven with a different logical
            // packet size can leave one or more short replies queued. Discard
            // those replies while waiting for the command just submitted.
        }
    }
}

impl Drop for CmsisDap {
    fn drop(&mut self) {
        let _ = self.disconnect();
        let _ = self.handle.release_interface(self.interface);
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    #[ignore = "requires an explicitly selected RP2040 probe; reads an unmapped address"]
    fn connected_invalid_memory_does_not_poison_following_access() {
        let serial = std::env::var("PDBG_PROBE_SERIAL").expect("set PDBG_PROBE_SERIAL");
        let mut dap = CmsisDap::open_serial(serial).unwrap();
        dap.connect_swd_core(1_000_000, 0).unwrap();
        dap.power_up_debug_port().unwrap();
        dap.read_memory32(0x20000000, 1).unwrap();
        assert!(dap.read_memory32(0xfffffff0, 1).is_err());
        dap.read_memory32(0xe000edf0, 1)
            .expect("a failed memory view must not prevent reading DHCSR or resuming");
    }
}
