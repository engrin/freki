//! Optional boot ROM support.
//!
//! By default freki starts games in the state the DMG boot ROM leaves behind. With a
//! [`BootRom`] (a dump of the 256-byte DMG boot ROM, which is not included), the machine
//! starts at power-on instead: the boot ROM is mapped over $0000–$00FF and runs the
//! scrolling logo and sound until it writes $FF50, which unmaps it for good.

use std::fmt;

/// Size of the DMG boot ROM.
pub const BOOT_ROM_SIZE: usize = 0x100;

/// A DMG boot ROM image.
#[derive(Clone, PartialEq, Eq)]
pub struct BootRom(Box<[u8; BOOT_ROM_SIZE]>);

impl BootRom {
    /// Checks that `bytes` is a DMG boot ROM (256 bytes).
    pub fn new(bytes: &[u8]) -> Result<Self, BootRomError> {
        let image: [u8; BOOT_ROM_SIZE] = bytes
            .try_into()
            .map_err(|_| BootRomError::WrongSize(bytes.len()))?;
        Ok(Self(Box::new(image)))
    }

    pub fn read(&self, addr: u16) -> u8 {
        self.0[addr as usize & (BOOT_ROM_SIZE - 1)]
    }
}

impl fmt::Debug for BootRom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BootRom")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootRomError {
    WrongSize(usize),
}

impl fmt::Display for BootRomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongSize(len) => write!(
                f,
                "a DMG boot ROM is {BOOT_ROM_SIZE} bytes, this file has {len} \
                 (Game Boy Color boot ROMs are not supported yet)"
            ),
        }
    }
}

impl std::error::Error for BootRomError {}
