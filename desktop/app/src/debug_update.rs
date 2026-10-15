//! Debug Core updates through its ROM bootloader, pinned to the physical USB port.
use crate::firmware::{Image, Target};
use device_control::{usb, PdbgLink};
use std::{
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

pub enum Event {
    Phase(String),
    Done(Result<String, String>),
}
pub struct Job {
    pub rx: Receiver<Event>,
}
impl Job {
    pub fn start(serial: String, file: PathBuf, picotool: String) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = update(&serial, &file, &picotool, |text| {
                let _ = tx.send(Event::Phase(text));
            });
            let _ = tx.send(Event::Done(result));
        });
        Self { rx }
    }
}
fn update(
    serial: &str,
    file: &std::path::Path,
    picotool: &str,
    mut phase: impl FnMut(String),
) -> Result<String, String> {
    if !file
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("uf2"))
    {
        return Err("Select a Debug Core UF2 image".into());
    }
    let mut link = PdbgLink::open_serial(serial).map_err(|e| e.to_string())?;
    let info = link.info().map_err(|e| e.to_string())?;
    let powered = link.status().map_err(|e| e.to_string())?.power_on;
    let (target, pid, family) = match info
        .split(';')
        .find(|part| *part == "pico" || *part == "pico2")
    {
        Some("pico") => (Target::Rp2040, 0x0003, "0xe48bff56"),
        Some("pico2") => (Target::Rp2350, 0x000f, "0xe48bff59"),
        _ => return Err("Debug Core did not report its chip family".into()),
    };
    if !info.split(';').any(|field| field == "debug-bootsel") {
        return Err("This Debug Core firmware does not support software updates".into());
    }
    // Check all blocks, the image family, the programmer, and USB identity before rebooting.
    let image = Image::read(file, target)?;
    if !image.segments.iter().any(|segment| {
        segment
            .bytes
            .windows(b"pdbg-link;dedicated-core1".len())
            .any(|bytes| bytes == b"pdbg-link;dedicated-core1")
    }) {
        return Err(
            "The selected UF2 does not identify itself as a PicoHarness Debug Core image".into(),
        );
    }
    run(Command::new(picotool).arg("version"))?;
    let location = link.usb_location().map_err(|e| e.to_string())?;
    phase("Entering Debug Core bootloader".into());
    link.enter_debug_bootsel().map_err(|e| e.to_string())?;
    drop(link);
    let started = Instant::now();
    let (bus, address) = loop {
        if let Some(device) = usb::bootloader_at(&location, pid).map_err(|e| e.to_string())? {
            break device;
        }
        if started.elapsed() > Duration::from_secs(10) {
            return Err("Debug Core bootloader did not appear on the selected USB port".into());
        }
        thread::sleep(Duration::from_millis(50));
    };
    phase("Writing and verifying Debug Core".into());
    run(Command::new(picotool)
        .args([
            "load",
            "-v",
            "--bus",
            &bus.to_string(),
            "--address",
            &address.to_string(),
            "--family",
            family,
        ])
        .arg(file))?;
    phase("Restarting Debug Core".into());
    run(Command::new(picotool).args([
        "reboot",
        "--bus",
        &bus.to_string(),
        "--address",
        &address.to_string(),
    ]))?;
    let started = Instant::now();
    loop {
        if let Ok(mut updated) = PdbgLink::open_serial(serial) {
            if let Ok(info) = updated.info() {
                updated.set_power(powered).map_err(|e| e.to_string())?;
                return Ok(info);
            }
        }
        if started.elapsed() > Duration::from_secs(20) {
            return Err(
                "Updated Debug Core did not reconnect with the original serial number".into(),
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}
fn run(command: &mut Command) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|e| format!("Cannot run picotool: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "picotool failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Updates the connected Debug Core: set PDBG_PROBE_SERIAL and PDBG_DEBUG_IMAGE"]
    fn upgrade_connected_debug_core() {
        let serial = std::env::var("PDBG_PROBE_SERIAL").unwrap();
        let image = PathBuf::from(std::env::var("PDBG_DEBUG_IMAGE").unwrap());
        let info = update(&serial, &image, "picotool", |phase| eprintln!("{phase}")).unwrap();
        assert!(info.contains("capture-async"));
        eprintln!("Debug Core reconnected: {info}");
    }
}
