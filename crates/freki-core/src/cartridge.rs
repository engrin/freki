//! Cartridge loading, header parsing and memory bank controllers.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mbc {
    RomOnly,
    // TODO: Mbc1, Mbc3, Mbc5 — needed for most commercial games.
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CartridgeError {
    TooSmall(usize),
    UnsupportedType(u8),
}

impl fmt::Display for CartridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall(len) => {
                write!(f, "ROM is too small ({len} bytes, need at least 32 KiB)")
            }
            Self::UnsupportedType(t) => write!(f, "unsupported cartridge type ${t:02X}"),
        }
    }
}

impl std::error::Error for CartridgeError {}

pub struct Cartridge {
    rom: Vec<u8>,
    ram: Vec<u8>,
    mbc: Mbc,
}

impl Cartridge {
    pub fn from_rom(rom: Vec<u8>) -> Result<Self, CartridgeError> {
        if rom.len() < 0x8000 {
            return Err(CartridgeError::TooSmall(rom.len()));
        }
        let mbc = match rom[0x147] {
            0x00 => Mbc::RomOnly,
            other => return Err(CartridgeError::UnsupportedType(other)),
        };
        let ram_size = match rom[0x149] {
            0x02 => 8 * 1024,
            0x03 => 32 * 1024,
            0x04 => 128 * 1024,
            0x05 => 64 * 1024,
            _ => 0,
        };
        Ok(Self {
            rom,
            ram: vec![0; ram_size],
            mbc,
        })
    }

    /// Game title from the header ($0134–$0143), trimmed at the first NUL.
    pub fn title(&self) -> String {
        let raw = &self.rom[0x134..0x144];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        String::from_utf8_lossy(&raw[..end]).trim().to_string()
    }

    pub fn mbc(&self) -> Mbc {
        self.mbc
    }

    /// Read from $0000–$7FFF.
    pub fn read_rom(&self, addr: u16) -> u8 {
        match self.mbc {
            Mbc::RomOnly => self.rom.get(addr as usize).copied().unwrap_or(0xFF),
        }
    }

    /// Write to $0000–$7FFF (MBC control registers).
    pub fn write_rom(&mut self, _addr: u16, _value: u8) {
        match self.mbc {
            Mbc::RomOnly => {}
        }
    }

    /// Read from external RAM at $A000–$BFFF.
    pub fn read_ram(&self, addr: u16) -> u8 {
        self.ram
            .get((addr - 0xA000) as usize)
            .copied()
            .unwrap_or(0xFF)
    }

    /// Write to external RAM at $A000–$BFFF.
    pub fn write_ram(&mut self, addr: u16, value: u8) {
        if let Some(byte) = self.ram.get_mut((addr - 0xA000) as usize) {
            *byte = value;
        }
    }
}
