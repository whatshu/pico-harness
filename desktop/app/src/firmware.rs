//! Validated Work-core images and an asynchronous OpenOCD programmer.
//! UF2 is decoded to a sparse Intel HEX image; it is never written as raw UF2 bytes.
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const FLASH_BASE: u32 = 0x1000_0000;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const UF2_RP2040: u32 = 0xe48b_ff56;
const UF2_ABSOLUTE: u32 = 0xe48b_ff57;
const UF2_ARM_S: u32 = 0xe48b_ff59;
const UF2_ARM_NS: u32 = 0xe48b_ff5b;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Rp2040,
    Rp2350,
}
impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Self::Rp2040 => "rp2040",
            Self::Rp2350 => "rp2350",
        }
    }
    fn flash_end(self) -> u32 {
        FLASH_BASE
            + match self {
                Self::Rp2040 => 16 * 1024 * 1024,
                Self::Rp2350 => 32 * 1024 * 1024,
            }
    }
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "rp2040" => Ok(Self::Rp2040),
            "rp2350" => Ok(Self::Rp2350),
            _ => Err("Select RP2040 or RP2350 for the Work Core".into()),
        }
    }
}
#[derive(Debug, Clone)]
pub struct Segment {
    pub address: u32,
    pub bytes: Vec<u8>,
}
#[derive(Debug)]
pub struct Image {
    pub segments: Vec<Segment>,
    pub format: &'static str,
}
impl Image {
    pub fn byte_count(&self) -> usize {
        self.segments.iter().map(|s| s.bytes.len()).sum()
    }
    pub fn read(path: &Path, target: Target) -> Result<Self, String> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let file = fs::File::open(path).map_err(|e| format!("Cannot read firmware: {e}"))?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Select a firmware file, not a directory".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_FILE {
            return Err("Firmware file exceeds 64 MiB".into());
        }
        Self::parse(&extension, &bytes, target)
    }
    pub fn parse(extension: &str, data: &[u8], target: Target) -> Result<Self, String> {
        let mut image = match extension.to_ascii_lowercase().as_str() {
            "uf2" => parse_uf2(data, target)?,
            "elf" => parse_elf(data)?,
            "bin" => Self {
                segments: vec![Segment {
                    address: FLASH_BASE,
                    bytes: data.to_vec(),
                }],
                format: "BIN",
            },
            _ => return Err("Supported firmware files: .uf2, .elf, .bin".into()),
        };
        if image.segments.is_empty() {
            return Err("Image contains no flash data".into());
        }
        image.segments.sort_by_key(|s| s.address);
        let mut previous_end = 0;
        for segment in &image.segments {
            let end = segment
                .address
                .checked_add(u32::try_from(segment.bytes.len()).map_err(|_| "Image too large")?)
                .ok_or("Firmware address overflow")?;
            if segment.bytes.is_empty() || segment.address < FLASH_BASE || end > target.flash_end()
            {
                return Err(format!("Image range 0x{:08x}..0x{end:08x} is outside {} flash; RAM-only ELF images cannot be flashed",segment.address,target.name()));
            }
            if segment.address < previous_end {
                return Err("Firmware image contains overlapping segments".into());
            }
            previous_end = end;
        }
        Ok(image)
    }
    pub fn intel_hex(&self) -> String {
        fn record(output: &mut String, address: u16, kind: u8, payload: &[u8]) {
            let mut bytes = vec![
                payload.len() as u8,
                (address >> 8) as u8,
                address as u8,
                kind,
            ];
            bytes.extend_from_slice(payload);
            let sum = bytes.iter().fold(0u8, |n, b| n.wrapping_add(*b));
            bytes.push(0u8.wrapping_sub(sum));
            output.push(':');
            for b in bytes {
                output.push_str(&format!("{b:02X}"));
            }
            output.push('\n');
        }
        let mut output = String::new();
        let mut upper = None;
        for segment in &self.segments {
            let mut offset = 0;
            while offset < segment.bytes.len() {
                let address = segment.address + offset as u32;
                let high = (address >> 16) as u16;
                if upper != Some(high) {
                    record(&mut output, 0, 4, &high.to_be_bytes());
                    upper = Some(high);
                }
                let len = (segment.bytes.len() - offset)
                    .min(32)
                    .min(0x10000 - (address as usize & 0xffff));
                record(
                    &mut output,
                    address as u16,
                    0,
                    &segment.bytes[offset..offset + len],
                );
                offset += len;
            }
        }
        record(&mut output, 0, 1, &[]);
        output
    }
}
fn word(bytes: &[u8], offset: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or("Truncated firmware header")?
            .try_into()
            .unwrap(),
    ))
}
fn half(bytes: &[u8], offset: usize) -> Result<u16, String> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or("Truncated firmware header")?
            .try_into()
            .unwrap(),
    ))
}
fn parse_uf2(data: &[u8], target: Target) -> Result<Image, String> {
    if data.is_empty() || data.len() % 512 != 0 {
        return Err("UF2 must contain complete 512-byte blocks".into());
    }
    let mut blocks = BTreeMap::<u32, Segment>::new();
    let mut declared = None;
    let mut family = None;
    for block in data.chunks_exact(512) {
        if word(block, 0)? != 0x0a324655
            || word(block, 4)? != 0x9e5d5157
            || word(block, 508)? != 0x0ab16f30
        {
            return Err("Invalid UF2 magic".into());
        }
        let flags = word(block, 8)?;
        let address = word(block, 12)?;
        let size = word(block, 16)? as usize;
        if flags & 0x1000 != 0 {
            return Err("UF2 file containers are not firmware images".into());
        }
        if flags & 1 != 0 {
            continue;
        }
        if flags & 0x2000 == 0 {
            return Err("UF2 is missing its chip family ID".into());
        }
        let f = word(block, 28)?;
        // Pico SDK RP2350 UF2s prepend this specific E10 compatibility block.
        // Other absolute-address images require an explicit format implementation.
        if f == UF2_ABSOLUTE
            && flags == 0xa000
            && address == 0x10ffff00
            && size == 256
            && word(block, 20)? == 0
            && word(block, 24)? == 2
        {
            continue;
        }
        let matches = match target {
            Target::Rp2040 => f == UF2_RP2040,
            Target::Rp2350 => f == UF2_ARM_S || f == UF2_ARM_NS,
        };
        if !matches {
            return Err(format!(
                "UF2 family 0x{f:08x} does not match {} Arm firmware",
                target.name()
            ));
        }
        if family.is_some_and(|old| old != f) {
            return Err("Mixed UF2 chip families".into());
        }
        family = Some(f);
        if size == 0 || size > 476 || size % 4 != 0 || address % 4 != 0 {
            return Err("Invalid UF2 payload size or alignment".into());
        }
        let number = word(block, 20)?;
        let total = word(block, 24)?;
        if total == 0 || number >= total || total as usize > data.len() / 512 {
            return Err("UF2 has missing blocks or invalid block numbers".into());
        }
        if declared.is_some_and(|old| old != total) {
            return Err("Inconsistent UF2 block count".into());
        }
        declared = Some(total);
        let bytes = block[32..32 + size].to_vec();
        if let Some(old) = blocks.get(&number) {
            if old.address != address || old.bytes != bytes {
                return Err("Conflicting duplicate UF2 block".into());
            }
        } else {
            blocks.insert(number, Segment { address, bytes });
        }
    }
    if declared.is_none() || blocks.len() != declared.unwrap() as usize {
        return Err("UF2 has no complete firmware image".into());
    }
    Ok(Image {
        segments: blocks.into_values().collect(),
        format: "UF2",
    })
}
fn parse_elf(data: &[u8]) -> Result<Image, String> {
    if data.len() < 52
        || &data[..4] != b"\x7fELF"
        || data[4] != 1
        || data[5] != 1
        || data[6] != 1
        || half(data, 16)? != 2
        || half(data, 18)? != 40
    {
        return Err("Expected a little-endian ARM executable ELF32 file".into());
    }
    let offset = word(data, 28)? as usize;
    let entry_size = half(data, 42)? as usize;
    let count = half(data, 44)? as usize;
    if entry_size < 32
        || count == 0
        || offset
            .checked_add(entry_size * count)
            .is_none_or(|end| end > data.len())
    {
        return Err("Invalid ELF program header table".into());
    }
    let mut segments = vec![];
    for i in 0..count {
        let h = offset + i * entry_size;
        if word(data, h)? != 1 {
            continue;
        }
        let size = word(data, h + 16)? as usize;
        if size == 0 {
            continue;
        }
        if size > word(data, h + 20)? as usize {
            return Err("ELF segment is larger than its memory range".into());
        }
        let start = word(data, h + 4)? as usize;
        let end = start.checked_add(size).ok_or("ELF file offset overflow")?;
        let bytes = data
            .get(start..end)
            .ok_or("Truncated ELF load segment")?
            .to_vec();
        segments.push(Segment {
            address: word(data, h + 12)?,
            bytes,
        });
    }
    Ok(Image {
        segments,
        format: "ELF",
    })
}

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
        let (events, rx) = mpsc::channel();
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
    link: ph_app_link::PhBoardLink,
    restore_off: bool,
}
impl Drop for Power {
    fn drop(&mut self) {
        if self.restore_off {
            let _ = self.link.set_power(false);
        }
    }
}
fn program(config: &Config, events: &Sender<Event>, cancel: &AtomicBool) -> Result<(), String> {
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
    let _lease = crate::gdb::Lease::acquire(format!("usb:{}", config.serial))?;
    let image_file = ImageFile::new(&image)?;
    // Check the configured binary before changing Work power.
    let check = Command::new(&config.openocd)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("OpenOCD: {e}"))?;
    if !check.success() {
        return Err("OpenOCD version check failed".into());
    }
    let _ = events.send(Event::Phase("Preparing"));
    let mut link =
        ph_app_link::PhBoardLink::open_serial(&config.serial).map_err(|e| e.to_string())?;
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
    events: &Sender<Event>,
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
    let (tx, rx) = mpsc::channel();
    fn reader<R: Read + Send + 'static>(input: R, tx: Sender<String>) {
        thread::spawn(move || {
            for line in BufReader::new(input).lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }
    reader(process.0.stdout.take().unwrap(), tx.clone());
    reader(process.0.stderr.take().unwrap(), tx);
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut complete = false;
    let line = |line: String, complete: &mut bool| match line.trim() {
        "PDBG_FLASH_WRITING" => {
            let _ = events.send(Event::Phase("Writing"));
        }
        "PDBG_FLASH_VERIFYING" => {
            let _ = events.send(Event::Phase("Verifying"));
        }
        "PDBG_FLASH_COMPLETE" => *complete = true,
        _ => {
            let _ = events.send(Event::Log(line));
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
    fn put(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn uf2(family: u32, number: u32, total: u32, address: u32) -> Vec<u8> {
        let mut data = vec![0; 512];
        for (offset, value) in [
            (0, 0x0a324655),
            (4, 0x9e5d5157),
            (8, 0x2000),
            (12, address),
            (16, 256),
            (20, number),
            (24, total),
            (28, family),
            (508, 0x0ab16f30),
        ] {
            put(&mut data, offset, value);
        }
        for i in 0..256 {
            data[32 + i] = i as u8;
        }
        data
    }
    fn elf(addresses: &[(u32, u32)]) -> Vec<u8> {
        let mut data = vec![0u8; 512];
        data[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
        data[16..18].copy_from_slice(&2u16.to_le_bytes());
        data[18..20].copy_from_slice(&40u16.to_le_bytes());
        put(&mut data, 20, 1);
        put(&mut data, 28, 52);
        data[42..44].copy_from_slice(&32u16.to_le_bytes());
        data[44..46].copy_from_slice(&(addresses.len() as u16).to_le_bytes());
        for (i, (virtual_address, load_address)) in addresses.iter().enumerate() {
            let header = 52 + i * 32;
            for (offset, value) in [
                (0, 1),
                (4, 256 + i as u32 * 16),
                (8, *virtual_address),
                (12, *load_address),
                (16, 16),
                (20, 32),
            ] {
                put(&mut data, header + offset, value);
            }
            data[256 + i * 16..272 + i * 16].fill((i + 1) as u8);
        }
        data
    }
    #[test]
    fn uf2_preserves_addresses_and_payload_not_container_bytes() {
        let mut bytes = uf2(UF2_RP2040, 1, 2, FLASH_BASE + 0x100);
        bytes.extend(uf2(UF2_RP2040, 0, 2, FLASH_BASE));
        let image = Image::parse("UF2", &bytes, Target::Rp2040).unwrap();
        assert_eq!(image.byte_count(), 512);
        assert_eq!(image.segments[0].address, FLASH_BASE);
        assert_eq!(&image.segments[0].bytes[..4], &[0, 1, 2, 3]);
        let hex = image.intel_hex();
        assert!(hex.starts_with(":020000041000EA\n"));
        assert!(hex.ends_with(":00000001FF\n"));
        for line in hex.lines() {
            let bytes: Vec<u8> = (1..line.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&line[i..i + 2], 16).unwrap())
                .collect();
            assert_eq!(bytes.iter().fold(0u8, |s, b| s.wrapping_add(*b)), 0);
        }
    }
    #[test]
    fn uf2_rejects_incomplete_corrupt_mismatched_and_overlapping_images() {
        let valid = uf2(UF2_RP2040, 0, 1, FLASH_BASE);
        assert!(Image::parse("uf2", &valid[..511], Target::Rp2040).is_err());
        assert!(Image::parse("uf2", &valid, Target::Rp2350).is_err());
        let mut corrupt = valid.clone();
        corrupt[508] = 0;
        assert!(Image::parse("uf2", &corrupt, Target::Rp2040).is_err());
        assert!(Image::parse("uf2", &uf2(UF2_RP2040, 0, 2, FLASH_BASE), Target::Rp2040).is_err());
        let mut overlap = uf2(UF2_RP2040, 0, 2, FLASH_BASE);
        overlap.extend(uf2(UF2_RP2040, 1, 2, FLASH_BASE + 128));
        assert!(Image::parse("uf2", &overlap, Target::Rp2040).is_err());
        assert!(Image::parse("uf2", &uf2(UF2_RP2040, 0, 1, 0xffff_ff00), Target::Rp2040).is_err());
    }
    #[test]
    fn pico2_sdk_prefix_is_not_programmed() {
        let mut e10 = uf2(UF2_ABSOLUTE, 0, 2, 0x10ffff00);
        put(&mut e10, 8, 0xa000);
        e10.extend(uf2(UF2_ARM_S, 0, 1, FLASH_BASE));
        let image = Image::parse("uf2", &e10, Target::Rp2350).unwrap();
        assert_eq!(image.segments.len(), 1);
        assert_eq!(image.segments[0].address, FLASH_BASE);
    }
    #[test]
    fn elf_uses_physical_load_addresses_and_rejects_ram_only_or_truncated_segments() {
        let bytes = elf(&[(FLASH_BASE, FLASH_BASE), (0x20000000, FLASH_BASE + 0x1000)]);
        let image = Image::parse("elf", &bytes, Target::Rp2040).unwrap();
        assert_eq!(image.segments[1].address, FLASH_BASE + 0x1000);
        assert_eq!(image.byte_count(), 32);
        assert!(Image::parse("elf", &bytes[..270], Target::Rp2040).is_err());
        assert!(Image::parse("elf", &elf(&[(0x20000000, 0x20000000)]), Target::Rp2040).is_err());
        let mut wrong_machine = bytes;
        wrong_machine[18] = 62;
        assert!(Image::parse("elf", &wrong_machine, Target::Rp2040).is_err());
    }
    #[test]
    fn ihex_splits_records_at_64k_boundaries() {
        let image = Image {
            segments: vec![Segment {
                address: FLASH_BASE + 0xfff0,
                bytes: vec![0xa5; 64],
            }],
            format: "BIN",
        };
        let hex = image.intel_hex();
        assert!(hex.contains(":020000041001E9\n"));
        assert!(hex.lines().any(|line| line.starts_with(":10FFF000")));
    }
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
        let (tx, _rx) = mpsc::channel();
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
