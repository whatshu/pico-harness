//! Finite captures and configurable UART/SPI/I2C/PWM decoders, independent of the UI.

// Work Core GPIO numbers, normalized by the dualcore Debug firmware's
// PDBG_GPIO_MAP. GP25 (the Pico LED) has no carrier observation connection.
pub const PINS: [u8; 16] = [
    5, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 26, 27, 28,
];
pub const DEFAULT_PINS: [u8; 7] = [5, 12, 13, 18, 19, 20, 21];
pub use ph_app_link::logic_capture::RATES as SAMPLE_RATES;
pub const MAX_SAMPLES: usize = 32768;

pub fn sample_rate_label(rate: u32) -> String {
    for (scale, unit) in [(1_000_000_000, "GS/s"), (1_000_000, "MS/s"), (1_000, "kS/s")] {
        if rate >= scale {
            let value = rate as f64 / scale as f64;
            return if rate % scale == 0 {
                format!("{value:.0} {unit}")
            } else {
                format!("{value:.3} {unit}")
            };
        }
    }
    format!("{rate} S/s")
}
#[derive(Clone)]
pub struct Channel {
    pub pin: u8,
    pub name: String,
    pub enabled: bool,
}
#[derive(Clone, Debug)]
pub struct Annotation {
    pub decoder_id: u64,
    pub start: f64,
    pub end: f64,
    pub protocol: String,
    pub text: String,
    pub error: bool,
}
#[derive(Clone, PartialEq)]
pub enum Decoder {
    Uart {
        pin: u8,
        baud: u32,
        bits: u8,
        parity: u8,
        stop: u8,
    },
    Spi {
        pins: [u8; 4],
        mode: u8,
        lsb: bool,
        bits: u8,
    },
    I2c {
        scl: u8,
        sda: u8,
    },
    Pwm {
        pin: u8,
    },
}
impl Decoder {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Uart { .. } => "UART",
            Self::Spi { .. } => "SPI",
            Self::I2c { .. } => "I²C",
            Self::Pwm { .. } => "PWM",
        }
    }
    pub fn catalog() -> [Self; 4] {
        [
            Self::Uart {
                pin: 12,
                baud: 115200,
                bits: 8,
                parity: 0,
                stop: 1,
            },
            Self::Spi {
                pins: [18, 20, 19, 21],
                mode: 0,
                lsb: false,
                bits: 8,
            },
            Self::I2c { scl: 10, sda: 11 },
            Self::Pwm { pin: 5 },
        ]
    }
}
#[derive(Clone, PartialEq)]
pub struct DecoderInstance {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    pub decoder: Decoder,
}
pub struct Logic {
    pub channels: Vec<Channel>,
    pub samples: Vec<u32>,
    pub rate: u32,
    pub count: usize,
    pub packed_capture: bool,
    pub duration_s: f64,
    pub offset: f64,
    pub span: f64,
    pub cursors: bool,
    pub a: f64,
    pub b: f64,
    pub markers: Vec<(f64, String)>,
    pub decoders: Vec<DecoderInstance>,
    next_decoder_id: u64,
    pub annotations: Vec<Annotation>,
    pub selected: Option<usize>,
    pub capturing: Option<f64>,
    pub trigger: u8,
    pub trigger_index: Option<u32>,
    pub trigger_pin: u8,
    pub trigger_timeout_ms: u16,
    pub sample_rate: u32,
}
impl Default for Logic {
    fn default() -> Self {
        Self {
            channels: PINS
                .into_iter()
                .map(|pin| Channel {
                    pin,
                    name: String::new(),
                    enabled: DEFAULT_PINS.contains(&pin),
                })
                .collect(),
            samples: vec![],
            rate: 2_000_000,
            sample_rate: 2_000_000,
            count: MAX_SAMPLES,
            packed_capture: false,
            duration_s: MAX_SAMPLES as f64 / 2_000_000.,
            offset: 0.,
            span: 0.004,
            cursors: true,
            a: 0.0004,
            b: 0.0014,
            markers: vec![],
            decoders: vec![],
            next_decoder_id: 1,
            annotations: vec![],
            selected: None,
            capturing: None,
            trigger: 0,
            trigger_index: None,
            trigger_pin: 5,
            trigger_timeout_ms: 1000,
        }
    }
}
impl Logic {
    pub fn select_channels(&mut self, selection: Option<bool>) {
        for channel in &mut self.channels {
            channel.enabled = selection.unwrap_or_else(|| DEFAULT_PINS.contains(&channel.pin));
        }
        if selection == Some(false) {
            for decoder in &mut self.decoders {
                decoder.enabled = false;
            }
            self.decode();
        }
        if selection.is_none() {
            self.decoders.clear();
            for decoder in [
                Decoder::catalog()[0].clone(),
                Decoder::Uart {
                    pin: 13,
                    baud: 115200,
                    bits: 8,
                    parity: 0,
                    stop: 1,
                },
                Decoder::catalog()[1].clone(),
                Decoder::catalog()[3].clone(),
            ] {
                self.add_decoder(decoder);
            }
        }
    }
    pub fn channel_mask(&self) -> u32 {
        self.channels.iter().filter(|c| c.enabled).fold(0, |mask, c| mask | (1u32 << c.pin))
    }
    pub fn capture_mask(&self) -> u32 { if self.packed_capture { self.channel_mask() } else { 0 } }
    pub fn max_samples(&self) -> usize {
        if self.packed_capture {
            ph_app_link::logic_capture::profile(self.channel_mask(), self.trigger, self.trigger_pin)
                .map_or(MAX_SAMPLES, |p| p.max_samples as usize)
        } else { MAX_SAMPLES }
    }
    pub fn update_sampling(&mut self) {
        if !SAMPLE_RATES.contains(&self.rate) {
            self.rate = 2_000_000;
        }
        // Edge acquisition takes four PIO cycles on RP2040; 50 MS/s is burst-only.
        if self.trigger != 0 && self.rate > 20_000_000 {
            self.rate = 20_000_000;
        }
        self.trigger_timeout_ms = self.trigger_timeout_ms.clamp(1, 5000);
        let maximum = self.max_samples();
        if !self.duration_s.is_finite() {
            self.duration_s = maximum as f64 / self.rate as f64;
        }
        let minimum = if self.trigger == 0 { 1. } else { 8. };
        self.duration_s = self.duration_s.clamp(
            minimum / self.rate as f64,
            maximum as f64 / self.rate as f64,
        );
        self.count = (self.duration_s * self.rate as f64)
            .round()
            .clamp(minimum, maximum as f64) as usize;
    }
    pub fn enable_decoder_channels(&mut self) {
        let mut pins = Vec::new();
        for instance in self.decoders.iter().filter(|d| d.enabled) {
            match instance.decoder {
                Decoder::Uart { pin, .. } | Decoder::Pwm { pin } => pins.push(pin),
                Decoder::Spi { pins: channels, .. } => pins.extend(channels),
                Decoder::I2c { scl, sda } => pins.extend([scl, sda]),
            }
        }
        for channel in &mut self.channels {
            if pins.contains(&channel.pin) {
                channel.enabled = true;
            }
        }
    }

    pub fn add_decoder(&mut self, decoder: Decoder) {
        let id = self.next_decoder_id;
        self.next_decoder_id += 1;
        let count = self
            .decoders
            .iter()
            .filter(|instance| instance.decoder.name() == decoder.name())
            .count();
        let name = if count == 0 {
            decoder.name().into()
        } else {
            format!("{} {id}", decoder.name())
        };
        self.decoders.push(DecoderInstance {
            id,
            name,
            enabled: true,
            decoder,
        });
        self.enable_decoder_channels();
        self.decode();
    }
    pub fn set_capture(&mut self, samples: Vec<u32>, rate: u32) {
        self.trigger_index = None;
        self.sample_rate = rate.max(1);
        self.samples = samples;
        self.offset = 0.;
        self.span = self.duration().min(0.004);
        self.decode();
    }
    pub fn duration(&self) -> f64 {
        self.samples.len() as f64 / self.sample_rate as f64
    }
    pub fn decode(&mut self) {
        self.annotations.clear();
        self.selected = None;
        for instance in self.decoders.iter().filter(|instance| instance.enabled) {
            let mut annotations = match instance.decoder {
                Decoder::Uart {
                    pin,
                    baud,
                    bits,
                    parity,
                    stop,
                } => decode_uart(
                    &self.samples,
                    self.sample_rate,
                    pin,
                    baud,
                    bits,
                    parity,
                    stop,
                ),
                Decoder::Spi {
                    pins,
                    mode,
                    lsb,
                    bits,
                } => decode_spi(&self.samples, self.sample_rate, pins, mode, lsb, bits),
                Decoder::I2c { scl, sda } => decode_i2c(&self.samples, self.sample_rate, scl, sda),
                Decoder::Pwm { pin } => decode_pwm(&self.samples, self.sample_rate, pin),
            };
            for annotation in &mut annotations {
                annotation.protocol = instance.name.clone();
                annotation.decoder_id = instance.id;
            }
            self.annotations.extend(annotations);
        }
        self.annotations.sort_by(|a, b| a.start.total_cmp(&b.start));
    }
    pub fn csv(&self) -> String {
        let mut s = "time_s,sample_hex\n".to_string();
        for (i, v) in self.samples.iter().enumerate() {
            s.push_str(&format!(
                "{:.9},0x{v:08x}\n",
                i as f64 / self.sample_rate as f64
            ));
        }
        s
    }
    pub fn vcd(&self) -> String {
        let mut s = "$timescale 1 ns $end\n$scope module work $end\n".to_string();
        for c in &self.channels {
            s.push_str(&format!("$var wire 1 p{} GP{} $end\n", c.pin, c.pin));
        }
        s.push_str("$upscope $end\n$enddefinitions $end\n");
        let mut last = None;
        for (i, v) in self.samples.iter().enumerate() {
            if Some(*v) == last {
                continue;
            }
            s.push_str(&format!(
                "#{}\n",
                i as u64 * 1_000_000_000 / self.sample_rate as u64
            ));
            for c in &self.channels {
                if last.is_none_or(|p: u32| (p ^ v) & (1 << c.pin) != 0) {
                    s.push_str(&format!("{}p{}\n", (v >> c.pin) & 1, c.pin));
                }
            }
            last = Some(*v);
        }
        s
    }
    pub fn annotation_csv(&self) -> String {
        let mut s = String::from("start_s,end_s,protocol,value,error\n");
        for a in &self.annotations {
            s.push_str(&format!(
                "{:.9},{:.9},\"{}\",\"{}\",{}\n",
                a.start,
                a.end,
                a.protocol.replace('"', "\"\""),
                a.text.replace('"', "\"\""),
                a.error
            ));
        }
        s
    }
}
fn bit(samples: &[u32], i: usize, pin: u8) -> bool {
    samples.get(i).is_some_and(|v| v & (1u32 << pin) != 0)
}
fn decode_pwm(samples: &[u32], rate: u32, pin: u8) -> Vec<Annotation> {
    let rising: Vec<_> = (1..samples.len())
        .filter(|i| !bit(samples, i - 1, pin) && bit(samples, *i, pin))
        .collect();
    rising
        .windows(2)
        .filter_map(|edges| {
            let start = edges[0];
            let end = edges[1];
            let falling = (start + 1..end).find(|i| !bit(samples, *i, pin))?;
            Some(Annotation {
                decoder_id: 0,
                start: start as f64 / rate as f64,
                end: end as f64 / rate as f64,
                protocol: "PWM".into(),
                text: format!(
                    "{:.3} Hz · {:.1}%",
                    rate as f64 / (end - start) as f64,
                    (falling - start) as f64 * 100. / (end - start) as f64
                ),
                error: false,
            })
        })
        .collect()
}
fn decode_i2c(samples: &[u32], rate: u32, scl: u8, sda: u8) -> Vec<Annotation> {
    let mut output = vec![];
    let mut active = false;
    let mut first = true;
    let mut value = 0u8;
    let mut bits = 0;
    let mut start = 0;
    for i in 1..samples.len() {
        let clock = bit(samples, i, scl);
        let data = bit(samples, i, sda);
        if clock && bit(samples, i - 1, scl) && bit(samples, i - 1, sda) != data {
            active = !data;
            first = true;
            bits = 0;
            value = 0;
            output.push(Annotation {
                decoder_id: 0,
                start: (i - 1) as f64 / rate as f64,
                end: i as f64 / rate as f64,
                protocol: "I²C".into(),
                text: if data { "STOP" } else { "START" }.into(),
                error: false,
            });
        } else if active && clock && !bit(samples, i - 1, scl) {
            if bits == 0 {
                start = i;
            }
            if bits < 8 {
                value = (value << 1) | data as u8;
                bits += 1;
            } else {
                let text = if first {
                    format!(
                        "0x{:02X} {} · {}",
                        value >> 1,
                        if value & 1 == 0 { "W" } else { "R" },
                        if data { "NACK" } else { "ACK" }
                    )
                } else {
                    format!("0x{value:02X} · {}", if data { "NACK" } else { "ACK" })
                };
                output.push(Annotation {
                    decoder_id: 0,
                    start: start as f64 / rate as f64,
                    end: i as f64 / rate as f64,
                    protocol: "I²C".into(),
                    text,
                    error: data && first,
                });
                first = false;
                bits = 0;
                value = 0;
            }
        }
    }
    output
}
#[cfg(test)]
fn generate(rate: u32, count: usize) -> Vec<u32> {
    (0..count)
        .map(|i| {
            let t = i as f64 / rate as f64;
            let mut v = (1 << 12) | (1 << 13) | (1 << 21);
            let uart = (t - 0.00025) * 115200.;
            if uart >= 0. && uart < 50. {
                let frame = (uart / 10.) as usize;
                let b = uart as usize % 10;
                let value = b"PICO!"[frame];
                if b == 0 || (1..=8).contains(&b) && value & (1 << (b - 1)) == 0 {
                    v &= !(1 << 12);
                }
            }
            let spi = (t - 0.001) * 100000.;
            if spi >= 0. && spi < 32. {
                v &= !(1 << 21);
                let n = spi as usize;
                let mosi = [0x9fu8, 0x00, 0xA5, 0x5A][n / 8];
                let miso = [0u8, 0xEF, 0x40, 0x18][n / 8];
                if spi.fract() >= 0.5 {
                    v |= 1 << 18;
                }
                if mosi & (1 << (7 - n % 8)) != 0 {
                    v |= 1 << 20;
                }
                if miso & (1 << (7 - n % 8)) != 0 {
                    v |= 1 << 19;
                }
            }
            if (t * 10000.).fract() < 0.35 {
                v |= 1 << 5;
            }
            v
        })
        .collect()
}
pub fn decode_uart(
    samples: &[u32],
    rate: u32,
    pin: u8,
    baud: u32,
    bits: u8,
    parity: u8,
    stop: u8,
) -> Vec<Annotation> {
    let mut out = Vec::new();
    if baud == 0 || rate < baud * 2 || !(5..=8).contains(&bits) {
        return out;
    }
    let period = rate as f64 / baud as f64;
    let mut i = 1;
    while i < samples.len() {
        if bit(samples, i - 1, pin) && !bit(samples, i, pin) {
            let end = i as f64
                + period * (1. + bits as f64 + if parity > 0 { 1. } else { 0. } + stop as f64);
            if end >= samples.len() as f64 {
                break;
            }
            if bit(samples, (i as f64 + period * 0.5) as usize, pin) {
                i += 1;
                continue;
            }
            let mut data = 0u8;
            for b in 0..bits {
                if bit(
                    samples,
                    (i as f64 + period * (1.5 + b as f64)) as usize,
                    pin,
                ) {
                    data |= 1 << b;
                }
            }
            let mut error = false;
            if parity > 0 {
                let p = bit(
                    samples,
                    (i as f64 + period * (1.5 + bits as f64)) as usize,
                    pin,
                );
                let odd = data.count_ones() % 2 == 1;
                error = p != if parity == 1 { odd } else { !odd };
            }
            for stop_bit in 0..stop {
                error |= !bit(
                    samples,
                    (i as f64
                        + period
                            * (1.5
                                + bits as f64
                                + if parity > 0 { 1. } else { 0. }
                                + stop_bit as f64)) as usize,
                    pin,
                );
            }
            let display = if data.is_ascii_graphic() {
                format!("0x{data:02X} '{}'", data as char)
            } else {
                format!("0x{data:02X}")
            };
            out.push(Annotation {
                decoder_id: 0,
                start: i as f64 / rate as f64,
                end: end / rate as f64,
                protocol: "UART".into(),
                text: display,
                error,
            });
            i = (end - period * 0.3) as usize;
        } else {
            i += 1;
        }
    }
    out
}
pub fn decode_spi(
    samples: &[u32],
    rate: u32,
    pins: [u8; 4],
    mode: u8,
    lsb: bool,
    bits: u8,
) -> Vec<Annotation> {
    let [clk, mosi, miso, cs] = pins;
    let mut out = Vec::new();
    let mut count = 0;
    let (mut tx, mut rx) = (0u32, 0u32);
    let mut start = 0;
    for i in 1..samples.len() {
        if bit(samples, i, cs) {
            count = 0;
            tx = 0;
            rx = 0;
            continue;
        }
        let rising = !bit(samples, i - 1, clk) && bit(samples, i, clk);
        let falling = bit(samples, i - 1, clk) && !bit(samples, i, clk);
        let sample_rising = (mode & 2 == 0) == (mode & 1 == 0);
        if (sample_rising && rising) || (!sample_rising && falling) {
            if count == 0 {
                start = i;
            }
            let shift = if lsb { count } else { bits - 1 - count };
            tx |= (bit(samples, i, mosi) as u32) << shift;
            rx |= (bit(samples, i, miso) as u32) << shift;
            count += 1;
            if count == bits {
                out.push(Annotation {
                    decoder_id: 0,
                    start: start as f64 / rate as f64,
                    end: i as f64 / rate as f64,
                    protocol: "SPI".into(),
                    text: format!("MOSI {tx:02X} / MISO {rx:02X}"),
                    error: false,
                });
                count = 0;
                tx = 0;
                rx = 0;
            }
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_channels_and_default_decoders_remain_consistent() {
        let mut logic = Logic::default();
        assert!(logic
            .channels
            .windows(2)
            .all(|pair| pair[0].pin < pair[1].pin));
        assert!(logic.channels.iter().all(|channel| channel.name.is_empty()));
        logic.channels[0].name = "Clock reference".into();
        logic.select_channels(None);
        assert_eq!(
            logic
                .channels
                .iter()
                .filter(|c| c.enabled)
                .map(|c| c.pin)
                .collect::<Vec<_>>(),
            DEFAULT_PINS
        );
        assert_eq!(logic.decoders.len(), 4);
        assert_eq!(logic.channels[0].name, "Clock reference");
        logic.select_channels(Some(false));
        assert!(logic.channels.iter().all(|channel| !channel.enabled));
        assert!(logic.decoders.iter().all(|decoder| !decoder.enabled));
        logic.add_decoder(Decoder::I2c { scl: 10, sda: 11 });
        assert_eq!(
            logic
                .channels
                .iter()
                .filter(|c| c.enabled)
                .map(|c| c.pin)
                .collect::<Vec<_>>(),
            [10, 11]
        );
        logic.select_channels(Some(true));
        assert!(logic.channels.iter().all(|channel| channel.enabled));
    }

    #[test]
    fn sump_edge_limits_keep_a_verifiable_history_and_supported_pio_rate() {
        let mut logic = Logic::default();
        logic.trigger = 1;
        logic.rate = 50_000_000;
        logic.duration_s = 0.;
        logic.trigger_timeout_ms = 10000;
        logic.update_sampling();
        assert_eq!(logic.rate, 20_000_000);
        assert_eq!(logic.count, 8);
        assert_eq!(logic.trigger_timeout_ms, 5000);
    }
    #[test]
    fn duration_presets_respect_hardware_capacity() {
        let mut logic = Logic::default();
        for rate in SAMPLE_RATES {
            logic.rate = rate;
            logic.duration_s = 100.;
            logic.update_sampling();
            assert_eq!(logic.count, MAX_SAMPLES);
            assert_eq!(logic.duration_s, MAX_SAMPLES as f64 / rate as f64);
            logic.duration_s = 0.001;
            logic.update_sampling();
            assert_eq!(logic.count, (rate as f64 / 1000.).round().max(1.).min(MAX_SAMPLES as f64) as usize);
        }
        logic.rate = 123456;
        logic.duration_s = f64::NAN;
        logic.update_sampling();
        assert_eq!(logic.rate, 2_000_000);
        assert_eq!(logic.count, MAX_SAMPLES);
        assert_eq!(sample_rate_label(100_000), "100 kS/s");
        assert_eq!(sample_rate_label(2_000_000), "2 MS/s");
        assert_eq!(sample_rate_label(1_000_000_000), "1 GS/s");
    }
    #[test]
    fn selected_channels_use_board_memory_and_trigger_byte_storage() {
        let mut logic = Logic::default();
        logic.packed_capture = true;
        logic.select_channels(Some(false));
        logic.channels.iter_mut().find(|c| c.pin == 12).unwrap().enabled = true;
        logic.rate = 500000;
        logic.duration_s = 1000.;
        logic.update_sampling();
        assert_eq!(logic.count, 1310720);
        assert_eq!(logic.duration_s, 2.62144);
        logic.trigger = 1;
        logic.trigger_pin = 12;
        logic.update_sampling();
        assert_eq!(logic.count, 163840);
        logic.trigger = 0;
        logic.channels.iter_mut().find(|c| c.pin == 13).unwrap().enabled = true;
        logic.duration_s = 1000.;
        logic.update_sampling();
        assert_eq!(logic.count, 655360);
        logic.select_channels(Some(true));
        logic.update_sampling();
        assert_eq!(logic.count, 40960);
        logic.rate = 5000;
        logic.duration_s = 1000.;
        logic.update_sampling();
        assert_eq!(logic.duration_s, 8.192);
    }

    #[test]
    fn fixture_decodes() {
        let mut l = Logic::default();
        for decoder in Decoder::catalog().into_iter().take(2) {
            l.add_decoder(decoder);
        }
        l.set_capture(generate(l.rate, l.count), l.rate);
        let uart: Vec<_> = l
            .annotations
            .iter()
            .filter(|a| a.protocol == "UART")
            .collect();
        assert_eq!(uart.len(), 5);
        assert_eq!(uart[0].text, "0x50 'P'");
        assert!(uart.iter().all(|a| !a.error));
        let spi: Vec<_> = l
            .annotations
            .iter()
            .filter(|a| a.protocol == "SPI")
            .collect();
        assert_eq!(spi.len(), 4);
        assert_eq!(spi[1].text, "MOSI 00 / MISO EF");
        assert!(l.vcd().contains("$timescale 1 ns"));
    }
}
