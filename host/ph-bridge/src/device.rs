//! Device discovery: map the Debug core's CDC serial ports on Linux/macOS.
//!
//! Linux walks `/sys/class/tty/ttyACM*`, matches the parent USB device
//! `0x2e8a:0x000c`, and classifies each tty by its interface number:
//! CDC data interface 2 = PH Terminal, interface 5 = Logic Analyzer.

use std::fs;
use std::path::{Path, PathBuf};

pub const USB_VENDOR_ID: u16 = 0x2e8a;
pub const USB_PRODUCT_ID: u16 = 0x000c;

#[derive(Debug, Default, Clone)]
pub struct PhDeviceInfo {
    pub terminal_tty: Option<PathBuf>,
    pub la_tty: Option<PathBuf>,
    pub serial: Option<String>,
}

/// Read a small attribute file; `None` when absent/unreadable.
fn attr(dir: &Path, name: &str) -> Option<String> {
    fs::read_to_string(dir.join(name))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Resolve the USB device directory from a tty's `device` symlink by walking
/// up until `bDeviceClass`/`idVendor` are siblings (i.e. the usb_device node).
fn usb_device_dir(dev_symlink: &Path) -> Option<PathBuf> {
    let mut p = fs::canonicalize(dev_symlink).ok()?;
    loop {
        match (attr(&p, "idVendor"), attr(&p, "idProduct")) {
            (Some(v), Some(pr)) => {
                let vid = u16::from_str_radix(&v, 16).ok()?;
                let pid = u16::from_str_radix(&pr, 16).ok()?;
                if vid == USB_VENDOR_ID && pid == USB_PRODUCT_ID {
                    return Some(p);
                }
                return None;
            }
            _ => {}
        }
        match p.parent() {
            Some(parent) => p = parent.to_path_buf(),
            None => return None,
        }
    }
}

fn scan_linux() -> Vec<PhDeviceInfo> {
    let mut rows: Vec<(Option<String>, u32, PathBuf)> = Vec::new();
    let sys = Path::new("/sys/class/tty");
    let entries = match fs::read_dir(sys) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("ttyACM") {
            continue;
        }
        let devlink = entry.path().join("device");
        let Some(usbdev) = usb_device_dir(&devlink) else {
            continue;
        };
        let Some(ifnum) = attr(&devlink, "bInterfaceNumber") else {
            continue;
        };
        let ifnum = ifnum.parse::<u32>().unwrap_or(99);
        let tty = PathBuf::from("/dev").join(&name);
        rows.push((attr(&usbdev, "serial"), ifnum, tty));
    }

    let mut out: Vec<PhDeviceInfo> = Vec::new();
    for (serial, ifnum, tty) in rows {
        let len = out.len();
        let phdev = match out.iter_mut().find(|d| d.serial == serial) {
            Some(d) => d,
            None => {
                out.push(PhDeviceInfo {
                    serial,
                    ..Default::default()
                });
                &mut out[len]
            }
        };
        match ifnum {
            2 => phdev.terminal_tty = Some(tty),
            5 => phdev.la_tty = Some(tty),
            _ => {}
        }
    }
    out
}

/// Enumerate connected PicoHarness probes and their control / logic ttyACM
/// ports (Linux only for now; see doc/ARCHITECTURE.md).
pub fn scan() -> Vec<PhDeviceInfo> {
    if cfg!(target_os = "linux") {
        scan_linux()
    } else {
        // macOS/Windows: not implemented (see doc/ARCHITECTURE.md).
        Vec::new()
    }
}