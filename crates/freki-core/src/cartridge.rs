//! Cartridge loading, header parsing and memory bank controllers (MBCs).
//!
//! Supported: no MBC, MBC1, MBC2, MBC3 (with real-time clock), MBC5, MBC6, MBC7, MMM01,
//! HuC1, HuC3 and TAMA5.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::CLOCK_HZ;
use crate::camera::Camera;
use crate::huc3::HuC3Clock;
use crate::mbc6::Mbc6;
use crate::mbc7::{self, Mbc7};
use crate::mmm01::Mmm01;
use crate::tama5::{self, Tama5};
use crate::unlicensed::{self, Sachen};

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
    /// Hudson HuC3: up to 32 KiB RAM, a real-time clock, an infrared port and a tone
    /// generator (silent here); see [`crate::huc3`].
    HuC3 {
        /// What $A000–$BFFF does ($0A RAM, $0B–$0D clock, $0E infrared, else RAM read only).
        mode: u8,
        rom_bank: u8,
        ram_bank: u8,
        clock: Box<HuC3Clock>,
    },
    /// MBC7: accelerometer and serial EEPROM (see [`crate::mbc7`]); the EEPROM is the RAM.
    Mbc7 {
        /// Both enables are needed: $0A to $0000–$1FFF and $40 to $4000–$5FFF.
        ram_enable1: bool,
        ram_enable2: bool,
        rom_bank: u8,
        chip: Mbc7,
    },
    /// MMM01: multi-game collections; a menu, then one game's slice of the ROM (see
    /// [`crate::mmm01`]).
    Mmm01(Mmm01),
    /// Bandai TAMA5 (Tamagotchi 3): banking, save memory and clock all behind a register
    /// window at $A000/$A001 (see [`crate::tama5`]).
    Tama5(Box<Tama5>),
    /// MBC6 (Net de Get): ROM and RAM banked in halves, and 1 MiB of flash (see
    /// [`crate::mbc6`]).
    Mbc6(Box<Mbc6>),
    /// Game Boy Camera: MBC3-like banking, 128 KiB RAM and the camera sensor (see
    /// [`crate::camera`]).
    Camera(Box<Camera>),
    /// Wisdom Tree: the whole 32 KiB, bank chosen by the written address (see
    /// [`crate::unlicensed`]).
    WisdomTree { bank: u8 },
    /// M161 (Mani 4-in-1): the whole 32 KiB, switched once.
    M161 { bank: u8, locked: bool },
    /// Sachen MMC1 / MMC2 (see [`crate::unlicensed::Sachen`]).
    Sachen(Box<Sachen>),
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
    const SAVE_BYTES: usize = 48;

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

    /// Moves the clock on by `n` seconds at once (time that passed while the game was
    /// not running). A halted clock stays put.
    fn advance_seconds(&mut self, mut n: u64) {
        if self.halted() {
            return;
        }
        // A game can write out-of-range values (say 63 seconds); those count up to their
        // wrap one second at a time, as on the cartridge.
        while n > 0 && !(self.regs[0] < 60 && self.regs[1] < 60 && self.regs[2] < 24) {
            self.advance_second();
            n -= 1;
        }
        if n == 0 {
            return;
        }
        let [s, m, h, dl, dh] = self.regs;
        let day = u64::from(dh & 1) << 8 | u64::from(dl);
        let total = ((day * 24 + u64::from(h)) * 60 + u64::from(m)) * 60 + u64::from(s) + n;
        let days = total / 86_400;
        let carry = if days >= 512 { Self::DAY_CARRY } else { 0 };
        let day = days % 512;
        self.regs = [
            (total % 60) as u8,
            (total / 60 % 60) as u8,
            (total / 3600 % 24) as u8,
            day as u8,
            (dh & !1) | carry | (day >> 8) as u8,
        ];
    }

    /// The clock as VBA-M, BGB, mGBA and SameBoy append it to the save file: the live and
    /// the latched registers as ten little-endian 32-bit words, then the Unix time it was
    /// written at as a 64-bit word.
    fn to_save(&self, now: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::SAVE_BYTES);
        for &reg in self.regs.iter().chain(&self.latched) {
            out.extend(u32::from(reg).to_le_bytes());
        }
        out.extend(now.to_le_bytes());
        out
    }

    /// Reads [`Rtc::to_save`]'s block (or the older 44-byte one with a 32-bit time) and
    /// runs the clock on to `now`; `None` leaves it where it was saved.
    fn from_save(data: &[u8], now: Option<u64>) -> Option<Self> {
        let word = |i: usize| -> Option<u64> {
            let bytes = data.get(4 * i..4 * i + 4)?;
            Some(u64::from(u32::from_le_bytes(bytes.try_into().ok()?)))
        };
        let reg = |i: usize| word(i).map(|w| w as u8 & Self::MASKS[i % 5]);
        let mut rtc = Self::default();
        for i in 0..5 {
            rtc.regs[i] = reg(i)?;
            rtc.latched[i] = reg(i + 5)?;
        }
        let saved = match data.get(40..48) {
            Some(bytes) => u64::from_le_bytes(bytes.try_into().ok()?),
            None => word(10)?,
        };
        if let Some(now) = now {
            rtc.advance_seconds(now.saturating_sub(saved));
        }
        Some(rtc)
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

/// Whether `rom` is an MMM01 collection, judged by the menu's header in the last 32 KiB
/// (the first bank holds a game's header). As in SameBoy, $11 (MBC3) there counts too,
/// for some collections that claim it.
fn is_mmm01(rom: &[u8]) -> bool {
    let Some(menu) = rom.len().checked_sub(0x8000).filter(|&m| m > 0) else {
        return false;
    };
    rom[0x104..0x134] == rom[menu + 0x104..menu + 0x134]
        && matches!(rom[menu + 0x147], 0x0B..=0x0D | 0x11)
}

/// Some MMM01 dumps put the menu first (its header says $0B–$0D); the cartridge has it
/// in the last 32 KiB, so move it there.
fn mmm01_menu_last(mut rom: Vec<u8>) -> Vec<u8> {
    if matches!(rom[0x147], 0x0B..=0x0D) && rom.len() > 0x8000 {
        rom.rotate_left(0x8000);
    }
    rom
}

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
    /// MBC7: how far the Game Boy is tilted, in g (see [`Cartridge::set_tilt`]).
    #[serde(skip)]
    tilt: (f32, f32),
    /// HuC1/HuC3: the cartridge's infrared LED is on.
    #[serde(default)]
    ir_led: bool,
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
        let rom = mmm01_menu_last(rom);
        let mmm01 = is_mmm01(&rom);
        let header = if mmm01 { rom.len() - 0x8000 } else { 0 };
        let kind = if mmm01 { 0x0B } else { rom[0x147] };
        let unlicensed = if mmm01 {
            None
        } else {
            unlicensed::detect(&rom)
        };
        let mbc = match unlicensed {
            Some(unlicensed::Kind::WisdomTree) => Mbc::WisdomTree { bank: 0 },
            Some(unlicensed::Kind::M161) => Mbc::M161 {
                bank: 0,
                locked: false,
            },
            Some(unlicensed::Kind::SachenMmc1) => Mbc::Sachen(Box::new(Sachen::new(false))),
            Some(unlicensed::Kind::SachenMmc2) => Mbc::Sachen(Box::new(Sachen::new(true))),
            None => match kind {
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
                0x0B..=0x0D => Mbc::Mmm01(Mmm01::default()),
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
                0x20 => Mbc::Mbc6(Box::default()),
                0x22 => Mbc::Mbc7 {
                    ram_enable1: false,
                    ram_enable2: false,
                    rom_bank: 1,
                    chip: Mbc7::default(),
                },
                0xFC => Mbc::Camera(Box::default()),
                0xFD => Mbc::Tama5(Box::default()),
                0xFE => Mbc::HuC3 {
                    mode: 0,
                    rom_bank: 1,
                    ram_bank: 0,
                    clock: Box::default(),
                },
                0xFF => Mbc::HuC1 {
                    ir_mode: false,
                    rom_bank: 1,
                    ram_bank: 0,
                },
                other => return Err(CartridgeError::UnsupportedType(other)),
            },
        };
        let battery = matches!(
            rom[header + 0x147],
            0x03 | 0x06
                | 0x0D
                | 0x09
                | 0x0F
                | 0x10
                | 0x13
                | 0x1B
                | 0x1E
                | 0x20
                | 0x22
                | 0xFC
                | 0xFD
                | 0xFE
                | 0xFF
        );
        let ram_size = match (&mbc, rom[header + 0x149]) {
            (Mbc::Mbc2 { .. }, _) => MBC2_RAM,
            (Mbc::Mbc7 { .. }, _) => mbc7::EEPROM_BYTES,
            (Mbc::Tama5(_), _) => tama5::RAM_BYTES,
            (_, 0x02) => 8 * 1024,
            (_, 0x03) => 32 * 1024,
            (_, 0x04) => 128 * 1024,
            (_, 0x05) => 64 * 1024,
            _ => 0,
        };
        // An unused EEPROM reads as all ones.
        let blank = if matches!(mbc, Mbc::Mbc7 { .. }) {
            0xFF
        } else {
            0
        };
        Ok(Self {
            rom,
            ram: vec![blank; ram_size],
            mbc,
            battery,
            ram_dirty: false,
            motor_cycles: (0, 0),
            tilt: (0.0, 0.0),
            ir_led: false,
        })
    }

    /// Moves the ROM over from `old` (save states do not contain it). Battery RAM restored
    /// from a state is marked dirty so the save file is brought in line with it.
    /// The whole ROM.
    pub(crate) fn rom_data(&self) -> &[u8] {
        &self.rom
    }

    pub(crate) fn adopt_rom_from(&mut self, old: &mut Cartridge) {
        self.rom = std::mem::take(&mut old.rom);
        if let (Mbc::Camera(new), Mbc::Camera(old)) = (&mut self.mbc, &mut old.mbc) {
            new.adopt_image_from(old);
        }
        self.ram_dirty = self.battery;
    }

    /// Game title from the header ($0134–$0143), trimmed at the first NUL. On CGB games
    /// $0143 is the CGB flag, not part of the title.
    pub fn title(&self) -> String {
        let end = if self.supports_cgb() { 0x143 } else { 0x144 };
        let raw = &self.header()[0x134..end];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        String::from_utf8_lossy(&raw[..end]).trim().to_string()
    }

    /// Whether the header ($0143 bit 7) says the game uses Game Boy Color features.
    pub fn supports_cgb(&self) -> bool {
        self.header()[0x143] & 0x80 != 0
    }

    /// Whether the game runs only on a Game Boy Color ($0143 = $C0).
    pub fn cgb_only(&self) -> bool {
        self.header()[0x143] == 0xC0
    }

    /// The ROM's header area: the menu's, at the end, on an MMM01 collection.
    fn header(&self) -> &[u8] {
        match self.mbc {
            Mbc::Mmm01(_) => &self.rom[self.rom.len() - 0x8000..],
            _ => &self.rom,
        }
    }

    pub fn mbc(&self) -> &Mbc {
        &self.mbc
    }

    /// Whether the cartridge keeps its RAM or clock when switched off, i.e. needs a save
    /// file.
    pub fn has_battery(&self) -> bool {
        self.battery
            && (!self.ram.is_empty() || self.has_clock() || matches!(self.mbc, Mbc::Mbc6(_)))
    }

    /// Whether the cartridge has a real-time clock (MBC3 with timer, HuC3).
    pub fn has_clock(&self) -> bool {
        matches!(
            self.mbc,
            Mbc::Mbc3 { rtc: Some(_), .. } | Mbc::HuC3 { .. } | Mbc::Tama5(_)
        )
    }

    /// External RAM contents.
    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    /// The save file: external RAM, followed by the clock (if any) stamped with `now`, the
    /// current Unix time in seconds.
    pub fn save_data(&self, now: u64) -> Vec<u8> {
        let mut data = self.ram.clone();
        match &self.mbc {
            Mbc::Mbc3 { rtc: Some(rtc), .. } => data.extend(rtc.to_save(now)),
            Mbc::HuC3 { clock, .. } => data.extend(clock.to_save(now)),
            Mbc::Tama5(chip) => data.extend(chip.to_save(now)),
            Mbc::Mbc6(chip) => data.extend(chip.to_save()),
            _ => {}
        }
        data
    }

    /// The Unix time the clock block of save file `data` was written at, or `None` if the
    /// cartridge has no clock or the block is missing.
    pub fn save_time(&self, data: &[u8]) -> Option<u64> {
        let footer = data.get(self.ram.len()..)?;
        let word = |at: usize| -> Option<u64> {
            Some(u64::from_le_bytes(footer.get(at..at + 8)?.try_into().ok()?))
        };
        match &self.mbc {
            Mbc::Mbc3 { rtc: Some(_), .. } => word(40).or_else(|| {
                let old = footer.get(40..44)?;
                Some(u64::from(u32::from_le_bytes(old.try_into().ok()?)))
            }),
            Mbc::HuC3 { .. } if footer.len() >= 12 => word(0),
            Mbc::Tama5(_) => word(32),
            _ => None,
        }
    }

    /// Restores external RAM and the clock from a save file. Extra bytes are ignored,
    /// missing ones stay 0. The clock catches up on the time since the save, from its
    /// timestamp to `now` (Unix seconds); with `now` = `None` it carries on from where it
    /// was saved. A save without a clock block leaves the clock as it is.
    pub fn load_ram(&mut self, data: &[u8], now: Option<u64>) {
        let n = data.len().min(self.ram.len());
        self.ram[..n].copy_from_slice(&data[..n]);
        self.ram_dirty = false;
        let footer = data.get(self.ram.len()..).unwrap_or_default();
        match &mut self.mbc {
            Mbc::Mbc3 { rtc: Some(rtc), .. } => {
                if let Some(saved) = Rtc::from_save(footer, now) {
                    *rtc = saved;
                }
            }
            Mbc::HuC3 { clock, .. } => {
                clock.load_save(footer, now);
            }
            Mbc::Tama5(chip) => chip.load_save(footer, now),
            Mbc::Mbc6(chip) => chip.load_save(footer),
            _ => {}
        }
    }

    /// Returns `true` if RAM changed since the last call.
    pub fn take_ram_dirty(&mut self) -> bool {
        std::mem::take(&mut self.ram_dirty)
    }

    /// Advances time-based hardware (the MBC3 clock) by `cycles` T-cycles.
    pub fn tick(&mut self, cycles: u32) {
        if let Mbc::Camera(camera) = &mut self.mbc
            && camera.tick(cycles, &mut self.ram)
        {
            self.ram_dirty = true;
        }
        match &mut self.mbc {
            Mbc::Mbc3 { rtc: Some(rtc), .. } => rtc.tick(cycles),
            Mbc::HuC3 { clock, .. } => clock.tick(cycles),
            Mbc::Tama5(chip) => chip.tick(cycles),
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

    /// Whether the cartridge has a tilt sensor (MBC7).
    pub fn has_tilt_sensor(&self) -> bool {
        matches!(self.mbc, Mbc::Mbc7 { .. })
    }

    /// Sets how far the Game Boy is tilted, in g, by which way things on it would roll:
    /// `x` to the right, `y` towards the bottom of the screen (about ±1 at most for normal
    /// play).
    pub fn set_tilt(&mut self, x: f32, y: f32) {
        self.tilt = (x, y);
    }

    /// Whether `byte` is at `addr` ($0000–$7FFF) in some ROM bank that can appear there:
    /// any bank at $4000–$7FFF; at $0000–$3FFF bank 0, or $20, $40 and $60 (which MBC1's
    /// large-ROM mode maps there).
    pub fn rom_has(&self, addr: u16, byte: u8) -> bool {
        let offset = addr as usize & (ROM_BANK - 1);
        self.rom
            .chunks(ROM_BANK)
            .enumerate()
            .filter(|&(bank, _)| addr >= 0x4000 || bank % 0x20 == 0)
            .any(|(_, bank)| bank.get(offset) == Some(&byte))
    }

    /// Whether some bank holds all of `bytes` (address, byte) at once; like
    /// [`Cartridge::rom_has`] for several neighbouring bytes.
    pub fn rom_has_all(&self, bytes: &[(u16, u8)]) -> bool {
        let Some(&(first, _)) = bytes.first() else {
            return true;
        };
        self.rom
            .chunks(ROM_BANK)
            .enumerate()
            .filter(|&(bank, _)| first >= 0x4000 || bank % 0x20 == 0)
            .any(|(_, bank)| {
                bytes
                    .iter()
                    .all(|&(addr, byte)| bank.get(addr as usize & (ROM_BANK - 1)) == Some(&byte))
            })
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
            Mbc::Mmm01(ref m) => {
                if !m.ram_enabled {
                    return None;
                }
                m.ram_bank() * RAM_BANK + local
            }
            Mbc::Mbc6(ref chip) => chip.ram_offset(addr)?,
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
                let last_bank = if self.is_mbc30() { 0x07 } else { 0x03 };
                if !ram_enabled || ram_select > last_bank {
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
            Mbc::HuC3 { mode, ram_bank, .. } => {
                if !matches!(mode, 0x00..=0x0A) {
                    return None;
                }
                ram_bank as usize * RAM_BANK + local
            }
            // The EEPROM is reached through registers only.
            Mbc::Mbc7 { .. } | Mbc::Tama5(_) => return None,
            Mbc::WisdomTree { .. } | Mbc::M161 { .. } | Mbc::Sachen(_) => return None,
            // RAM is always readable, but not while the registers are mapped or a picture
            // is being taken.
            Mbc::Camera(ref camera) => {
                if camera.registers_selected() || camera.busy() {
                    return None;
                }
                usize::from(camera.ram_bank & 0x0F) * RAM_BANK + local
            }
        };
        Some(offset % self.ram.len())
    }

    /// Read from $0000–$7FFF.
    pub fn read_rom(&self, addr: u16) -> u8 {
        if let Mbc::Sachen(ref sachen) = self.mbc {
            let (bank, addr) = sachen.map(addr);
            return self.rom_byte(bank, addr);
        }
        match self.mbc {
            Mbc::RomOnly => return self.rom.get(addr as usize).copied().unwrap_or(0xFF),
            Mbc::Mbc6(ref chip) if addr >= 0x4000 => {
                if let Some(value) = chip.read_flash(addr) {
                    return value;
                }
                let offset = chip.rom_offset(addr) % self.rom.len().next_power_of_two();
                return self.rom.get(offset).copied().unwrap_or(0xFF);
            }
            _ => {}
        }
        self.rom_byte(self.rom_bank_at(addr), addr)
    }

    /// The ROM bank mapped at `addr` ($0000–$7FFF), as debuggers and symbol files count
    /// them (wrapped to the ROM's size).
    pub fn rom_bank_at(&self, addr: u16) -> usize {
        let low = addr < 0x4000;
        let bank = match self.mbc {
            Mbc::RomOnly => usize::from(!low),
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
            Mbc::Tama5(ref chip) => {
                if low {
                    0
                } else {
                    chip.rom_bank()
                }
            }
            Mbc::Mmm01(ref m) => {
                let (bank0, bank) = m.rom_banks(self.rom.len() / ROM_BANK);
                if low { bank0 } else { bank }
            }
            Mbc::Mbc6(ref chip) if !low => chip.rom_offset(addr) / ROM_BANK,
            Mbc::Camera(ref camera) => {
                if low {
                    0
                } else {
                    camera.rom_bank as usize
                }
            }
            Mbc::WisdomTree { bank } | Mbc::M161 { bank, .. } => {
                unlicensed::bank_of_32k(bank, addr)
            }
            Mbc::Sachen(ref sachen) => sachen.bank_at(addr),
            Mbc::Mbc6(_) => 0,
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
            Mbc::HuC1 { rom_bank, .. }
            | Mbc::HuC3 { rom_bank, .. }
            | Mbc::Mbc7 { rom_bank, .. } => {
                if low {
                    0
                } else {
                    rom_bank as usize
                }
            }
        };
        bank % (self.rom.len() / ROM_BANK).max(1)
    }

    /// Whether this MBC3 cartridge is an MBC30 (only the Japanese Pokémon Crystal): the
    /// same chip with 8 RAM banks (64 KiB) and an 8-bit ROM bank register (4 MiB). The
    /// header type is MBC3's, so the sizes tell them apart, as in other emulators.
    fn is_mbc30(&self) -> bool {
        matches!(self.mbc, Mbc::Mbc3 { .. })
            && (self.ram.len() > 4 * RAM_BANK || self.rom.len() > 128 * ROM_BANK)
    }

    /// A CPU access to $C000 and up (Sachen MMC2 watches those).
    pub(crate) fn high_access(&self) {
        if let Mbc::Sachen(sachen) = &self.mbc {
            sachen.high_access();
        }
    }

    /// Started without a boot ROM (see [`Sachen::skip_boot`]).
    pub(crate) fn skip_boot(&mut self) {
        if let Mbc::Sachen(sachen) = &mut self.mbc {
            sachen.skip_boot();
        }
    }

    /// The Game Boy Camera's sensor, if this is the camera cartridge.
    pub fn camera(&self) -> Option<&Camera> {
        match &self.mbc {
            Mbc::Camera(camera) => Some(camera),
            _ => None,
        }
    }

    pub fn camera_mut(&mut self) -> Option<&mut Camera> {
        match &mut self.mbc {
            Mbc::Camera(camera) => Some(camera),
            _ => None,
        }
    }

    /// Size of the ROM in bytes.
    pub fn rom_len(&self) -> usize {
        self.rom.len()
    }

    /// The cartridge RAM bank mapped at $A000–$BFFF, if RAM is accessible.
    pub fn ram_bank_at(&self, addr: u16) -> Option<usize> {
        self.ram_offset(addr).map(|offset| offset / RAM_BANK)
    }

    /// Write to $0000–$7FFF (MBC control registers).
    pub fn write_rom(&mut self, addr: u16, value: u8) {
        let rom_bank_mask = if self.is_mbc30() { 0xFF } else { 0x7F };
        match &mut self.mbc {
            // TAMA5 is driven through $A000/$A001 only.
            Mbc::RomOnly | Mbc::Tama5(_) => {}
            // The low byte of the address picks the bank; the value is ignored.
            Mbc::WisdomTree { bank } => *bank = addr as u8,
            Mbc::M161 { bank, locked } => {
                if !*locked {
                    *bank = value & 0x07;
                    *locked = true;
                }
            }
            Mbc::Sachen(sachen) => sachen.write(addr, value),
            Mbc::Camera(camera) => match addr {
                0x0000..=0x1FFF => camera.ram_enabled = value & 0x0F == 0x0A,
                0x2000..=0x3FFF => camera.rom_bank = value & 0x3F,
                0x4000..=0x5FFF => camera.ram_bank = value & 0x1F,
                _ => {}
            },
            Mbc::Mmm01(m) => m.write(addr, value),
            Mbc::Mbc6(chip) => {
                if chip.write(addr, value) {
                    self.ram_dirty = true;
                }
            }
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
                0x2000..=0x3FFF => *rom_bank = (value & rom_bank_mask).max(1),
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
            Mbc::HuC3 {
                mode,
                rom_bank,
                ram_bank,
                ..
            } => match addr {
                0x0000..=0x1FFF => *mode = value & 0x0F,
                0x2000..=0x3FFF => *rom_bank = value & 0x7F,
                0x4000..=0x5FFF => *ram_bank = value & 0x03,
                _ => {}
            },
            Mbc::Mbc7 {
                ram_enable1,
                ram_enable2,
                rom_bank,
                ..
            } => match addr {
                0x0000..=0x1FFF => *ram_enable1 = value == 0x0A,
                0x2000..=0x3FFF => *rom_bank = value & 0x7F,
                0x4000..=0x5FFF => *ram_enable2 = value == 0x40,
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
        match &self.mbc {
            Mbc::HuC1 { ir_mode: true, .. } | Mbc::HuC3 { mode: 0x0E, .. } => return IR_DARK,
            Mbc::HuC3 {
                mode: 0x0C, clock, ..
            } => return clock.response(),
            Mbc::HuC3 { mode: 0x0D, .. } => return 0xFF, // ready
            Mbc::HuC3 { mode: 0x0B, .. } => return 0xFF,
            Mbc::Mbc7 {
                ram_enable1: true,
                ram_enable2: true,
                chip,
                ..
            } if addr < 0xB000 => return chip.read((addr >> 4) as u8 & 0x0F),
            Mbc::Mbc7 { .. } => return 0xFF,
            Mbc::Tama5(chip) => return chip.read(addr, &self.ram),
            Mbc::Camera(camera) if camera.registers_selected() => {
                return camera.read_register(addr);
            }
            Mbc::Camera(camera) if camera.busy() => return 0x00,
            _ => {}
        }
        let Some(i) = self.ram_offset(addr) else {
            return 0xFF;
        };
        match self.mbc {
            Mbc::Mbc2 { .. } => self.ram[i] | 0xF0, // only the low nibble exists
            _ => self.ram[i],
        }
    }

    /// HuC1/HuC3: $A000–$BFFF is the infrared port (bit 0 written: the LED; read: whether
    /// light is seen, which the bus answers).
    pub fn ir_selected(&self) -> bool {
        matches!(
            self.mbc,
            Mbc::HuC1 { ir_mode: true, .. } | Mbc::HuC3 { mode: 0x0E, .. }
        )
    }

    /// HuC1/HuC3: the cartridge's infrared LED is on.
    pub fn ir_led(&self) -> bool {
        self.ir_led
    }

    /// Write to external RAM at $A000–$BFFF.
    pub fn write_ram(&mut self, addr: u16, value: u8) {
        if let Some((rtc, reg)) = self.rtc_register() {
            rtc.write(reg, value);
            return;
        }
        if self.ir_selected() {
            self.ir_led = value & 1 != 0;
            return;
        }
        if let Mbc::HuC3 { mode, clock, .. } = &mut self.mbc
            && *mode != 0x0A
        {
            // Clock commands in mode $0B; the other modes ignore writes (read-only RAM,
            // the infrared LED, the ready flag).
            if *mode == 0x0B {
                clock.command(value);
            }
            return;
        }
        if let Mbc::Tama5(chip) = &mut self.mbc {
            if chip.write(addr, value, &mut self.ram) {
                self.ram_dirty = true;
            }
            return;
        }
        if let Mbc::Camera(camera) = &mut self.mbc {
            if camera.registers_selected() {
                camera.write_register(addr, value);
                return;
            }
            if !camera.ram_enabled {
                return;
            }
        }
        if let Mbc::Mbc7 {
            ram_enable1: true,
            ram_enable2: true,
            chip,
            ..
        } = &mut self.mbc
            && addr < 0xB000
        {
            let reg = (addr >> 4) as u8 & 0x0F;
            if chip.write(reg, value, self.tilt, &mut self.ram) {
                self.ram_dirty = true;
            }
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

    /// MBC30 (Japanese Pokémon Crystal: 2 MiB ROM, 64 KiB RAM, clock): RAM banks 4–7
    /// exist; a plain MBC3 with 32 KiB has none there. Bigger MBC30 ROMs use bit 7 of the
    /// ROM bank.
    #[test]
    fn mbc30_has_eight_ram_banks_and_256_rom_banks() {
        let mut crystal = Cartridge::from_rom(banked_rom(128, 0x10, 0x05)).unwrap();
        assert_eq!(crystal.ram().len(), 64 * 1024);
        crystal.write_rom(0x0000, 0x0A);
        for bank in 0..8u8 {
            crystal.write_rom(0x4000, bank);
            crystal.write_ram(0xA123, 0x40 + bank);
        }
        for bank in 0..8u8 {
            crystal.write_rom(0x4000, bank);
            assert_eq!(crystal.read_ram(0xA123), 0x40 + bank, "RAM bank {bank}");
        }
        crystal.write_rom(0x4000, 0x08); // the clock still answers at $08–$0C
        crystal.write_rom(0x6000, 0x00);
        crystal.write_rom(0x6000, 0x01);
        assert_eq!(crystal.read_ram(0xA000), 0, "seconds");

        let mut mbc3 = Cartridge::from_rom(banked_rom(128, 0x10, 0x03)).unwrap();
        mbc3.write_rom(0x0000, 0x0A);
        mbc3.write_rom(0x4000, 0x05);
        mbc3.write_ram(0xA123, 0x99);
        assert_eq!(mbc3.read_ram(0xA123), 0xFF, "no RAM bank 5 on an MBC3");
        assert!(mbc3.ram().iter().all(|&b| b == 0));

        let mut big = Cartridge::from_rom(banked_rom(256, 0x10, 0x05)).unwrap();
        big.write_rom(0x2000, 0xC5);
        assert_eq!(big.read_rom(0x4000), 0xC5);
        mbc3.write_rom(0x2000, 0xC5);
        assert_eq!(mbc3.read_rom(0x4000), 0x45, "MBC3 ignores bit 7");
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
        assert!(cart.ir_led());
        cart.write_rom(0x0000, 0x00);
        assert_eq!(cart.read_ram(0xA000), 0x42);
        cart.write_rom(0x4000, 0x00);
        assert_eq!(cart.read_ram(0xA000), 0x00);
    }

    #[test]
    fn huc3_modes_switch_between_ram_clock_and_infrared() {
        let mut cart = Cartridge::from_rom(banked_rom(32, 0xFE, 0x03)).unwrap();
        assert!(cart.has_battery());
        cart.write_rom(0x2000, 0x1F);
        assert_eq!(cart.read_rom(0x4000), 0x1F);
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x01);
        cart.write_ram(0xA000, 0x77);
        cart.write_rom(0x0000, 0x00); // read only
        cart.write_ram(0xA000, 0x11);
        assert_eq!(cart.read_ram(0xA000), 0x77);
        cart.write_rom(0x0000, 0x0B);
        cart.write_ram(0xA000, 0x62); // "ready?"
        cart.write_rom(0x0000, 0x0C);
        assert_eq!(cart.read_ram(0xA000), 0x61);
        cart.write_rom(0x0000, 0x0D);
        assert_eq!(cart.read_ram(0xA000) & 1, 1);
        cart.write_rom(0x0000, 0x0E);
        assert_eq!(cart.read_ram(0xA000), IR_DARK);
        cart.write_ram(0xA000, 0x01);
        assert!(cart.ir_led());
        cart.write_ram(0xA000, 0x00);
        assert!(!cart.ir_led());
    }

    #[test]
    fn mbc7_registers_need_both_enables_and_save_to_ram() {
        let mut cart = Cartridge::from_rom(banked_rom(16, 0x22, 0x00)).unwrap();
        assert!(cart.has_battery() && cart.has_tilt_sensor());
        assert_eq!(cart.ram().len(), mbc7::EEPROM_BYTES);
        cart.write_rom(0x2000, 0x05);
        assert_eq!(cart.read_rom(0x4000), 0x05);
        cart.write_rom(0x0000, 0x0A);
        assert_eq!(cart.read_ram(0xA070), 0xFF, "one enable is not enough");
        cart.write_rom(0x4000, 0x40);
        assert_eq!(cart.read_ram(0xA060), 0x00);
        cart.set_tilt(0.0, -1.0); // rolling up the screen
        cart.write_ram(0xA000, 0x55);
        cart.write_ram(0xA010, 0xAA);
        assert_eq!(cart.read_ram(0xA050), 0x82, "Y high byte: $81D0 + $70");
        assert_eq!(cart.read_ram(0xA040), 0x40);
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
    fn mbc3_clock_is_saved_after_ram_and_catches_up() {
        let mut cart = Cartridge::from_rom(banked_rom(4, 0x10, 0x02)).unwrap();
        let Mbc::Mbc3 { rtc: Some(rtc), .. } = &mut cart.mbc else {
            unreachable!()
        };
        // Day 511, 23:59:50, with the latched copy older.
        rtc.regs = [50, 59, 23, 0xFF, 0x01];
        rtc.latched = [1, 2, 3, 4, 0];
        cart.ram[0] = 0x42;
        let data = cart.save_data(5_000);
        assert_eq!(data.len(), 8 * 1024 + Rtc::SAVE_BYTES);
        assert_eq!(&data[8192..8196], &[50, 0, 0, 0]);
        assert_eq!(&data[8232..8240], &5_000u64.to_le_bytes());
        assert_eq!(cart.save_time(&data), Some(5_000));
        assert_eq!(cart.save_time(&data[..8192]), None, "RAM only");

        // 15 seconds later the day counter has wrapped and set the carry.
        let mut fresh = Cartridge::from_rom(banked_rom(4, 0x10, 0x02)).unwrap();
        fresh.load_ram(&data, Some(5_015));
        assert_eq!(fresh.ram[0], 0x42);
        let Mbc::Mbc3 { rtc: Some(rtc), .. } = &fresh.mbc else {
            unreachable!()
        };
        assert_eq!(rtc.regs, [5, 0, 0, 0, Rtc::DAY_CARRY]);
        assert_eq!(rtc.latched, [1, 2, 3, 4, 0]);

        // A halted clock does not move; the old 44-byte block still loads.
        let mut halted = rtc.clone();
        halted.regs[4] |= Rtc::HALT;
        let mut old = halted.to_save(100);
        old.truncate(40);
        old.extend(100u32.to_le_bytes());
        let loaded = Rtc::from_save(&old, Some(1_000_000)).unwrap();
        assert_eq!(loaded.regs, halted.regs);
    }

    #[test]
    fn rtc_catches_up_from_out_of_range_seconds() {
        let mut rtc = Rtc {
            regs: [62, 0, 0, 0, 0],
            ..Default::default()
        };
        // 62 → 63 → 0 (no minute), then 3600 more seconds.
        rtc.advance_seconds(2 + 3600);
        assert_eq!(rtc.regs, [0, 0, 1, 0, 0]);
    }

    #[test]
    fn clock_only_cartridge_still_gets_a_save_file() {
        // MBC3 + timer + battery ($0F) has no RAM; the save is the clock alone.
        let cart = Cartridge::from_rom(banked_rom(4, 0x0F, 0x00)).unwrap();
        assert!(cart.has_battery());
        assert_eq!(cart.save_data(0).len(), Rtc::SAVE_BYTES);
    }

    /// A 4-game, 256 KiB collection: a game's header in bank 0, the menu's ($0D, 8 KiB
    /// RAM, battery) in the last 32 KiB, each bank filled with its number.
    fn mmm01_rom() -> Vec<u8> {
        let mut rom = banked_rom(16, 0x01, 0x00);
        let menu = rom.len() - 0x8000;
        // Both headers carry the same Nintendo logo.
        rom[0x104..0x134].fill(0xCE);
        rom[menu + 0x104..menu + 0x134].fill(0xCE);
        rom[menu + 0x134..menu + 0x13C].copy_from_slice(b"MENU\0\0\0\0");
        rom[menu + 0x147] = 0x0D;
        rom[menu + 0x149] = 0x02;
        rom
    }

    #[test]
    fn mmm01_is_found_by_the_menu_header_at_the_end() {
        let cart = Cartridge::from_rom(mmm01_rom()).unwrap();
        assert!(matches!(cart.mbc, Mbc::Mmm01(_)));
        assert_eq!(cart.title(), "MENU");
        assert!(cart.has_battery());
        assert_eq!(cart.ram().len(), 8 * 1024);
        assert_eq!((cart.read_rom(0x0000), cart.read_rom(0x4000)), (14, 15));

        // A dump with the menu first is the same cartridge.
        let mut menu_first = mmm01_rom();
        menu_first.rotate_right(0x8000);
        let mut cart = Cartridge::from_rom(menu_first).unwrap();
        assert_eq!((cart.read_rom(0x0000), cart.read_rom(0x4000)), (14, 15));

        // The menu maps the game at banks 4–7 (mid = 0, high = 0, low bits 2–4 frozen).
        cart.write_rom(0x2000, 0x04);
        cart.write_rom(0x6000, 0b1110 << 2);
        cart.write_rom(0x0000, 0x40);
        assert_eq!((cart.read_rom(0x0000), cart.read_rom(0x4000)), (4, 5));
        cart.write_rom(0x2000, 0x03);
        assert_eq!(cart.read_rom(0x4000), 7);
    }

    #[test]
    fn save_round_trip() {
        let mut cart = Cartridge::from_rom(banked_rom(4, 0x1B, 0x02)).unwrap();
        cart.load_ram(&[1, 2, 3], None);
        assert_eq!(&cart.ram()[..4], &[1, 2, 3, 0]);
        assert!(!cart.take_ram_dirty());
    }
}
