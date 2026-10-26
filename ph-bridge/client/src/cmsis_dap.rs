//! A connection-scoped, exclusive SWD lease held by phd.
use crate::{DeviceSelector, Result, RpcClient};
pub use ph_protocol::dap::TransferRequest;
use serde_json::{json, Value};
pub struct CmsisDap {
    rpc: RpcClient,
    packet_size: usize,
}
impl CmsisDap {
    pub fn open() -> Result<Self> {
        Self::open_with_selector(DeviceSelector::any())
    }
    pub fn open_serial(serial: impl Into<String>) -> Result<Self> {
        Self::open_with_selector(DeviceSelector::serial(serial))
    }
    pub fn open_with_selector(selector: DeviceSelector) -> Result<Self> {
        let mut rpc = RpcClient::from_env()?;
        rpc.call::<Value>("device.open", json!({"serial":selector.serial}))?;
        let packet_size = rpc.call("swd.open", json!({}))?;
        Ok(Self { rpc, packet_size })
    }
    pub fn packet_size(&self) -> usize {
        self.packet_size
    }
    pub fn info(&mut self, id: u8) -> Result<Vec<u8>> {
        self.rpc.call("swd.info", json!({"id":id}))
    }
    pub fn connect_swd(&mut self, clock_hz: u32) -> Result<()> {
        self.rpc
            .call::<()>("swd.connect_swd", json!({"clock_hz":clock_hz}))?;
        self.refresh_packet_size()
    }
    pub fn connect_swd_adiv6(&mut self, clock_hz: u32, ap_base: u32) -> Result<()> {
        self.rpc.call::<()>(
            "swd.connect_swd_adiv6",
            json!({"clock_hz":clock_hz, "ap_base":ap_base}),
        )?;
        self.refresh_packet_size()
    }
    pub fn connect_swd_core(&mut self, clock_hz: u32, core: u8) -> Result<()> {
        self.rpc.call::<()>(
            "swd.connect_swd_core",
            json!({"clock_hz":clock_hz, "core":core}),
        )?;
        self.refresh_packet_size()
    }
    fn refresh_packet_size(&mut self) -> Result<()> {
        self.packet_size = self.rpc.call("swd.packet_size", json!({}))?;
        Ok(())
    }
    pub fn disconnect(&mut self) -> Result<()> {
        self.rpc.call("swd.disconnect", json!({}))
    }
    pub fn set_clock(&mut self, hz: u32) -> Result<()> {
        self.rpc.call("swd.set_clock", json!({"hz":hz}))
    }
    pub fn swj_sequence(&mut self, clocks: u8, data: &[u8]) -> Result<()> {
        self.rpc
            .call("swd.swj_sequence", json!({"clocks":clocks, "data":data}))
    }
    pub fn reset_target(&mut self, milliseconds: u32) -> Result<()> {
        self.rpc
            .call("swd.reset_target", json!({"milliseconds":milliseconds}))
    }
    pub fn set_reset(&mut self, asserted: bool) -> Result<()> {
        self.rpc.call("swd.set_reset", json!({"asserted":asserted}))
    }
    pub fn transfer(&mut self, transfers: &[TransferRequest]) -> Result<Vec<u32>> {
        self.rpc
            .call("swd.transfer", json!({"transfers":transfers}))
    }
    pub fn read_dp(&mut self, address: u8) -> Result<u32> {
        self.rpc.call("swd.read_dp", json!({"address":address}))
    }
    pub fn write_dp(&mut self, address: u8, value: u32) -> Result<()> {
        self.rpc
            .call("swd.write_dp", json!({"address":address, "value":value}))
    }
    pub fn read_ap(&mut self, address: u8) -> Result<u32> {
        self.rpc.call("swd.read_ap", json!({"address":address}))
    }
    pub fn write_ap(&mut self, address: u8, value: u32) -> Result<()> {
        self.rpc
            .call("swd.write_ap", json!({"address":address, "value":value}))
    }
    pub fn read_dp_idcode(&mut self) -> Result<u32> {
        self.rpc.call("swd.read_dp_idcode", json!({}))
    }
    pub fn power_up_debug_port(&mut self) -> Result<u32> {
        self.rpc.call("swd.power_up_debug_port", json!({}))
    }
    pub fn clear_sticky_errors(&mut self) -> Result<()> {
        self.rpc.call("swd.clear_sticky_errors", json!({}))
    }
    pub fn read_memory32(&mut self, address: u32, words: usize) -> Result<Vec<u32>> {
        self.rpc.call(
            "swd.read_memory32",
            json!({"address":address, "words":words}),
        )
    }
    pub fn write_memory32(&mut self, address: u32, value: u32) -> Result<()> {
        self.rpc.call(
            "swd.write_memory32",
            json!({"address":address, "value":value}),
        )
    }
}
