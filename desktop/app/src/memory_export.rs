//! Memory range validation and portable export formats.
pub fn parse_address(text: &str) -> Result<u32, std::num::ParseIntError> {
    let text = text.trim();
    u32::from_str_radix(
        text.strip_prefix("0x")
            .or_else(|| text.strip_prefix("0X"))
            .unwrap_or(text),
        16,
    )
}

/// Clamp inspection to the target's contiguous SRAM and the 32-bit address space.
pub fn read_length(target: &str, address: u32, requested: usize) -> usize {
    let ram_end = match target {
        "rp2040" => 0x2004_2000,
        "rp2350" => 0x2008_2000,
        _ => 0,
    };
    let end = if (0x2000_0000..ram_end).contains(&address) {
        ram_end as u64
    } else {
        0x1_0000_0000
    };
    requested.min((end - address as u64) as usize)
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Binary,
    IntelHex,
    Csv,
}
impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Self::Binary => "Binary",
            Self::IntelHex => "Intel HEX",
            Self::Csv => "CSV",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::Binary => "bin",
            Self::IntelHex => "hex",
            Self::Csv => "csv",
        }
    }
    pub fn encode(self, address: u32, bytes: &[u8]) -> Vec<u8> {
        match self {
            Self::Binary => bytes.to_vec(),
            Self::Csv => {
                let mut text = String::from("address,byte_hex\n");
                for (offset, byte) in bytes.iter().enumerate() {
                    text.push_str(&format!(
                        "0x{:08X},{byte:02X}\n",
                        address as u64 + offset as u64
                    ));
                }
                text.into_bytes()
            }
            Self::IntelHex => {
                let mut text = String::new();
                let mut offset = 0;
                let mut upper = None;
                while offset < bytes.len() {
                    let current = address as u64 + offset as u64;
                    let high = (current >> 16) as u16;
                    if upper != Some(high) {
                        record(&mut text, 0, 4, &high.to_be_bytes());
                        upper = Some(high);
                    }
                    let low = current as u16;
                    let count = (bytes.len() - offset).min(16).min(0x10000 - low as usize);
                    record(&mut text, low, 0, &bytes[offset..offset + count]);
                    offset += count;
                }
                record(&mut text, 0, 1, &[]);
                text.into_bytes()
            }
        }
    }
}
fn record(text: &mut String, address: u16, kind: u8, bytes: &[u8]) {
    let mut sum = bytes.len() as u8;
    sum = sum
        .wrapping_add((address >> 8) as u8)
        .wrapping_add(address as u8)
        .wrapping_add(kind);
    text.push_str(&format!(":{:02X}{address:04X}{kind:02X}", bytes.len()));
    for byte in bytes {
        text.push_str(&format!("{byte:02X}"));
        sum = sum.wrapping_add(*byte);
    }
    text.push_str(&format!("{:02X}\n", sum.wrapping_neg()));
}
pub fn validate_range(address: u32, length: usize) -> Result<(), String> {
    if !(1..=4 * 1024 * 1024).contains(&length) || address as u64 + length as u64 > 0x1_0000_0000 {
        return Err(
            "Memory range must contain 1 byte to 4 MiB and fit the 32-bit address space".into(),
        );
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspection_addresses_are_hex_and_reads_stop_at_sram_end() {
        for text in ["20041FD0", "0x20041FD0", " 0X20041fd0 "] {
            assert_eq!(parse_address(text).unwrap(), 0x20041FD0);
        }
        assert_eq!(parse_address("20000000").unwrap(), 0x20000000);
        assert!(parse_address("0xGG").is_err());
        assert_eq!(read_length("rp2040", 0x20041FD0, 256), 48);
        assert_eq!(read_length("rp2040", 0x20041FD0, 1), 1);
        assert_eq!(read_length("rp2350", 0x20081FFD, 256), 3);
        assert_eq!(read_length("rp2040", 0x10000000, 256), 256);
        assert_eq!(read_length("rp2040", u32::MAX, 256), 1);
    }
    #[test]
    fn hex_crosses_address_boundary_and_has_valid_checksums() {
        let text = String::from_utf8(Format::IntelHex.encode(0x2000fffe, &[1, 2, 3, 4])).unwrap();
        assert_eq!(
            text,
            ":020000042000DA\n:02FFFE000102FE\n:020000042001D9\n:020000000304F7\n:00000001FF\n"
        );
        for line in text.lines() {
            let sum = (1..line.len()).step_by(2).fold(0u8, |sum, i| {
                sum.wrapping_add(u8::from_str_radix(&line[i..i + 2], 16).unwrap())
            });
            assert_eq!(sum, 0);
        }
        assert!(validate_range(u32::MAX, 2).is_err());
        assert!(validate_range(u32::MAX, 1).is_ok());
    }
}
