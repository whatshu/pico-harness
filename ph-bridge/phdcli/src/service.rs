//! Explicit installation commands. Normal CLI/device use never installs a
//! login item, changes system services or requires administrator privileges.
use super::{Args, Result, Service};
use serde_json::{json, Value};
use std::{path::Path, process::Command};
fn command(command: &mut Command) -> Result<()> {
    let status = command.status()?;
    if !status.success() {
        return Err(format!("service manager failed ({status})").into());
    }
    Ok(())
}
pub(super) fn run(args: &Args, action: &Service, phd: &Path) -> Result<Value> {
    args.address.parse::<std::net::SocketAddr>()?;
    let phd = std::fs::canonicalize(phd)?;
    let token = args
        .token_file
        .clone()
        .unwrap_or(ph_client::rpc::token_path()?);
    if matches!(action, Service::Install) {
        command(
            Command::new(&phd)
                .arg("--init-token")
                .arg("--token-file")
                .arg(&token),
        )?;
    }
    let token = std::path::absolute(token)?;
    #[cfg(target_os = "linux")]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|p| std::path::PathBuf::from(p).join(".config"))
            })
            .ok_or("cannot locate user configuration directory")?;
        let path = config.join("systemd/user/phd.service");
        if matches!(action, Service::Install) {
            std::fs::create_dir_all(path.parent().ok_or("invalid service path")?)?;
            let quote = |value: &str| -> Result<String> {
                if value.contains(['\n', '\r']) {
                    return Err("newline in service argument".into());
                }
                Ok(format!(
                    "\"{}\"",
                    value
                        .replace('\\', "\\\\")
                        .replace('"', "\\\"")
                        .replace('%', "%%")
                        .replace('$', "$$")
                ))
            };
            std::fs::write(&path,format!("[Unit]\nDescription=Pico Harness daemon\n\n[Service]\nExecStart={} --listen {} --token-file {}\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n",quote(&phd.to_string_lossy())?,quote(&args.address)?,quote(&token.to_string_lossy())?))?;
            command(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
            command(Command::new("systemctl").args(["--user", "enable", "--now", "phd.service"]))?;
        } else if matches!(action, Service::Uninstall) {
            command(Command::new("systemctl").args(["--user", "disable", "--now", "phd.service"]))?;
            std::fs::remove_file(path)?;
            command(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
        } else {
            let verb = match action {
                Service::Start => "start",
                Service::Stop => "stop",
                _ => "status",
            };
            command(Command::new("systemctl").args(["--user", verb, "phd.service"]))?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").ok_or("HOME is unset")?;
        let path =
            std::path::PathBuf::from(home).join("Library/LaunchAgents/org.picoharness.phd.plist");
        let uid = Command::new("id").arg("-u").output()?;
        let domain = format!("gui/{}", String::from_utf8(uid.stdout)?.trim());
        let name = format!("{domain}/org.picoharness.phd");
        if matches!(action, Service::Install) {
            std::fs::create_dir_all(path.parent().ok_or("invalid LaunchAgent path")?)?;
            let xml = |value: &str| {
                value
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('"', "&quot;")
            };
            std::fs::write(&path,format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>org.picoharness.phd</string><key>ProgramArguments</key><array><string>{}</string><string>--listen</string><string>{}</string><string>--token-file</string><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/></dict></plist>",xml(&phd.to_string_lossy()),xml(&args.address),xml(&token.to_string_lossy())))?;
            command(
                Command::new("launchctl")
                    .arg("bootstrap")
                    .arg(&domain)
                    .arg(&path),
            )?;
        } else if matches!(action, Service::Uninstall) {
            // A stopped LaunchAgent has already been booted out. Its plist
            // must still be removable through the normal uninstall command.
            let _ = Command::new("launchctl").arg("bootout").arg(&name).output();
            std::fs::remove_file(path)?;
        } else if matches!(action, Service::Start) {
            command(
                Command::new("launchctl")
                    .arg("bootstrap")
                    .arg(&domain)
                    .arg(&path),
            )?;
        } else {
            let verb = match action {
                Service::Start => unreachable!(),
                Service::Stop => "bootout",
                _ => "print",
            };
            command(Command::new("launchctl").arg(verb).arg(&name))?;
        }
    }
    #[cfg(windows)]
    {
        match action {
            Service::Install => {
                let binary = format!(
                    "\"{}\" --windows-service --listen {} --token-file \"{}\"",
                    phd.display(),
                    args.address,
                    token.display()
                );
                command(Command::new("sc.exe").args([
                    "create",
                    "PicoHarness",
                    "binPath=",
                    &binary,
                    "start=",
                    "auto",
                    "DisplayName=",
                    "Pico Harness Daemon",
                ]))?;
                command(Command::new("sc.exe").args(["start", "PicoHarness"]))?;
            }
            Service::Uninstall => {
                let _ = command(Command::new("sc.exe").args(["stop", "PicoHarness"]));
                command(Command::new("sc.exe").args(["delete", "PicoHarness"]))?;
            }
            _ => {
                let verb = match action {
                    Service::Start => "start",
                    Service::Stop => "stop",
                    _ => "query",
                };
                command(Command::new("sc.exe").args([verb, "PicoHarness"]))?;
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    return Err("native service installation is supported on Linux, macOS and Windows".into());
    Ok(json!({"ok":true}))
}
