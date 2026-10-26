//! Daemon-side Work programming, retaining image validation and verification.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

pub use ph_image::{Image, Target};
#[derive(Clone)]
pub struct Config {
    pub file: PathBuf,
    pub serial: String,
    pub target: Target,
    pub openocd: String,
    pub scripts: String,
    pub reset_after: bool,
}
#[derive(Debug)]
pub enum Event {
    Phase(&'static str),
    Log(String),
    Image { format: &'static str, bytes: usize },
    Done(Result<(), String>),
}
pub struct Job {
    pub rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Job {
    pub fn start(config: Config) -> Self {
        let (events, rx) = mpsc::sync_channel(256);
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = cancel.clone();
        let worker = thread::spawn(move || {
            let result = program(&config, &events, &stopped);
            let _ = events.send(Event::Done(result));
        });
        Self {
            rx,
            cancel,
            worker: Some(worker),
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Programming cancelled; any completed flash writes remain. Program again before running the target.".into())
    } else {
        Ok(())
    }
}
static TEMP_NUMBER: AtomicU64 = AtomicU64::new(0);
struct ImageFile(PathBuf);
impl ImageFile {
    fn new(image: &Image) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "ph-fw-program-{}-{}.hex",
            std::process::id(),
            TEMP_NUMBER.fetch_add(1, Ordering::Relaxed)
        ));
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let guard = Self(path);
        file.write_all(image.intel_hex().as_bytes())
            .map_err(|e| e.to_string())?;
        Ok(guard)
    }
}
impl Drop for ImageFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
struct Power {
    link: crate::PhBoardLink,
    restore_off: bool,
}
impl Drop for Power {
    fn drop(&mut self) {
        if self.restore_off {
            let _ = self.link.set_power(false);
        }
    }
}
fn program(config: &Config, events: &SyncSender<Event>, cancel: &AtomicBool) -> Result<(), String> {
    let _ = events.send(Event::Phase("Validating"));
    let image = Image::read(&config.file, config.target)?;
    cancelled(cancel)?;
    let _ = events.send(Event::Image {
        format: image.format,
        bytes: image.byte_count(),
    });
    if config.serial.is_empty()
        || !config
            .serial
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
    {
        return Err("Select a USB probe with a valid serial number to flash its Work Core".into());
    }
    let image_file = ImageFile::new(&image)?;
    // Check the configured binary before changing Work power.
    crate::debug::run_tool(Command::new(&config.openocd).arg("--version"), cancel)?;
    let _ = events.send(Event::Phase("Preparing"));
    let mut link = crate::PhBoardLink::open_serial(&config.serial).map_err(|e| e.to_string())?;
    let was_on = link.status().map_err(|e| e.to_string())?.power_on;
    let mut power = Power {
        link,
        restore_off: !was_on,
    };
    if !was_on {
        power.link.set_power(true).map_err(|e| e.to_string())?;
        let _ = events.send(Event::Log("Work power enabled by Debug Core".into()));
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        cancelled(cancel)?;
        let status = power.link.status().map_err(|e| e.to_string())?;
        if status.power_fault {
            return Err("Work power fault; programming stopped".into());
        }
        if status.power_good {
            break;
        }
        if Instant::now() > deadline {
            return Err("Work 3V3 did not become ready".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
    let result = run_openocd(config, &image_file.0, events, cancel);
    if result.is_ok() {
        power.restore_off = false;
    }
    result
}
fn tcl_word(value: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' | '"' | '$' | '[' | ']' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}
fn script(path: &Path, reset_after: bool) -> String {
    let path = tcl_word(&path.to_string_lossy());
    // Separate commands and explicit sentinels make verify failures unambiguous.
    format!("init; reset init; echo PDBG_FLASH_WRITING; flash write_image erase {path} 0 ihex; echo PDBG_FLASH_VERIFYING; verify_image {path} 0 ihex; {}echo PDBG_FLASH_COMPLETE; shutdown",if reset_after {"reset run; "}else{""})
}
struct Process(std::process::Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn run_openocd(
    config: &Config,
    image: &Path,
    events: &SyncSender<Event>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    cancelled(cancel)?;
    let mut command = Command::new(&config.openocd);
    if !config.scripts.is_empty() {
        command.args(["-s", &config.scripts]);
    }
    command.args(["-f","interface/cmsis-dap.cfg","-c",&format!("adapter serial {}; adapter speed 1000; gdb_port disabled; tcl_port disabled; telnet_port disabled",config.serial),"-f",&format!("target/{}.cfg",config.target.name()),"-c",&script(image,config.reset_after)]);
    let mut process = Process(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("OpenOCD: {e}"))?,
    );
    let (tx, rx) = mpsc::sync_channel(128);
    crate::debug::reader(process.0.stdout.take().unwrap(), tx.clone());
    crate::debug::reader(process.0.stderr.take().unwrap(), tx);
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut complete = false;
    let mut diagnostic_count = 0;
    let mut line = |line: String, complete: &mut bool| match line.trim() {
        "PDBG_FLASH_WRITING" => {
            let _ = events.send(Event::Phase("Writing"));
        }
        "PDBG_FLASH_VERIFYING" => {
            let _ = events.send(Event::Phase("Verifying"));
        }
        "PDBG_FLASH_COMPLETE" => *complete = true,
        _ => {
            // Reserve event capacity for phases and completion, even if the
            // receiver stops polling. Drop must always be able to join safely.
            if diagnostic_count < 128 {
                let _ = events.send(Event::Log(line));
                diagnostic_count += 1;
            }
        }
    };
    loop {
        cancelled(cancel)?;
        if let Ok(text) = rx.recv_timeout(Duration::from_millis(50)) {
            line(text, &mut complete);
        }
        if let Some(status) = process.0.try_wait().map_err(|e| e.to_string())? {
            // Readers finish once both process streams close. Drain trailing verify output.
            while let Ok(text) = rx.recv_timeout(Duration::from_millis(100)) {
                line(text, &mut complete);
            }
            return if status.success() && complete {
                Ok(())
            } else {
                Err(format!(
                    "OpenOCD programming or verification failed ({status}); see the log"
                ))
            };
        }
        if Instant::now() > deadline {
            return Err("OpenOCD programming timed out".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tcl_file_names_are_data_and_reset_is_explicit() {
        let command = script(Path::new("/tmp/firmware [x];$y\".hex"), false);
        assert!(command.contains("\\[x\\]"));
        assert!(command.contains("\\$y\\\""));
        assert!(!command.contains("reset run"));
        assert!(command.contains("verify_image"));
        assert!(script(Path::new("x"), true).contains("reset run"));
    }
    #[cfg(unix)]
    #[test]
    fn process_failure_and_cancel_never_report_success() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("ph-fw-openocd-test-{}", std::process::id()));
        fs::write(
            &path,
            "#!/bin/sh\necho PDBG_FLASH_WRITING\necho PDBG_FLASH_COMPLETE\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let config = Config {
            file: "unused".into(),
            serial: "test".into(),
            target: Target::Rp2040,
            openocd: path.to_string_lossy().into(),
            scripts: String::new(),
            reset_after: true,
        };
        let (tx, _rx) = mpsc::sync_channel(256);
        let cancel = AtomicBool::new(false);
        assert!(run_openocd(&config, Path::new("unused.hex"), &tx, &cancel).is_err());
        fs::write(
            &path,
            "#!/bin/sh\necho PDBG_FLASH_VERIFYING\necho PDBG_FLASH_COMPLETE\nexit 0\n",
        )
        .unwrap();
        assert!(run_openocd(&config, Path::new("unused.hex"), &tx, &cancel).is_ok());
        cancel.store(true, Ordering::Relaxed);
        assert!(run_openocd(&config, Path::new("unused.hex"), &tx, &cancel).is_err());
        fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod hardware_tests {
    use super::*;
    /// Explicitly opted-in integration test; never selected by ordinary cargo test.
    #[test]
    #[ignore = "Writes Work flash: set PDBG_FLASH_TEST_IMAGE, PDBG_PROBE_SERIAL and PDBG_FLASH_TARGET"]
    fn program_connected_work_core() {
        let config = Config {
            file: std::env::var_os("PDBG_FLASH_TEST_IMAGE")
                .expect("PDBG_FLASH_TEST_IMAGE")
                .into(),
            serial: std::env::var("PDBG_PROBE_SERIAL").expect("PDBG_PROBE_SERIAL"),
            target: Target::parse(&std::env::var("PDBG_FLASH_TARGET").expect("PDBG_FLASH_TARGET"))
                .unwrap(),
            openocd: std::env::var("PDBG_OPENOCD").unwrap_or("openocd".into()),
            scripts: std::env::var("PDBG_OPENOCD_SCRIPTS").unwrap_or_default(),
            reset_after: true,
        };
        let job = Job::start(config);
        let deadline = Instant::now() + Duration::from_secs(200);
        loop {
            let event = job
                .rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("flash worker timed out");
            println!("{event:?}");
            if let Event::Done(result) = event {
                result.unwrap();
                break;
            }
        }
    }
}
