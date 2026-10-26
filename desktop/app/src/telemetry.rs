//! Real measurements, bounded live history, and durable recording sessions.
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const SAMPLE_LIMIT: usize = 200_000;
pub const MAX_SAMPLE_GAP: f64 = 2.0;

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn recording_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("PDBG_RECORDINGS") {
        return path.into();
    }
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()))
                .join(".local/share")
        });
    root.join("picoharness/recordings")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    pub timestamp_unix_ms: u64,
    pub elapsed_s: f64,
    pub bus_voltage_v: f64,
    pub work_3v3_v: f64,
    pub current_ma: f64,
    pub power_mw: f64,
    pub power_on: bool,
    pub power_fault: bool,
}
impl Sample {
    pub fn value(&self, metric: usize) -> f64 {
        match metric {
            0 => self.power_mw,
            1 => self.bus_voltage_v,
            _ => self.current_ma,
        }
    }
    fn valid(&self) -> bool {
        [
            self.elapsed_s,
            self.bus_voltage_v,
            self.work_3v3_v,
            self.current_ma,
            self.power_mw,
        ]
        .iter()
        .all(|v| v.is_finite())
            && self.elapsed_s >= 0.
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct History {
    pub version: u8,
    pub serial: String,
    pub started_unix_ms: u64,
    #[serde(skip)]
    pub samples: Vec<Sample>,
    #[serde(skip)]
    pub path: Option<PathBuf>,
}
impl History {
    pub fn read(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() > 128 * 1024 * 1024 {
            return Err("Recording file exceeds 128 MB".into());
        }
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let mut history: Self = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if history.version != 1 || history.serial.is_empty() {
            return Err("Unsupported recording format".into());
        }
        loop {
            line.clear();
            if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                break;
            }
            let sample: Sample = match serde_json::from_str(&line) {
                Ok(sample) => sample,
                // A crash can leave one unfinished trailing line. Complete rows remain readable.
                Err(e) if !line.ends_with('\n') && e.is_eof() => break,
                Err(e) => return Err(format!("Invalid recording row: {e}")),
            };
            if history.samples.len() >= SAMPLE_LIMIT
                || !sample.valid()
                || history.samples.last().is_some_and(|last| {
                    sample.elapsed_s < last.elapsed_s
                        || sample.timestamp_unix_ms < last.timestamp_unix_ms
                })
            {
                return Err("Invalid or oversized recording history".into());
            }
            history.samples.push(sample);
        }
        history.path = Some(path.into());
        Ok(history)
    }
    pub fn csv(&self) -> String {
        let mut csv = String::from("device_serial,timestamp_unix_ms,elapsed_s,bus_voltage_v,work_3v3_v,current_ma,power_mw,power_on,power_fault\n");
        let serial = self.serial.replace('"', "\"\"");
        for s in &self.samples {
            csv.push_str(&format!(
                "\"{serial}\",{},{:.6},{:.6},{:.6},{:.6},{:.6},{},{}\n",
                s.timestamp_unix_ms,
                s.elapsed_s,
                s.bus_voltage_v,
                s.work_3v3_v,
                s.current_ma,
                s.power_mw,
                s.power_on,
                s.power_fault
            ));
        }
        csv
    }
    pub fn duration(&self) -> f64 {
        self.samples
            .last()
            .zip(self.samples.first())
            .map_or(0., |(last, first)| last.elapsed_s - first.elapsed_s)
    }
}

#[derive(Clone, Copy, Default)]
pub struct ChartViewport {
    pub time: Option<(f64, f64)>,
    pub values: [Option<(f64, f64)>; 3],
}

pub struct Monitor {
    pub live: VecDeque<Sample>,
    pub session: Option<History>,
    pub replay: Option<History>,
    pub history_view: bool,
    pub window_s: f64,
    pub sample_hz: u32,
    pub metric: usize,
    pub zero_baseline: bool,
    pub viewport: ChartViewport,
    pub error: String,
    writer: Option<BufWriter<File>>,
    origin: Option<f64>,
    frozen: Option<(f64, VecDeque<Sample>)>,
}
impl Default for Monitor {
    fn default() -> Self {
        Self {
            live: VecDeque::new(),
            session: None,
            replay: None,
            history_view: false,
            window_s: 60.,
            sample_hz: 2,
            metric: 0,
            zero_baseline: false,
            viewport: ChartViewport::default(),
            error: String::new(),
            writer: None,
            origin: None,
            frozen: None,
        }
    }
}
impl Monitor {
    pub fn paused(&self) -> bool {
        self.frozen.is_some()
    }
    pub fn set_paused(&mut self, paused: bool) {
        self.frozen = if paused {
            Some((
                self.live.back().map_or(0., |sample| sample.elapsed_s),
                self.live.clone(),
            ))
        } else {
            None
        };
    }
    pub fn display_time(&self, now: f64) -> f64 {
        self.frozen.as_ref().map_or(now, |(time, _)| *time)
    }
    pub fn recording(&self) -> bool {
        self.writer.is_some()
    }
    pub fn start(&mut self, serial: &str, dir: &Path) -> Result<(), String> {
        if self.recording() {
            return Err("A recording is already running".into());
        }
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let timestamp = unix_ms();
        let safe: String = serial
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let path = dir.join(format!("pico-{safe}-{timestamp}.jsonl"));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let history = History {
            version: 1,
            serial: serial.into(),
            started_unix_ms: timestamp,
            samples: Vec::new(),
            path: Some(path),
        };
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, &history).map_err(|e| e.to_string())?;
        writer
            .write_all(b"\n")
            .and_then(|_| writer.flush())
            .map_err(|e| e.to_string())?;
        self.writer = Some(writer);
        self.session = Some(history);
        self.origin = None;
        self.error.clear();
        Ok(())
    }
    pub fn stop(&mut self) -> Result<(), String> {
        if let Some(mut writer) = self.writer.take() {
            writer
                .flush()
                .and_then(|_| writer.get_ref().sync_data())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn push(&mut self, sample: Sample) {
        if !sample.valid()
            || self
                .live
                .back()
                .is_some_and(|last| sample.elapsed_s < last.elapsed_s)
        {
            return;
        }
        self.live.push_back(sample.clone());
        while self.live.len() > 7200 {
            self.live.pop_front();
        }
        if let Some(writer) = &mut self.writer {
            let mut row = sample;
            row.elapsed_s -= *self.origin.get_or_insert(row.elapsed_s);
            let history = self
                .session
                .as_mut()
                .expect("recording has session metadata");
            let result = if history.samples.len() >= SAMPLE_LIMIT {
                Err("Recording limit reached; start a new session".into())
            } else {
                serde_json::to_writer(&mut *writer, &row)
                    .map_err(|e| e.to_string())
                    .and_then(|_| {
                        writer
                            .write_all(b"\n")
                            .and_then(|_| writer.flush())
                            .map_err(|e| e.to_string())
                    })
            };
            match result {
                Ok(()) => history.samples.push(row),
                Err(error) => {
                    self.error = error;
                    let _ = self.stop();
                }
            }
        }
    }
    pub fn history(&self) -> Option<&History> {
        self.replay.as_ref().or(self.session.as_ref())
    }
    pub fn samples(&self, now: f64) -> Vec<&Sample> {
        if self.history_view {
            return self
                .history()
                .map(|h| {
                    h.samples
                        .iter()
                        .filter(|s| {
                            self.viewport.time.is_none_or(|(start, end)| {
                                s.elapsed_s >= start && s.elapsed_s <= end
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        let (now, live) = self
            .frozen
            .as_ref()
            .map_or((now, &self.live), |(time, live)| (*time, live));
        let (start, end) = self.viewport.time.unwrap_or((now - self.window_s, now));
        live.iter()
            .filter(|s| s.elapsed_s >= start && s.elapsed_s <= end)
            .collect()
    }
    pub fn load(&mut self, path: &Path) -> Result<(), String> {
        self.replay = Some(History::read(path)?);
        self.history_view = true;
        self.viewport = ChartViewport::default();
        Ok(())
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[derive(Default, Debug)]
pub struct Stats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub energy_mwh: f64,
}
pub fn stats(samples: &[&Sample], metric: usize) -> Option<Stats> {
    let first = samples.first()?;
    let mut stats = Stats {
        min: first.value(metric),
        max: first.value(metric),
        ..Default::default()
    };
    for s in samples {
        let value = s.value(metric);
        stats.min = stats.min.min(value);
        stats.max = stats.max.max(value);
        stats.mean += value;
    }
    stats.mean /= samples.len() as f64;
    for pair in samples.windows(2) {
        let gap = pair[1].elapsed_s - pair[0].elapsed_s;
        if (0. ..=MAX_SAMPLE_GAP).contains(&gap) {
            stats.energy_mwh += (pair[0].power_mw + pair[1].power_mw) * 0.5 * gap / 3600.;
        }
    }
    Some(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pause_freezes_view_but_keeps_recording_and_resumes_at_latest() {
        let dir = std::env::temp_dir().join(format!("ph-app-pause-{}", unix_ms()));
        let mut monitor = Monitor::default();
        monitor.start("test", &dir).unwrap();
        monitor.push(sample(1.));
        monitor.set_paused(true);
        monitor.push(sample(2.));
        assert_eq!(monitor.samples(2.).last().unwrap().elapsed_s, 1.);
        assert_eq!(monitor.session.as_ref().unwrap().samples.len(), 2);
        let path = monitor.session.as_ref().unwrap().path.clone().unwrap();
        monitor.stop().unwrap();
        assert_eq!(History::read(&path).unwrap().samples.len(), 2);
        monitor.set_paused(false);
        assert_eq!(monitor.samples(2.).last().unwrap().elapsed_s, 2.);
        std::fs::remove_dir_all(dir).unwrap();
    }
    fn sample(t: f64) -> Sample {
        Sample {
            timestamp_unix_ms: 1_700_000_000_000 + (t * 1000.) as u64,
            elapsed_s: t,
            bus_voltage_v: 5.,
            work_3v3_v: 3.3,
            current_ma: 20.,
            power_mw: 100.,
            power_on: true,
            power_fault: false,
        }
    }
    #[test]
    fn recording_flushes_every_sample_and_round_trips_into_csv() {
        let dir = std::env::temp_dir().join(format!("ph-app-history-{}", std::process::id()));
        let mut m = Monitor::default();
        m.start("test-serial", &dir).unwrap();
        m.push(sample(10.));
        m.push(sample(10.5));
        let path = m.session.as_ref().unwrap().path.clone().unwrap();
        let h = History::read(&path).unwrap(); // Read while recording, before stop.
        assert_eq!(h.samples.len(), 2);
        assert_eq!(h.samples[1].elapsed_s, 0.5);
        assert_eq!(
            h.samples[0].timestamp_unix_ms,
            sample(10.).timestamp_unix_ms
        );
        assert!(h
            .csv()
            .contains("5.000000,3.300000,20.000000,100.000000,true,false"));
        m.stop().unwrap();
        m.load(&path).unwrap();
        assert!(m.history_view);
        assert_eq!(m.samples(1000.).len(), 2);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"timestamp_unix_ms\":").unwrap();
        assert_eq!(History::read(&path).unwrap().samples.len(), 2);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn stats_do_not_integrate_disconnected_time_or_invent_samples() {
        let rows = [sample(0.), sample(0.5), sample(30.), sample(30.5)];
        let refs: Vec<_> = rows.iter().collect();
        let s = stats(&refs, 0).unwrap();
        assert_eq!(s.mean, 100.);
        assert!((s.energy_mwh - 100. / 3600.).abs() < 1e-9);
        let mut m = Monitor::default();
        let mut invalid = sample(31.);
        invalid.current_ma = f64::NAN;
        m.push(invalid);
        assert!(m.live.is_empty());
        assert!(stats(&[], 0).is_none());
    }
}
