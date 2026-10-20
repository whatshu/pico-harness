#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferRequest {
    Read { ap: bool, address: u8 },
    Write { ap: bool, address: u8, value: u32 },
}

impl TransferRequest {
    pub const fn dp_read(address: u8) -> Self {
        Self::Read { ap: false, address }
    }

    pub const fn dp_write(address: u8, value: u32) -> Self {
        Self::Write {
            ap: false,
            address,
            value,
        }
    }

    pub const fn ap_read(address: u8) -> Self {
        Self::Read { ap: true, address }
    }

    pub const fn ap_write(address: u8, value: u32) -> Self {
        Self::Write {
            ap: true,
            address,
            value,
        }
    }

    pub fn request_byte(self) -> u8 {
        let (ap, address, read) = match self {
            Self::Read { ap, address } => (ap, address, true),
            Self::Write { ap, address, .. } => (ap, address, false),
        };
        (if ap { 1 } else { 0 }) | (if read { 2 } else { 0 }) | (address & 0x0c)
    }
}
