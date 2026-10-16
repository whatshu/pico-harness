//! `ph` — PicoHarness device CLI (thin client of the ph_bridge crate).
//!
//! Note: the eventual `ph` command is a thin Python shell over the PyO3
//! binding (P4). This Rust bin is the tooling used during bring-up so the
//! crate is exercised end-to-end without the binding.

use std::process::exit;

use ph_bridge::device;
use ph_bridge::{PhClient, Status};

fn usage() -> ! {
    eprintln!(
        "usage: ph <command> [args]\n\
         \x20 device list\n\
         \x20 power on|off\n\
         \x20 measure\n\
         \x20 status\n\
         \x20 console send <text>\n\
         \x20 console read [n]\n\
         \x20 term <baud>\n\
         \x20 bootsel"
    );
    exit(2);
}

fn open(port: Option<&str>) -> PhClient<ph_bridge::client::SerialTransport> {
    let port = match port {
        Some(p) => p.to_string(),
        None => match device::scan().into_iter().find_map(|d| d.terminal_tty) {
            Some(p) => p.to_string_lossy().into_owned(),
            None => {
                eprintln!("no PH Terminal found (use a tty path as an argument)");
                exit(1);
            }
        },
    };
    match PhClient::open_serial(&port) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("open {port}: {e}");
            exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
    }
    let mut cmd: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();
    let mut explicit_port: Option<&str> = None;
    const COMMANDS: &[&str] = &[
        "device", "power", "measure", "status", "console", "term", "bulk", "bootsel",
    ];
    if !COMMANDS.contains(&cmd[0]) {
        explicit_port = Some(cmd.remove(0));
    }
    let mut client = open(explicit_port);

    match cmd.as_slice() {
        [sub, rest @ ..] if *sub == "device" => {
            if rest == ["list"] || rest.is_empty() {
                for d in device::scan() {
                    println!(
                        "serial={:?} terminal={:?} la={:?}",
                        d.serial, d.terminal_tty, d.la_tty
                    );
                }
            } else {
                usage();
            }
        }
        ["power", which] => match *which {
            "on" => client.power_set(true).expect("power on"),
            "off" => client.power_set(false).expect("power off"),
            _ => usage(),
        },
        ["measure"] => {
            let m = client.measure().expect("measure");
            println!(
                "shunt={}uV vbus={}uV current={}uA power={}uW",
                m.shunt_uv, m.vbus_uv, m.current_ua, m.power_uw
            );
        }
        ["status"] => {
            let st = client.get_status().expect("status");
            println!(
                "power_enabled={} power_good={} ina_alert={} work_fault={} vbus_mv={}",
                st.power_enabled, st.power_good, st.ina_alert, st.work_fault, st.vbus_mv
            );
        }
        ["console", "send", text] => client.console_tx(text.as_bytes()).expect("console send"),
        ["console", "read", n] => {
            let n: usize = n.parse().expect("count");
            read_console_events(&mut client, n);
        }
        ["console", "read"] => read_console_events(&mut client, 64),
        ["term", b] => {
            let baud: u32 = b.parse().expect("baud");
            let actual = client.term_config(baud, 8, 1, 0).expect("term config");
            println!("actual={actual}");
        }
        ["bulk", file] => bulk_send(&mut client, file),
        ["bootsel"] => {
            client.debug_bootsel().expect("bootsel");
            println!("requested self-bootsel");
        }
        _ => usage(),
    }
}

fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f6_3b78u32 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// Send a file to the Work core via the bulk data plane (route WORK_SPI):
/// BulkTxBegin on the CDC, the raw payload on the vendor bulk interface, then
/// drain for the BulkTxDone event and report its CRC verdict.
fn bulk_send(
    client: &mut PhClient<ph_bridge::client::SerialTransport>,
    file: &str,
) {
    use std::time::Duration;
    use ph_bridge::BulkWriter;
    use ph_bridge::device;
    use ph_bridge::msg::ph_message;
    use ph_bridge::msg::Route;

    let data = std::fs::read(file).expect("read file");
    let crc = crc32c(&data);
    client
        .bulk_tx_begin(Route::WorkSpi, data.len() as u32, crc)
        .expect("bulk begin");

    let serial = device::scan().into_iter().next().and_then(|d| d.serial);
    let mut writer = BulkWriter::open(serial.as_deref()).expect("open PH Terminal Bulk");
    writer.write(&data).expect("write payload");

    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        for ev in client.recv_events(Duration::from_millis(200)) {
            if let Some(ph_message::Body::BulkTxDone(done)) = ev.body {
                println!(
                    "bulk done: ok={} crc_recv=0x{:08x} expected=0x{:08x}",
                    done.ok, done.crc_recv, crc
                );
                if !done.ok {
                    std::process::exit(1);
                }
                return;
            }
        }
    }
    eprintln!("no BulkTxDone event within 8s");
    std::process::exit(1);
}

/// Consume queued events; used to watch Work console output that arrives as
/// ConsoleData events. Reads the port for a short window without sending any
/// request (sending a probe would mutate device state).
fn read_console_events(client: &mut PhClient<ph_bridge::client::SerialTransport>, n: usize) {
    use std::time::Duration;
    use ph_bridge::msg::ph_message;
    let mut printed = 0usize;
    for _ in 0..n {
        let events = client.recv_events(Duration::from_millis(250));
        for ev in events {
            if let Some(ph_message::Body::ConsoleData(d)) = ev.body {
                let s = String::from_utf8_lossy(&d.data);
                print!("{s}");
                printed += 1;
            }
        }
        if printed > 0 {
            break;
        }
    }
    let _ = std::io::Write::flush(&mut std::io::stdout());
    if printed == 0 {
        eprintln!("(no console output)");
    }
}

#[allow(dead_code)]
fn _status_import() -> Status {
    Status::Ok
}