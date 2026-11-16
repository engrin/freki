//! Optional boot ROM support.
//!
//! By default freki starts games in the state the boot ROM leaves behind. With a
//! [`BootRom`] (a dump of the console's boot ROM, which is not included), the machine
//! starts at power-on instead: the boot ROM is mapped over the start of the cartridge ROM
//! and runs the logo animation and sound until it writes $FF50, which unmaps it for good.
//!
//! - The DMG boot ROM is 256 bytes, mapped at $0000–$00FF.
//! - The Game Boy Color boot ROM is 2304 bytes, mapped at $0000–$00FF and $0200–$08FF
//!   (the cartridge header at $0100–$01FF stays visible). For original Game Boy games it
//!   picks a colour palette and switches the CGB into its DMG compatibility mode.

use std::fmt;

/// Size of the DMG boot ROM.
pub const DMG_BOOT_ROM_SIZE: usize = 0x100;
/// Size of the CGB boot ROM (including the unused $0100–$01FF gap).
pub const CGB_BOOT_ROM_SIZE: usize = 0x900;

/// A DMG or CGB boot ROM image.
#[derive(Clone, PartialEq, Eq)]
pub struct BootRom(Box<[u8]>);

impl BootRom {
    /// Checks that `bytes` is a DMG (256 bytes) or CGB (2304 bytes) boot ROM.
    pub fn new(bytes: &[u8]) -> Result<Self, BootRomError> {
        match bytes.len() {
            DMG_BOOT_ROM_SIZE | CGB_BOOT_ROM_SIZE => Ok(Self(bytes.into())),
            len => Err(BootRomError::WrongSize(len)),
        }
    }

    /// Whether this is the Game Boy Color boot ROM.
    pub fn is_cgb(&self) -> bool {
        self.0.len() == CGB_BOOT_ROM_SIZE
    }

    /// The boot ROM byte at `addr`, if the boot ROM covers that address.
    pub fn read(&self, addr: u16) -> Option<u8> {
        match addr {
            0x0000..=0x00FF => Some(self.0[addr as usize]),
            0x0200..=0x08FF if self.is_cgb() => Some(self.0[addr as usize]),
            _ => None,
        }
    }
}

impl fmt::Debug for BootRom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_cgb() {
            "BootRom(CGB)"
        } else {
            "BootRom(DMG)"
        })
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
                "a boot ROM is {DMG_BOOT_ROM_SIZE} bytes (Game Boy) or {CGB_BOOT_ROM_SIZE} \
                 bytes (Game Boy Color); this file has {len}"
            ),
        }
    }
}

impl std::error::Error for BootRomError {}
