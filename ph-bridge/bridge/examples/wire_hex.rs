//! Print wire bytes for known messages so nanopb (firmware) and prost (host)
//! can be diffed. Keep in sync with the C probes (none committed).
use ph_bridge::framing::encode;
use ph_bridge::msg::ph_message;
use ph_bridge::msg::*;

fn hex(v: &[u8]) -> String {
    v.iter().map(|b| format!("{:02x}", b)).collect()
}

fn main() {
    let power = PhMessage {
        seq: 7,
        body: Some(ph_message::Body::PowerSet(PowerSet { enabled: true })),
        ..Default::default()
    };
    println!("power_set_true: {}", hex(&encode(&power)));

    let getstatus = PhMessage {
        seq: 3,
        body: Some(ph_message::Body::GetStatus(GetStatus::default())),
        ..Default::default()
    };
    println!("get_status_req: {}", hex(&encode(&getstatus)));

    let mut tx = ConsoleTx::default();
    tx.data.extend_from_slice(b"hi");
    let console = PhMessage {
        seq: 9,
        body: Some(ph_message::Body::ConsoleTx(tx)),
        ..Default::default()
    };
    println!("console_tx_hi: {}", hex(&encode(&console)));
}
