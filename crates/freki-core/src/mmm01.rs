//! MMM01: the mapper of multi-game collections (Momotarou Collection 2, Taito Variety
//! Pack). Its registers follow SameBoy's model.
//!
//! The menu sits in the last 32 KiB of the ROM, and at power-on that is what
//! $0000–$7FFF shows ("unmapped"). The menu writes the chosen game's place in the ROM to
//! the registers, then sets the map-enable bit. From then on the cartridge acts like an
//! MBC1 shut inside that game's slice: the upper bank bits, and any low bits the menu
//! masked, can no longer change.
//!
//! | Range | Bits |
//! |---|---|
//! | $0000–$1FFF | 0–3 RAM enable ($A); 4–5 RAM bank mask*; 6 map enable* |
//! | $2000–$3FFF | 0–4 ROM bank low (masked bits frozen); 5–6 ROM bank mid* |
//! | $4000–$5FFF | 0–1 RAM bank low; 2–3 RAM bank high*; 4–5 ROM bank high*; 6 MBC1-mode write protect* |
//! | $6000–$7FFF | 0 MBC1 mode (unless protected); 2–5 ROM bank mask*; 6 multiplex* |
//!
//! \* only before mapping. In multiplex mode the RAM bank low and ROM bank mid bits swap
//! roles, as on an MBC1, where $4000 picks ROM bank bits 5–6.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Mmm01 {
    pub ram_enabled: bool,
    /// Mapped: the menu is done and the game's slice is locked in.
    locked: bool,
    rom_low: u8,
    rom_mid: u8,
    rom_high: u8,
    ram_low: u8,
    ram_high: u8,
    /// Bits 1–4 of `rom_low` that the game cannot change.
    rom_mask: u8,
    /// Bits of `ram_low` that the game can change.
    ram_mask: u8,
    mbc1_mode: bool,
    mbc1_mode_protected: bool,
    multiplex: bool,
}

impl Default for Mmm01 {
    fn default() -> Self {
        Self {
            ram_enabled: false,
            locked: false,
            rom_low: 0,
            rom_mid: 0,
            rom_high: 0,
            ram_low: 0,
            ram_high: 0,
            rom_mask: 0,
            ram_mask: 0x03,
            mbc1_mode: false,
            mbc1_mode_protected: false,
            multiplex: false,
        }
    }
}

impl Mmm01 {
    /// A write to $0000–$7FFF.
    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => {
                self.ram_enabled = value & 0x0F == 0x0A;
                if !self.locked {
                    self.ram_mask = value >> 4 & 0x03;
                    self.locked = value & 0x40 != 0;
                }
            }
            0x2000..=0x3FFF => {
                if !self.locked {
                    self.rom_mid = value >> 5 & 0x03;
                }
                let frozen = self.rom_mask << 1;
                self.rom_low = (self.rom_low & frozen | value & !frozen) & 0x1F;
            }
            0x4000..=0x5FFF => {
                self.ram_low = (value | !self.ram_mask) & 0x03;
                if !self.locked {
                    self.ram_high = value >> 2 & 0x03;
                    self.rom_high = value >> 4 & 0x03;
                    self.mbc1_mode_protected = value & 0x40 != 0;
                }
            }
            _ => {
                if !self.mbc1_mode_protected {
                    self.mbc1_mode = value & 0x01 != 0;
                }
                if !self.locked {
                    self.rom_mask = value >> 2 & 0x0F;
                    self.multiplex = value & 0x40 != 0;
                }
            }
        }
    }

    /// The ROM banks at $0000–$3FFF and $4000–$7FFF, for a ROM of `banks` 16 KiB banks.
    pub fn rom_banks(&self, banks: usize) -> (usize, usize) {
        if !self.locked {
            // The menu: the last 32 KiB.
            return (banks.saturating_sub(2), banks.saturating_sub(1));
        }
        let (mid0, mid) = if self.multiplex {
            (if self.mbc1_mode { 0 } else { self.ram_low }, self.ram_low)
        } else {
            (self.rom_mid, self.rom_mid)
        };
        let high = usize::from(self.rom_high) << 7;
        let bank0 = usize::from(self.rom_low & self.rom_mask << 1) | usize::from(mid0) << 5 | high;
        let mut bank = usize::from(self.rom_low) | usize::from(mid) << 5 | high;
        if bank == bank0 {
            bank += 1;
        }
        (bank0, bank)
    }

    /// The RAM bank at $A000–$BFFF.
    pub fn ram_bank(&self) -> usize {
        if !self.locked {
            return 0;
        }
        let low = if self.multiplex {
            self.rom_mid
        } else {
            self.ram_low
        };
        usize::from(low | self.ram_high << 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_until_mapped_then_locked_to_the_game() {
        let mut m = Mmm01::default();
        assert_eq!(m.rom_banks(64), (62, 63), "menu in the last 32 KiB");
        // The menu picks the game at banks $20–$2F: ROM bank mid = 1, bits 1-3 of the
        // low bank free (mask freezes bit 4), then maps.
        m.write(0x2000, 0x20);
        m.write(0x6000, 0b1000 << 2);
        m.write(0x0000, 0x40);
        assert_eq!(m.rom_banks(64), (0x20, 0x21), "bank 0 → 1, as on MBC1");
        // The game can switch within its slice but not out of it.
        m.write(0x2000, 0x05);
        assert_eq!(m.rom_banks(64), (0x20, 0x25));
        m.write(0x2000, 0x7F);
        assert_eq!(m.rom_banks(64), (0x20, 0x2F), "bit 4 and the mid bits stay");
        m.write(0x4000, 0x30);
        assert_eq!(m.rom_banks(64), (0x20, 0x2F), "high bits locked too");
    }

    #[test]
    fn ram_banks_and_enable() {
        let mut m = Mmm01::default();
        m.write(0x4000, 0b0100); // RAM bank high = 1
        m.write(0x0000, 0x4A); // map, RAM on, no RAM bank bits free
        assert!(m.ram_enabled);
        m.write(0x4000, 0x02);
        assert_eq!(m.ram_bank(), 0b111, "low bits not in the mask read as 1");
        m.write(0x0000, 0x00);
        assert!(!m.ram_enabled);
    }
}
