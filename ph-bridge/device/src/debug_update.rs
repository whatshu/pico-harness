//! Debug Core updates pinned to the selected physical USB port.
use crate::firmware::{Image, Target};
use crate::{usb, PhBoardLink};
use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};
pub fn update(
    serial: &str,
    file: &std::path::Path,
    picotool: &str,
    cancel: &std::sync::atomic::AtomicBool,
    mut phase: impl FnMut(String),
) -> Result<String, String> {
    if !file
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("uf2"))
    {
        return Err("Select a Debug Core UF2 image".into());
    }
    let mut link = PhBoardLink::open_serial(serial).map_err(|e| e.to_string())?;
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
    // New PH firmware advertises `ph-terminal` (DebugBootsel lives in the PH
    // protocol); accept the legacy PDBG `debug-bootsel` tag too.
    if !info
        .split(';')
        .any(|field| field == "ph-terminal" || field == "debug-bootsel")
    {
        return Err("This Debug Core firmware does not support software updates".into());
    }
    // Check all blocks, the image family, the programmer, and USB identity before rebooting.
    let image = Image::read(file, target)?;
    if !image.segments.iter().any(|segment| {
        segment
            .bytes
            .windows(b"ph-terminal".len())
            .any(|bytes| bytes == b"ph-terminal")
    }) {
        return Err(
            "The selected UF2 does not identify itself as a PicoHarness Debug Core image".into(),
        );
    }
    crate::debug::run_tool(Command::new(picotool).arg("version"), cancel)?;
    let location = link.usb_location().map_err(|e| e.to_string())?;
    phase("Entering Debug Core bootloader".into());
    link.enter_debug_bootsel().map_err(|e| e.to_string())?;
    drop(link);
    let started = Instant::now();
    let (bus, address) = loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Debug update cancelled".into());
        }
        if let Some(device) = usb::bootloader_at(&location, pid).map_err(|e| e.to_string())? {
            break device;
        }
        if started.elapsed() > Duration::from_secs(10) {
            return Err("Debug Core bootloader did not appear on the selected USB port".into());
        }
        thread::sleep(Duration::from_millis(50));
    };
    phase("Writing and verifying Debug Core".into());
    crate::debug::run_tool(
        Command::new(picotool)
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
            .arg(file),
        cancel,
    )?;
    phase("Restarting Debug Core".into());
    crate::debug::run_tool(
        Command::new(picotool).args([
            "reboot",
            "--bus",
            &bus.to_string(),
            "--address",
            &address.to_string(),
        ]),
        cancel,
    )?;
    let started = Instant::now();
    loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Debug update cancelled".into());
        }
        if let Ok(mut updated) = PhBoardLink::open_serial(serial) {
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
