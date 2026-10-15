//! Opt-in native-window fixtures. Compiled only with `--features ui-perf`.
use crate::{live, logic::Decoder, model::Device, App, Page};
use eframe::{egui, glow::HasContext};
use picoharness_desktop::telemetry::{self, History, Sample};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::PathBuf,
    sync::mpsc::Sender,
    time::Instant,
};

pub const SCENARIOS: &[&str] = &[
    "idle",
    "live",
    "live-record-paused",
    "console",
    "console-flood",
    "console-hex",
    "trace-bulk",
    "history-limit",
    "logic",
    "logic-dense",
    "swd-registers",
    "swd-memory",
    "gdb-source",
    "firmware",
    "connected-live",
];

pub struct Config {
    pub scenario: String,
    pub probe: Option<String>,
    output: PathBuf,
    warmup: f64,
    duration: f64,
}
impl Config {
    pub fn persistence_path(&self) -> PathBuf {
        self.output.with_extension("egui.ron")
    }
    pub fn parse(args: &[String]) -> Result<Option<Self>, String> {
        let Some(index) = args.iter().position(|arg| arg == "--perf-scenario") else {
            return Ok(None);
        };
        let value = |key: &str| -> Result<&str, String> {
            args.iter()
                .position(|arg| arg == key)
                .and_then(|index| args.get(index + 1))
                .map(String::as_str)
                .ok_or_else(|| format!("Missing {key} value"))
        };
        let scenario = args.get(index + 1).ok_or("Missing performance scenario")?;
        if !SCENARIOS.contains(&scenario.as_str()) {
            return Err(format!("Unknown performance scenario: {scenario}"));
        }
        let seconds = |key: &str| -> Result<f64, String> {
            let seconds: f64 = value(key)?.parse().map_err(|_| format!("Invalid {key}"))?;
            if !seconds.is_finite() || !(0.1..=600.).contains(&seconds) {
                return Err(format!("{key} must be between 0.1 and 600 seconds"));
            }
            Ok(seconds)
        };
        Ok(Some(Self {
            scenario: scenario.clone(),
            probe: if scenario == "connected-live" {
                Some(value("--perf-probe")?.into())
            } else {
                None
            },
            output: value("--perf-output")?.into(),
            warmup: seconds("--perf-warmup")?,
            duration: seconds("--perf-duration")?,
        }))
    }
}

pub struct Runtime {
    pub config: Config,
    events: Option<Sender<live::Event>>,
    start: Instant,
    next_measurement: u64,
    next_message: u64,
    next_trace: u64,
    update_ms: Vec<f64>,
    render_cpu_ms: Vec<f64>,
    interval_ms: Vec<f64>,
    last_frame: Option<f64>,
    finished: bool,
    fixture_ms: f64,
    backend: Value,
}
impl Runtime {
    pub fn new(config: Config, cc: &eframe::CreationContext<'_>) -> Self {
        let backend = cc
            .gl
            .as_ref()
            .map(|gl| unsafe {
                json!({
                    "renderer": gl.get_parameter_string(eframe::glow::RENDERER),
                    "vendor": gl.get_parameter_string(eframe::glow::VENDOR),
                    "version": gl.get_parameter_string(eframe::glow::VERSION),
                })
            })
            .unwrap_or(Value::Null);
        Self {
            config,
            events: None,
            start: Instant::now(),
            next_measurement: 0,
            next_message: 0,
            next_trace: 0,
            update_ms: vec![],
            render_cpu_ms: vec![],
            interval_ms: vec![],
            last_frame: None,
            finished: false,
            fixture_ms: 0.,
            backend,
        }
    }
    pub fn install(&mut self, app: &mut App) -> Result<(), String> {
        let setup = Instant::now();
        let directory = self
            .config
            .output
            .parent()
            .ok_or("Missing output directory")?;
        fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        if !matches!(self.config.scenario.as_str(), "idle" | "connected-live") {
            app.start = Instant::now() - std::time::Duration::from_secs(360);
            let mut device = Device::base("ui-perf".into(), "UI performance fixture".into());
            device.online = true;
            device.live_ready = true;
            device.power = true;
            device.voltage = 3.3;
            device.console_framed = true;
            let (session, events) = live::Session::performance_source();
            device.live_session = Some(session);
            self.events = Some(events);
            match self.config.scenario.as_str() {
                "live" | "live-record-paused" => {
                    let count = if self.config.scenario == "live" {
                        1200
                    } else {
                        7200
                    };
                    for i in 0..count {
                        device.monitor.push(sample(360. - (count - i) as f64 / 20.));
                    }
                    if self.config.scenario == "live-record-paused" {
                        device.monitor.start("ui-perf", directory)?;
                        device.monitor.set_paused(true);
                    }
                }
                "console" | "console-flood" | "console-hex" | "trace-bulk" => {
                    device.page = Page::Console;
                    let (count, size) = match self.config.scenario.as_str() {
                        "console-flood" => (5000, 1024),
                        "console-hex" => (5000, 4096),
                        _ => (200, 64),
                    };
                    for i in 0..count {
                        device.serial_bytes(i as f64 / 10000., "RX", message(i, size));
                        if i % 64 == 0 {
                            device.audit.clear();
                        }
                    }
                    device.audit.clear();
                    if matches!(
                        self.config.scenario.as_str(),
                        "console-flood" | "console-hex"
                    ) {
                        device.received.resize(16 * 1024 * 1024, b'A');
                    }
                    device.hex = self.config.scenario == "console-hex";
                    device.trace_open = self.config.scenario == "trace-bulk";
                    if device.trace_open {
                        for _ in 0..1024 {
                            self.events
                                .as_ref()
                                .unwrap()
                                .send(live::Event::Trace {
                                    bus: "SPI",
                                    direction: "RX",
                                    bytes: vec![0xa7; 2048],
                                })
                                .map_err(|e| e.to_string())?;
                        }
                    }
                }
                "history-limit" => {
                    let path = directory.join("history-limit.jsonl");
                    let mut writer =
                        BufWriter::new(File::create(&path).map_err(|e| e.to_string())?);
                    serde_json::to_writer(
                        &mut writer,
                        &History {
                            version: 1,
                            serial: "ui-perf".into(),
                            ..Default::default()
                        },
                    )
                    .map_err(|e| e.to_string())?;
                    writeln!(writer).map_err(|e| e.to_string())?;
                    for i in 0..telemetry::SAMPLE_LIMIT {
                        serde_json::to_writer(&mut writer, &sample(i as f64 / 20.))
                            .map_err(|e| e.to_string())?;
                        writeln!(writer).map_err(|e| e.to_string())?;
                    }
                    writer.flush().map_err(|e| e.to_string())?;
                    device.monitor.load(&path)?;
                }
                "logic" | "logic-dense" => {
                    device.page = Page::Logic;
                    let dense = self.config.scenario == "logic-dense";
                    let pins: Vec<_> = device.logic.channels.iter().map(|c| c.pin).collect();
                    for channel in &mut device.logic.channels {
                        channel.enabled = true;
                    }
                    let samples = (0..32768)
                        .map(|i| {
                            pins.iter().enumerate().fold(0, |value, (index, pin)| {
                                let period = if dense { 2 } else { 8 + index * 7 };
                                value | ((((i / period) & 1) as u32) << pin)
                            })
                        })
                        .collect();
                    device.logic.set_capture(samples, 2_000_000);
                    if dense {
                        for pin in pins.into_iter().take(8) {
                            device.logic.add_decoder(Decoder::Pwm { pin });
                        }
                    } else {
                        for decoder in Decoder::catalog() {
                            device.logic.add_decoder(decoder);
                        }
                    }
                    device.logic.span = device.logic.duration();
                }
                "swd-registers" | "swd-memory" => {
                    device.page = Page::Swd;
                    device.swd = true;
                    device.running = self.config.scenario == "swd-registers";
                    device.registers_valid = [true; 2];
                    device.registers = std::array::from_fn(|i| 0x20000000 + i as u32 * 4);
                    device.core_registers = [device.registers; 2];
                    device.swd_tab = if device.running { 2 } else { 1 };
                    device.memory = (0..256).map(|i| i as u8).collect();
                    device.live_memory_address = Some(0x20000000);
                }
                "gdb-source" => {
                    device.page = Page::Gdb;
                    device.debugger.source =
                        (0..2000).map(|i| format!("    value += {i};\n")).collect();
                    device.debugger.source_path = "fixture.c".into();
                    device.debugger.source_line = 1000;
                    device.debugger.follow = false;
                }
                "firmware" => device.page = Page::Firmware,
                _ => unreachable!(),
            }
            app.devices.push(device);
        }
        self.fixture_ms = setup.elapsed().as_secs_f64() * 1000.;
        self.start = Instant::now();
        self.write(
            &self.config.output.with_extension("ready.json"),
            &json!({
                "started_unix_ms": telemetry::unix_ms(), "backend": self.backend,
                "fixture_setup_ms": self.fixture_ms,
            }),
        )?;
        Ok(())
    }
    pub fn inject(&mut self) {
        let elapsed = self.start.elapsed().as_secs_f64();
        let Some(events) = &self.events else {
            return;
        };
        if matches!(self.config.scenario.as_str(), "live" | "live-record-paused") {
            let due = (elapsed * 20.).floor() as u64;
            while self.next_measurement < due {
                self.next_measurement += 1;
                let sample = sample(360. + self.next_measurement as f64 / 20.);
                let _ = events.send(live::Event::Measurement {
                    value: device_control::Measurement {
                        shunt_uv: 0.,
                        bus_mv: sample.bus_voltage_v * 1000.,
                        current_ma: sample.current_ma,
                        power_mw: sample.power_mw,
                    },
                    captured_at: Instant::now(),
                    timestamp_unix_ms: sample.timestamp_unix_ms,
                });
            }
        }
        let (rate, size) = match self.config.scenario.as_str() {
            "console" | "trace-bulk" => (20., 64),
            "console-flood" => (1024., 1024),
            "console-hex" => (128., 4096),
            _ => (0., 0),
        };
        let due = (elapsed * rate).floor() as u64;
        while self.next_message < due {
            let _ = events.send(live::Event::UartRx(message(
                self.next_message as usize,
                size,
            )));
            self.next_message += 1;
        }
        if self.config.scenario == "trace-bulk" {
            let due = (elapsed * 512.).floor() as u64;
            while self.next_trace < due {
                let _ = events.send(live::Event::Trace {
                    bus: "SPI",
                    direction: "RX",
                    bytes: vec![0xa7; 2048],
                });
                self.next_trace += 1;
            }
        }
    }
    pub fn after_update(
        &mut self,
        ctx: &egui::Context,
        render_cpu: Option<f32>,
        update_start: Instant,
        devices: &[Device],
    ) {
        if self.finished {
            return;
        }
        let elapsed = self.start.elapsed().as_secs_f64();
        if elapsed >= self.config.warmup {
            self.update_ms
                .push(update_start.elapsed().as_secs_f64() * 1000.);
            if let Some(previous) = self.last_frame {
                self.interval_ms.push((elapsed - previous) * 1000.);
                if let Some(cpu) = render_cpu {
                    self.render_cpu_ms.push(cpu as f64 * 1000.);
                }
            }
            self.last_frame = Some(elapsed);
        }
        if elapsed < self.config.warmup + self.config.duration {
            return;
        }
        self.finished = true;
        let device = devices.first();
        let result = json!({
            "schema_version": 1, "scenario": self.config.scenario,
            "warmup_s": self.config.warmup, "requested_duration_s": self.config.duration,
            "observed_duration_s": elapsed - self.config.warmup,
            "frames": self.update_ms.len(),
            "frames_per_s": self.update_ms.len() as f64 / (elapsed - self.config.warmup),
            "update_ms": distribution(&self.update_ms),
            "render_cpu_ms": distribution(&self.render_cpu_ms),
            "frame_interval_ms": distribution(&self.interval_ms),
            "backend": self.backend, "fixture_setup_ms": self.fixture_ms,
            "viewport_points": [ctx.screen_rect().width(), ctx.screen_rect().height()],
            "pixels_per_point": ctx.pixels_per_point(),
            "injected": { "measurements": self.next_measurement, "messages": self.next_message, "spi_chunks": self.next_trace },
            "buffers": device.map(|d| json!({
                "console_rows": d.logs.len(), "received_bytes": d.received.len(),
                "trace_rows": d.trace.len(), "trace_bytes": d.trace_size,
                "live_samples": d.monitor.live.len(),
                "history_samples": d.monitor.history().map_or(0, |h| h.samples.len()),
                "logic_samples": d.logic.samples.len(), "decoded_events": d.logic.annotations.len(),
                "live_ready": d.live_ready, "recording": d.monitor.recording(), "paused": d.monitor.paused(),
            })),
        });
        if let Err(error) = self.write(&self.config.output, &result) {
            eprintln!("Performance report: {error}");
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    fn write(&self, path: &std::path::Path, value: &Value) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, bytes)
            .and_then(|_| fs::rename(&temporary, path))
            .map_err(|e| e.to_string())
    }
}
fn sample(time: f64) -> Sample {
    let current = 20. + 8. * (time * 2.).sin();
    Sample {
        timestamp_unix_ms: 1_700_000_000_000 + (time * 1000.) as u64,
        elapsed_s: time,
        bus_voltage_v: 3.3 + 0.01 * time.sin(),
        work_3v3_v: 3.3,
        current_ma: current,
        power_mw: current * 3.3,
        power_on: true,
        power_fault: false,
    }
}
fn message(index: usize, size: usize) -> Vec<u8> {
    let mut bytes = vec![b'A' + (index % 26) as u8; size];
    if let Some(last) = bytes.last_mut() {
        *last = b'\n';
    }
    bytes
}
fn distribution(values: &[f64]) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| sorted[((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    json!({ "mean": values.iter().sum::<f64>() / values.len() as f64,
        "p50": percentile(0.5), "p95": percentile(0.95), "p99": percentile(0.99), "max": sorted.last() })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unknown_nonfinite_and_unselected_hardware_scenarios() {
        let args = |scenario: &str, time: &str| {
            [
                "app",
                "--perf-scenario",
                scenario,
                "--perf-output",
                "/tmp/perf/ui.json",
                "--perf-warmup",
                "1",
                "--perf-duration",
                time,
            ]
            .map(String::from)
        };
        assert!(Config::parse(&args("live", "8")).unwrap().is_some());
        for (scenario, time) in [
            ("demo", "8"),
            ("live", "NaN"),
            ("live", "0"),
            ("connected-live", "8"),
        ] {
            assert!(Config::parse(&args(scenario, time)).is_err());
        }
        assert!(Config::parse(&["app".into()]).unwrap().is_none());
    }
    #[test]
    fn percentiles_use_the_nearest_rank_including_outliers() {
        let values: Vec<_> = (1..=100).map(f64::from).collect();
        let summary = distribution(&values);
        assert_eq!(summary["mean"], 50.5);
        assert_eq!(summary["p95"], 95.);
        assert_eq!(summary["p99"], 99.);
        assert!(distribution(&[]).is_null());
    }
}
