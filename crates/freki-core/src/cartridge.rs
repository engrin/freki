//! Cartridge loading, header parsing and memory bank controllers (MBCs).
//!
//! Supported: no MBC, MBC1, MBC2, MBC3 (including the real-time clock) and MBC5.
//! Together these cover the vast majority of DMG games.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::CLOCK_HZ;

const ROM_BANK: usize = 0x4000;
const RAM_BANK: usize = 0x2000;
/// MBC2 has 512 half-bytes of RAM built in.
const MBC2_RAM: usize = 512;

/// What an infrared port reads when it sees no light (no other Game Boy is sending).
const IR_DARK: u8 = 0xC0;

/// Memory bank controller and its register state.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Mbc {
    /// No MBC: 32 KiB ROM, optionally up to 8 KiB RAM.
    RomOnly,
    /// MBC1: up to 2 MiB ROM and 32 KiB RAM.
    Mbc1 {
        ram_enabled: bool,
        /// BANK1: 5-bit ROM bank register ($2000–$3FFF); 0 reads as 1.
        bank1: u8,
        /// BANK2: 2-bit register ($4000–$5FFF): upper ROM bank bits or RAM bank.
        bank2: u8,
        /// Banking mode ($6000–$7FFF): when set, BANK2 also applies to $0000–$3FFF and RAM.
        advanced: bool,
        /// MBC1M multicart wiring: BANK2 selects ROM bits 4–5 and BANK1 bit 4 is unused.
        multicart: bool,
    },
    /// MBC2: up to 256 KiB ROM and 512×4 bits of built-in RAM.
    Mbc2 { ram_enabled: bool, rom_bank: u8 },
    /// MBC3: up to 2 MiB ROM, 32 KiB RAM and an optional real-time clock.
    Mbc3 {
        ram_enabled: bool,
        rom_bank: u8,
        /// $00–$03 selects a RAM bank, $08–$0C an RTC register.
        ram_select: u8,
        rtc: Option<Rtc>,
    },
    /// MBC5: up to 8 MiB ROM and 128 KiB RAM.
    Mbc5 {
        ram_enabled: bool,
        /// 9-bit ROM bank; unlike MBC1/3, bank 0 can be mapped to $4000–$7FFF.
        rom_bank: u16,
        ram_bank: u8,
        /// Rumble cartridge (types $1C–$1E): bit 3 of the RAM bank register drives the
        /// motor instead of selecting RAM.
        rumble: bool,
        motor: bool,
    },
    /// Hudson HuC1: like MBC1 with up to 32 KiB RAM, plus an infrared port in place of RAM.
    HuC1 {
        /// $0000–$1FFF = $0E maps the infrared port to $A000–$BFFF instead of RAM.
        ir_mode: bool,
        rom_bank: u8,
        ram_bank: u8,
    },
}

/// MBC3 real-time clock. It runs on emulated time: one second per [`CLOCK_HZ`] T-cycles.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct Rtc {
    /// Live registers: seconds, minutes, hours, day low, day high.
    regs: [u8; 5],
    /// Copy of `regs` taken by the last latch; this is what the game reads.
    latched: [u8; 5],
    /// The latch triggers on a $00 → $01 write sequence.
    latch_armed: bool,
    cycles: u32,
}

impl Rtc {
    const MASKS: [u8; 5] = [0x3F, 0x3F, 0x1F, 0xFF, 0xC1];
    const DAY_HIGH: usize = 4;
    const HALT: u8 = 0x40;
    const DAY_CARRY: u8 = 0x80;

    fn halted(&self) -> bool {
        self.regs[Self::DAY_HIGH] & Self::HALT != 0
    }

    fn tick(&mut self, cycles: u32) {
        if self.halted() {
            return;
        }
        self.cycles += cycles;
        while self.cycles >= CLOCK_HZ {
            self.cycles -= CLOCK_HZ;
            self.advance_second();
        }
    }

    fn advance_second(&mut self) {
        let [s, m, h, dl, dh] = &mut self.regs;
        *s = (*s + 1) & 0x3F;
        if *s != 60 {
            return;
        }
        *s = 0;
        *m = (*m + 1) & 0x3F;
        if *m != 60 {
            return;
        }
        *m = 0;
        *h = (*h + 1) & 0x1F;
        if *h != 24 {
            return;
        }
        *h = 0;
        let mut day = ((*dh as u16 & 1) << 8 | *dl as u16) + 1;
        if day == 512 {
            day = 0;
            *dh |= Self::DAY_CARRY;
        }
        *dl = day as u8;
        *dh = (*dh & !1) | (day >> 8) as u8;
    }

    fn write_latch(&mut self, value: u8) {
        if self.latch_armed && value == 1 {
            self.latched = self.regs;
        }
        self.latch_armed = value == 0;
    }

    fn read(&self, reg: u8) -> u8 {
        self.latched[(reg - 0x08) as usize]
    }

    fn write(&mut self, reg: u8, value: u8) {
        let i = (reg - 0x08) as usize;
        if i == 0 {
            self.cycles = 0; // writing seconds resets the sub-second divider
        }
        self.regs[i] = value & Self::MASKS[i];
    }
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

#[derive(Serialize, Deserialize)]
pub struct Cartridge {
    /// Not part of save states: restored from the loaded ROM.
    #[serde(skip)]
    rom: Vec<u8>,
    ram: Vec<u8>,
    mbc: Mbc,
    battery: bool,
    /// Set whenever RAM is written; lets the frontend save only when needed.
    ram_dirty: bool,
    /// T-cycles the rumble motor was on, and T-cycles in all, since the frontend last
    /// asked (see [`Cartridge::take_rumble`]).
    #[serde(skip)]
    motor_cycles: (u32, u32),
}

/// Detects an MBC1M multicart (several games on one 8 Mbit MBC1 cartridge): a second
/// Nintendo logo in the header of the game at bank $10.
fn is_mbc1_multicart(rom: &[u8]) -> bool {
    const LOGO: std::ops::Range<usize> = 0x104..0x134;
    rom.len() == 0x10_0000 && rom[LOGO] == rom[0x40000 + LOGO.start..0x40000 + LOGO.end]
}

impl Cartridge {
    pub fn from_rom(rom: Vec<u8>) -> Result<Self, CartridgeError> {
        if rom.len() < 0x8000 {
            return Err(CartridgeError::TooSmall(rom.len()));
        }
        let kind = rom[0x147];
        let mbc = match kind {
            0x00 | 0x08 | 0x09 => Mbc::RomOnly,
            0x01..=0x03 => Mbc::Mbc1 {
                ram_enabled: false,
                bank1: 1,
                bank2: 0,
                advanced: false,
                multicart: is_mbc1_multicart(&rom),
            },
            0x05 | 0x06 => Mbc::Mbc2 {
                ram_enabled: false,
                rom_bank: 1,
            },
            0x0F..=0x13 => Mbc::Mbc3 {
                ram_enabled: false,
                rom_bank: 1,
                ram_select: 0,
                rtc: matches!(kind, 0x0F | 0x10).then(Rtc::default),
            },
            0x19..=0x1E => Mbc::Mbc5 {
                ram_enabled: false,
                rom_bank: 1,
                ram_bank: 0,
                rumble: kind >= 0x1C,
                motor: false,
            },
            0xFF => Mbc::HuC1 {
                ir_mode: false,
                rom_bank: 1,
                ram_bank: 0,
            },
            other => return Err(CartridgeError::UnsupportedType(other)),
        };
        let battery = matches!(
            kind,
            0x03 | 0x06 | 0x09 | 0x0F | 0x10 | 0x13 | 0x1B | 0x1E | 0xFF
        );
        let ram_size = match (&mbc, rom[0x149]) {
            (Mbc::Mbc2 { .. }, _) => MBC2_RAM,
            (_, 0x02) => 8 * 1024,
            (_, 0x03) => 32 * 1024,
            (_, 0x04) => 128 * 1024,
            (_, 0x05) => 64 * 1024,
            _ => 0,
        };
        Ok(Self {
            rom,
            ram: vec![0; ram_size],
            mbc,
            battery,
            ram_dirty: false,
            motor_cycles: (0, 0),
        })
    }

    /// Moves the ROM over from `old` (save states do not contain it). Battery RAM restored
    /// from a state is marked dirty so the save file is brought in line with it.
    pub(crate) fn adopt_rom_from(&mut self, old: &mut Cartridge) {
        self.rom = std::mem::take(&mut old.rom);
        self.ram_dirty = self.battery;
    }

    /// Game title from the header ($0134–$0143), trimmed at the first NUL. On CGB games
    /// $0143 is the CGB flag, not part of the title.
    pub fn title(&self) -> String {
        let end = if self.supports_cgb() { 0x143 } else { 0x144 };
        let raw = &self.rom[0x134..end];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        String::from_utf8_lossy(&raw[..end]).trim().to_string()
    }

    /// Whether the header ($0143 bit 7) says the game uses Game Boy Color features.
    pub fn supports_cgb(&self) -> bool {
        self.rom[0x143] & 0x80 != 0
    }

    /// Whether the game runs only on a Game Boy Color ($0143 = $C0).
    pub fn cgb_only(&self) -> bool {
        self.rom[0x143] == 0xC0
    }

    pub fn mbc(&self) -> &Mbc {
        &self.mbc
    }

    /// Whether the cartridge keeps its RAM when switched off, i.e. needs a save file.
    pub fn has_battery(&self) -> bool {
        self.battery && !self.ram.is_empty()
    }

    /// External RAM contents, for writing a save file.
    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    /// Restores external RAM from a save file. Extra bytes are ignored, missing ones stay 0.
    pub fn load_ram(&mut self, data: &[u8]) {
        let n = data.len().min(self.ram.len());
        self.ram[..n].copy_from_slice(&data[..n]);
        self.ram_dirty = false;
    }

    /// Returns `true` if RAM changed since the last call.
    pub fn take_ram_dirty(&mut self) -> bool {
        std::mem::take(&mut self.ram_dirty)
    }

    /// Advances time-based hardware (the MBC3 clock) by `cycles` T-cycles.
    pub fn tick(&mut self, cycles: u32) {
        match &mut self.mbc {
            Mbc::Mbc3 { rtc: Some(rtc), .. } => rtc.tick(cycles),
            Mbc::Mbc5 {
                rumble: true,
                motor,
                ..
            } => {
                let (on, all) = &mut self.motor_cycles;
                *on = on.saturating_add(if *motor { cycles } else { 0 });
                *all = all.saturating_add(cycles);
            }
            _ => {}
        }
    }

    /// How strongly a rumble cartridge's motor ran since the last call: the fraction of
    /// time it was on (games pulse it to set the strength), 0 to 1. `None` if no time
    /// has passed or the cartridge has no motor.
    pub fn take_rumble(&mut self) -> Option<f32> {
        let (on, all) = std::mem::take(&mut self.motor_cycles);
        (all > 0).then_some(on as f32 / all as f32)
    }

    fn rom_byte(&self, bank: usize, addr: u16) -> u8 {
        let banks = (self.rom.len() / ROM_BANK).max(1);
        let offset = (bank % banks) * ROM_BANK + (addr as usize & (ROM_BANK - 1));
        self.rom.get(offset).copied().unwrap_or(0xFF)
    }

    /// Byte offset into `ram` for an $A000–$BFFF address, or `None` if RAM is not accessible.
    fn ram_offset(&self, addr: u16) -> Option<usize> {
        if self.ram.is_empty() {
            return None;
        }
        let local = (addr - 0xA000) as usize;
        let offset = match self.mbc {
            Mbc::RomOnly => local,
            Mbc::Mbc1 {
                ram_enabled,
                bank2,
                advanced,
                ..
            } => {
                if !ram_enabled {
                    return None;
                }
                let bank = if advanced { bank2 as usize } else { 0 };
                bank * RAM_BANK + local
            }
            Mbc::Mbc2 { ram_enabled, .. } => {
                if !ram_enabled {
                    return None;
                }
                local % MBC2_RAM // 512 bytes, mirrored across $A000–$BFFF
            }
            Mbc::Mbc3 {
                ram_enabled,
                ram_select,
                ..
            } => {
                if !ram_enabled || ram_select > 0x03 {
                    return None;
                }
                ram_select as usize * RAM_BANK + local
            }
            Mbc::Mbc5 {
                ram_enabled,
                ram_bank,
                ..
            } => {
                if !ram_enabled {
                    return None;
                }
                ram_bank as usize * RAM_BANK + local
            }
            Mbc::HuC1 {
                ir_mode, ram_bank, ..
            } => {
                if ir_mode {
                    return None;
                }
                ram_bank as usize * RAM_BANK + local
            }
        };
        Some(offset % self.ram.len())
    }

    /// Read from $0000–$7FFF.
    pub fn read_rom(&self, addr: u16) -> u8 {
        let low = addr < 0x4000;
        let bank = match self.mbc {
            Mbc::RomOnly => return self.rom.get(addr as usize).copied().unwrap_or(0xFF),
            Mbc::Mbc1 {
                bank1,
                bank2,
                advanced,
                multicart,
                ..
            } => {
                let (shift, bank1) = if multicart {
                    (4, bank1 & 0x0F)
                } else {
                    (5, bank1)
                };
                match (low, advanced) {
                    (true, false) => 0,
                    (true, true) => (bank2 as usize) << shift,
                    (false, _) => ((bank2 as usize) << shift) | bank1 as usize,
                }
            }
            Mbc::Mbc2 { rom_bank, .. } => {
                if low {
                    0
                } else {
                    rom_bank as usize
                }
            }
            Mbc::Mbc3 { rom_bank, .. } => {
                if low {
                    0
                } else {
                    rom_bank as usize
                }
            }
            Mbc::Mbc5 { rom_bank, .. } => {
                if low {
                    0
                } else {
                    rom_bank as usize
                }
            }
            Mbc::HuC1 { rom_bank, .. } => {
                if low {
                    0
                } else {
                    rom_bank as usize
                }
            }
        };
        self.rom_byte(bank, addr)
    }

    /// Write to $0000–$7FFF (MBC control registers).
    pub fn write_rom(&mut self, addr: u16, value: u8) {
        match &mut self.mbc {
            Mbc::RomOnly => {}
            Mbc::Mbc1 {
                ram_enabled,
                bank1,
                bank2,
                advanced,
                ..
            } => match addr {
                0x0000..=0x1FFF => *ram_enabled = value & 0x0F == 0x0A,
                0x2000..=0x3FFF => *bank1 = (value & 0x1F).max(1),
                0x4000..=0x5FFF => *bank2 = value & 0x03,
                _ => *advanced = value & 0x01 != 0,
            },
            Mbc::Mbc2 {
                ram_enabled,
                rom_bank,
            } => {
                // Only $0000–$3FFF is used; address bit 8 picks the register.
                if addr < 0x4000 {
                    if addr & 0x0100 == 0 {
                        *ram_enabled = value & 0x0F == 0x0A;
                    } else {
                        *rom_bank = (value & 0x0F).max(1);
                    }
                }
            }
            Mbc::Mbc3 {
                ram_enabled,
                rom_bank,
                ram_select,
                rtc,
            } => match addr {
                0x0000..=0x1FFF => *ram_enabled = value & 0x0F == 0x0A,
                0x2000..=0x3FFF => *rom_bank = (value & 0x7F).max(1),
                0x4000..=0x5FFF => *ram_select = value & 0x0F,
                _ => {
                    if let Some(rtc) = rtc {
                        rtc.write_latch(value);
                    }
                }
            },
            Mbc::Mbc5 {
                ram_enabled,
                rom_bank,
                ram_bank,
                rumble,
                motor,
            } => match addr {
                0x0000..=0x1FFF => *ram_enabled = value & 0x0F == 0x0A,
                0x2000..=0x2FFF => *rom_bank = (*rom_bank & 0x100) | value as u16,
                0x3000..=0x3FFF => *rom_bank = (*rom_bank & 0xFF) | ((value as u16 & 1) << 8),
                0x4000..=0x5FFF if *rumble => {
                    *ram_bank = value & 0x07;
                    *motor = value & 0x08 != 0;
                }
                0x4000..=0x5FFF => *ram_bank = value & 0x0F,
                _ => {}
            },
            Mbc::HuC1 {
                ir_mode,
                rom_bank,
                ram_bank,
            } => match addr {
                0x0000..=0x1FFF => *ir_mode = value & 0x0F == 0x0E,
                0x2000..=0x3FFF => *rom_bank = (value & 0x3F).max(1),
                0x4000..=0x5FFF => *ram_bank = value & 0x03,
                _ => {}
            },
        }
    }

    /// The selected MBC3 RTC register ($08–$0C), if one is mapped to $A000–$BFFF.
    fn rtc_register(&mut self) -> Option<(&mut Rtc, u8)> {
        match &mut self.mbc {
            Mbc::Mbc3 {
                ram_enabled: true,
                ram_select: reg @ 0x08..=0x0C,
                rtc: Some(rtc),
                ..
            } => Some((rtc, *reg)),
            _ => None,
        }
    }

    /// Read from external RAM at $A000–$BFFF.
    pub fn read_ram(&self, addr: u16) -> u8 {
        if let Mbc::Mbc3 {
            ram_enabled: true,
            ram_select: reg @ 0x08..=0x0C,
            rtc: Some(rtc),
            ..
        } = &self.mbc
        {
            return rtc.read(*reg);
        }
        if let Mbc::HuC1 { ir_mode: true, .. } = self.mbc {
            return IR_DARK;
        }
        let Some(i) = self.ram_offset(addr) else {
            return 0xFF;
        };
        match self.mbc {
            Mbc::Mbc2 { .. } => self.ram[i] | 0xF0, // only the low nibble exists
            _ => self.ram[i],
        }
    }

    /// Write to external RAM at $A000–$BFFF.
    pub fn write_ram(&mut self, addr: u16, value: u8) {
        if let Some((rtc, reg)) = self.rtc_register() {
            rtc.write(reg, value);
            return;
        }
        if let Some(i) = self.ram_offset(addr) {
            let value = match self.mbc {
                Mbc::Mbc2 { .. } => value & 0x0F,
                _ => value,
            };
            self.ram[i] = value;
            self.ram_dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ROM of `banks` 16 KiB banks where every byte holds its bank number (mod 256).
    fn banked_rom(banks: usize, kind: u8, ram_size: u8) -> Vec<u8> {
        let mut rom: Vec<u8> = (0..banks).flat_map(|b| vec![b as u8; ROM_BANK]).collect();
        rom[0x147] = kind;
        rom[0x149] = ram_size;
        rom
    }

    #[test]
    fn mbc1_switches_rom_banks() {
        let mut cart = Cartridge::from_rom(banked_rom(8, 0x03, 0x03)).unwrap();
        assert_eq!(cart.read_rom(0x4000), 1);
        cart.write_rom(0x2000, 5);
        assert_eq!(cart.read_rom(0x4000), 5);
        cart.write_rom(0x2000, 0); // bank 0 maps to 1
        assert_eq!(cart.read_rom(0x4000), 1);
        cart.write_rom(0x2000, 0x1F); // wraps to the ROM size
        assert_eq!(cart.read_rom(0x4000), 7);
    }

    #[test]
    fn mbc1_ram_needs_enable_and_banks_in_advanced_mode() {
        let mut cart = Cartridge::from_rom(banked_rom(8, 0x03, 0x03)).unwrap();
        cart.write_ram(0xA000, 0x42);
        assert_eq!(cart.read_ram(0xA000), 0xFF);

        cart.write_rom(0x0000, 0x0A);
        cart.write_ram(0xA000, 0x42);
        assert_eq!(cart.read_ram(0xA000), 0x42);
        assert!(cart.take_ram_dirty());
        assert!(!cart.take_ram_dirty());

        cart.write_rom(0x6000, 1);
        cart.write_rom(0x4000, 2);
        assert_eq!(cart.read_ram(0xA000), 0x00);
        cart.write_rom(0x4000, 0);
        assert_eq!(cart.read_ram(0xA000), 0x42);
    }

    #[test]
    fn mbc2_ram_is_four_bits_and_mirrored() {
        let mut cart = Cartridge::from_rom(banked_rom(16, 0x06, 0x00)).unwrap();
        assert!(cart.has_battery());
        cart.write_rom(0x0000, 0x0A); // bit 8 clear: RAM enable
        cart.write_ram(0xA000, 0xAB);
        assert_eq!(cart.read_ram(0xA000), 0xFB);
        assert_eq!(cart.read_ram(0xA200), 0xFB); // mirror
        cart.write_rom(0x0100, 0x03); // bit 8 set: ROM bank
        assert_eq!(cart.read_rom(0x4000), 3);
    }

    #[test]
    fn mbc3_rom_banks_and_rtc_latch() {
        let mut cart = Cartridge::from_rom(banked_rom(128, 0x10, 0x03)).unwrap();
        cart.write_rom(0x2000, 0x45);
        assert_eq!(cart.read_rom(0x4000), 0x45);

        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x08); // map RTC seconds
        cart.tick(CLOCK_HZ * 3);
        assert_eq!(cart.read_ram(0xA000), 0, "unlatched read shows old value");
        cart.write_rom(0x6000, 0x00);
        cart.write_rom(0x6000, 0x01);
        assert_eq!(cart.read_ram(0xA000), 3);
    }

    #[test]
    fn rtc_day_counter_overflow_sets_carry() {
        let mut rtc = Rtc {
            regs: [59, 59, 23, 0xFF, 0x01], // day 511, 23:59:59
            ..Default::default()
        };
        rtc.advance_second();
        assert_eq!(rtc.regs, [0, 0, 0, 0, Rtc::DAY_CARRY]);
    }

    #[test]
    fn huc1_banks_ram_and_infrared_mode() {
        let mut cart = Cartridge::from_rom(banked_rom(64, 0xFF, 0x03)).unwrap();
        assert!(cart.has_battery());
        cart.write_rom(0x2000, 0x3F);
        assert_eq!(cart.read_rom(0x4000), 0x3F);
        cart.write_rom(0x4000, 0x02);
        cart.write_ram(0xA000, 0x42); // RAM needs no enabling
        cart.write_rom(0x0000, 0x0E);
        assert_eq!(cart.read_ram(0xA000), IR_DARK, "infrared port: no light");
        cart.write_ram(0xA000, 0x01); // LED on: not RAM
        cart.write_rom(0x0000, 0x00);
        assert_eq!(cart.read_ram(0xA000), 0x42);
        cart.write_rom(0x4000, 0x00);
        assert_eq!(cart.read_ram(0xA000), 0x00);
    }

    #[test]
    fn mbc5_rumble_motor_uses_ram_bank_bit_3() {
        let mut cart = Cartridge::from_rom(banked_rom(4, 0x1E, 0x03)).unwrap();
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x01);
        cart.write_ram(0xA000, 0x55);
        cart.write_rom(0x4000, 0x09); // motor on, still RAM bank 1
        assert_eq!(cart.read_ram(0xA000), 0x55);
        cart.tick(300);
        cart.write_rom(0x4000, 0x01); // motor off
        cart.tick(100);
        assert_eq!(cart.take_rumble(), Some(0.75));
        assert_eq!(cart.take_rumble(), None, "no time passed");

        // Without a motor, bit 3 selects RAM bank 8+ and nothing rumbles.
        let mut plain = Cartridge::from_rom(banked_rom(4, 0x1B, 0x04)).unwrap();
        plain.write_rom(0x4000, 0x08);
        plain.tick(100);
        assert_eq!(plain.take_rumble(), None);
    }

    #[test]
    fn mbc5_nine_bit_rom_bank_and_bank_zero() {
        let mut rom = banked_rom(512, 0x1B, 0x04);
        rom[0x102 * ROM_BANK] = 0xAB; // tell bank $102 apart from bank $02
        let mut cart = Cartridge::from_rom(rom).unwrap();
        cart.write_rom(0x2000, 0x00);
        assert_eq!(cart.read_rom(0x4000), 0, "MBC5 can map bank 0");
        cart.write_rom(0x2000, 0x02);
        assert_eq!(cart.read_rom(0x4000), 0x02);
        cart.write_rom(0x3000, 0x01); // bank $102
        assert_eq!(cart.read_rom(0x4000), 0xAB);

        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x0F);
        cart.write_ram(0xA000, 0x99);
        cart.write_rom(0x4000, 0x00);
        assert_eq!(cart.read_ram(0xA000), 0x00);
        cart.write_rom(0x4000, 0x0F);
        assert_eq!(cart.read_ram(0xA000), 0x99);
    }

    #[test]
    fn save_round_trip() {
        let mut cart = Cartridge::from_rom(banked_rom(4, 0x1B, 0x02)).unwrap();
        cart.load_ram(&[1, 2, 3]);
        assert_eq!(&cart.ram()[..4], &[1, 2, 3, 0]);
        assert!(!cart.take_ram_dirty());
    }
}
