//! USB metadata is queried on the daemon host, including remote connections.
use crate::{Result, RpcClient};
pub use ph_protocol::{DeviceInfo, DeviceSelector, Location};
use serde_json::json;
pub const USB_VENDOR_ID: u16 = 0x2e8a;
pub const USB_PRODUCT_ID: u16 = 0x000c;
pub fn enumerate_devices() -> Result<Vec<DeviceInfo>> {
    RpcClient::from_env()?.call("device.list", json!({}))
}
pub fn bootloader_at(location: &Location, product_id: u16) -> Result<Option<(u8, u8)>> {
    RpcClient::from_env()?.call(
        "device.bootloader",
        json!({"location":location,"product_id":product_id}),
    )
}
