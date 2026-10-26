//! Board-resident capture capacity. No streaming or signal-dependent compression.
pub const BUFFER_BYTES: u32 = 160 * 1024;
pub const LEGACY_SAMPLES: u32 = 32768;
pub const PINS: [(u8, u8); 16] = [
    (3, 28), (6, 27), (7, 26), (27, 5), (12, 17), (13, 16), (18, 11), (19, 10),
    (16, 15), (17, 14), (20, 13), (21, 12), (8, 19), (9, 21), (10, 18), (11, 20),
];
pub const RATES: [u32; 14] = [
    2500, 5000, 10000, 20000, 50000, 100000, 250000, 500000,
    1000000, 2000000, 5000000, 10000000, 20000000, 50000000,
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    pub mask: u32,
    pub pin_base: u8,
    pub input_bits: u8,
    pub storage_bits: u8,
    pub max_samples: u32,
    pub quantum: u32,
}
/// A zero mask keeps the legacy all-channel selection. Include the trigger pin
/// even when its waveform is hidden, so the trigger remains observable.
pub fn profile(mut mask: u32, trigger: u8, pin: u8) -> Option<Profile> {
    let supported = PINS.iter().fold(0, |mask, (_, p)| mask | (1u32 << p));
    if mask == 0 { mask = supported; }
    if trigger != 0 { mask |= 1u32.checked_shl(pin as u32)?; }
    if mask & !supported != 0 { return None; }
    let mut raw = 0u32;
    for (debug, work) in PINS { if mask & (1 << work) != 0 { raw |= 1 << debug; } }
    let base = raw.trailing_zeros();
    let span = 32 - raw.leading_zeros() - base;
    let bits = span.next_power_of_two();
    let storage_bits = if trigger == 0 { bits } else { bits.max(8) };
    Some(Profile {
        mask, pin_base: if bits == 32 { 0 } else { base as u8 },
        input_bits: bits as u8, storage_bits: storage_bits as u8,
        max_samples: BUFFER_BYTES * 8 / storage_bits,
        quantum: if trigger == 0 { 32 / bits } else { 4 },
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_bound_depends_on_pin_span_and_trigger_dma_width() {
        let all = profile(0, 0, 0).unwrap();
        assert_eq!((all.input_bits, all.max_samples), (32, 40960));
        let uart = profile((1 << 12) | (1 << 13), 0, 0).unwrap();
        assert_eq!((uart.input_bits, uart.max_samples), (2, 655360));
        let one = profile(1 << 12, 0, 0).unwrap();
        assert_eq!((one.input_bits, one.max_samples), (1, 1310720));
        let edge = profile(1 << 12, 1, 12).unwrap();
        assert_eq!((edge.storage_bits, edge.max_samples), (8, 163840));
        assert_eq!(profile(1 << 5, 1, 12).unwrap().input_bits, 8);
        assert!(profile(1 << 25, 0, 0).is_none());
        assert!(profile(1 << 12, 1, 99).is_none());
    }
}
