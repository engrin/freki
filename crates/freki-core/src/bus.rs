//! The memory bus: maps the 16-bit address space onto the hardware.
//!
//! | Range       | Contents                          |
//! |-------------|-----------------------------------|
//! | 0000–7FFF   | Cartridge ROM (banked by the MBC) |
//! | 8000–9FFF   | VRAM                              |
//! | A000–BFFF   | Cartridge RAM                     |
//! | C000–DFFF   | Work RAM (D000–DFFF banked on CGB) |
//! | E000–FDFF   | Echo of C000–DDFF                 |
//! | FE00–FE9F   | OAM (sprite attributes)           |
//! | FEA0–FEFF   | Unusable                          |
//! | FF00–FF7F   | I/O registers                     |
//! | FF80–FFFE   | High RAM                          |
//! | FFFF        | Interrupt enable (IE)             |

use crate::apu::Apu;
use crate::boot::BootRom;
use crate::cartridge::Cartridge;
use crate::cheats::Cheats;
use crate::debugger::Breakpoints;
use crate::joypad::Joypad;
use crate::ppu::Ppu;
use crate::serial::Serial;
use crate::timer::Timer;
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

/// Interrupt bits, shared by IE ($FFFF) and IF ($FF0F).
pub mod interrupt {
    pub const VBLANK: u8 = 1 << 0;
    pub const STAT: u8 = 1 << 1;
    pub const TIMER: u8 = 1 << 2;
    pub const SERIAL: u8 = 1 << 3;
    pub const JOYPAD: u8 = 1 << 4;
}

#[derive(Serialize, Deserialize)]
pub struct Bus {
    pub cart: Cartridge,
    pub ppu: Ppu,
    pub timer: Timer,
    pub joypad: Joypad,
    pub serial: Serial,
    pub apu: Apu,
    /// Not part of save states: kept across loads.
    #[serde(skip)]
    pub cheats: Cheats,
    /// Not part of save states: kept across loads.
    #[serde(skip)]
    pub breaks: Breakpoints,
    /// Game Boy Color hardware (in CGB mode or DMG compatibility mode).
    pub cgb_hardware: bool,
    /// CGB mode: the CGB registers (banks, palettes, speed, VRAM DMA) are available.
    /// Off on a DMG and in the CGB's DMG compatibility mode.
    pub cgb: bool,
    /// KEY0 ($FF4C) bit 2, written by the CGB boot ROM: switch to DMG compatibility mode
    /// when the boot ROM is unmapped.
    key0_dmg: bool,
    /// Work RAM: 8 banks of 4 KiB (the DMG only uses banks 0 and 1).
    wram: Vec<u8>,
    /// SVBK ($FF70): work RAM bank at $D000 (CGB).
    svbk: u8,
    /// KEY1 ($FF4D) bit 0: a speed switch is armed and happens on the next STOP (CGB).
    speed_switch_armed: bool,
    /// CGB double speed: the CPU, timer, serial port and OAM DMA run twice as fast.
    pub double_speed: bool,
    /// Odd T-cycle left over when halving CPU cycles for the PPU and APU.
    half_cycle: u32,
    #[serde(with = "BigArray")]
    hram: [u8; 0x7F],
    /// Not part of save states: kept across loads.
    #[serde(skip)]
    boot_rom: Option<BootRom>,
    /// Whether the boot ROM is mapped over $0000–$00FF (until $FF50 is written).
    boot_rom_mapped: bool,
    /// Last value written to DMA ($FF46).
    dma_page: u8,
    pub interrupt_enable: u8,
    pub interrupt_flag: u8,
    dma: Dma,
    hdma: Hdma,
    /// T-cycles the CPU must pause for a VRAM DMA (taken by the CPU before its next step).
    dma_stall: u32,
}

/// CGB VRAM DMA ($FF51–$FF55): copies 16-byte blocks into VRAM, either all at once
/// (general purpose) or one block at the start of each HBlank.
#[derive(Serialize, Deserialize, Default, Clone, Copy)]
struct Hdma {
    source: u16,
    /// Offset into VRAM ($0000–$1FF0).
    dest: u16,
    /// Blocks left in an HBlank transfer, minus one (bits 0–6 of HDMA5).
    remaining: u8,
    /// An HBlank transfer is in progress.
    active: bool,
}

/// T-cycles (at normal speed) the CPU is paused per 16-byte VRAM DMA block.
const HDMA_BLOCK_CYCLES: u32 = 32;

/// OAM DMA state. A write to $FF46 starts a transfer after one M-cycle of setup; it then
/// copies one byte per M-cycle for 160 M-cycles. While it runs, the CPU cannot use OAM
/// or the bus the DMA reads from.
#[derive(Serialize, Deserialize, Default, Clone, Copy)]
struct Dma {
    /// A requested transfer: (source page, T-cycles until it starts).
    pending: Option<(u8, u8)>,
    /// The running transfer: (source address of the next byte, T-cycles into its M-cycle).
    active: Option<(u16, u8)>,
}

impl Bus {
    pub fn new(cart: Cartridge) -> Self {
        Self {
            cart,
            ppu: Ppu::new(),
            timer: Timer::new(),
            joypad: Joypad::new(),
            serial: Serial::new(),
            apu: Apu::new(),
            cheats: Cheats::new(),
            breaks: Breakpoints::default(),
            cgb_hardware: false,
            cgb: false,
            key0_dmg: false,
            wram: vec![0; 0x8000],
            svbk: 0,
            speed_switch_armed: false,
            double_speed: false,
            half_cycle: 0,
            hram: [0; 0x7F],
            boot_rom: None,
            boot_rom_mapped: false,
            dma_page: 0xFF,
            interrupt_enable: 0,
            interrupt_flag: 0x01,
            dma: Dma::default(),
            hdma: Hdma {
                remaining: 0x7F,
                ..Hdma::default()
            },
            dma_stall: 0,
        }
    }

    /// A bus at power-on, with `boot` mapped over the start of the cartridge ROM.
    pub fn power_on(cart: Cartridge, boot: BootRom) -> Self {
        Self {
            ppu: Ppu::power_on(),
            timer: Timer::power_on(),
            apu: Apu::power_on(),
            interrupt_flag: 0,
            boot_rom: Some(boot),
            boot_rom_mapped: true,
            ..Self::new(cart)
        }
    }

    pub fn boot_rom_mapped(&self) -> bool {
        self.boot_rom_mapped
    }

    /// Switches to Game Boy Color hardware in CGB mode.
    pub fn set_cgb(&mut self) {
        self.cgb_hardware = true;
        self.cgb = true;
        self.ppu.set_cgb();
    }

    /// Switches CGB hardware to DMG compatibility mode: the game sees a DMG, but its
    /// shades are coloured through palette RAM (BG palette 0, OBJ palettes 0 and 1).
    pub fn enter_dmg_compat(&mut self) {
        self.cgb = false;
        self.ppu.enter_dmg_compat();
    }

    /// $FF50 write: unmaps the boot ROM (for good); the CGB boot ROM may have asked for
    /// DMG compatibility mode through KEY0.
    fn unmap_boot_rom(&mut self, value: u8) {
        if value & 1 == 0 || !self.boot_rom_mapped {
            return;
        }
        self.boot_rom_mapped = false;
        if self.cgb_hardware && self.key0_dmg {
            self.enter_dmg_compat();
        }
    }

    /// Index into `wram` for an address in $C000–$FDFF.
    fn wram_index(&self, addr: u16) -> usize {
        let offset = addr as usize & 0x0FFF;
        if addr & 0x1000 == 0 {
            offset
        } else {
            let bank = if self.cgb { (self.svbk & 7).max(1) } else { 1 };
            bank as usize * 0x1000 + offset
        }
    }

    /// STOP: performs an armed CGB speed switch. Returns whether it switched.
    pub fn stop(&mut self) -> bool {
        if !(self.cgb && self.speed_switch_armed) {
            return false;
        }
        self.speed_switch_armed = false;
        self.double_speed = !self.double_speed;
        self.timer.write(0xFF04, 0);
        true
    }

    // Specific I/O registers are matched before the generic $FF00–$FF7F fallback.
    #[allow(clippy::match_overlapping_arm)]
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x08FF
                if let Some(byte) = self
                    .boot_rom
                    .as_ref()
                    .filter(|_| self.boot_rom_mapped)
                    .and_then(|boot| boot.read(addr)) =>
            {
                byte
            }
            0x0000..=0x7FFF => {
                let byte = self.cart.read_rom(addr);
                if self.cheats.has_rom_patches() {
                    self.cheats.patch_rom_read(addr, byte)
                } else {
                    byte
                }
            }
            0x8000..=0x9FFF => self.ppu.read_vram(addr),
            0xA000..=0xBFFF => self.cart.read_ram(addr),
            0xC000..=0xFDFF => self.wram[self.wram_index(addr)],
            0xFE00..=0xFE9F => self.ppu.read_oam(addr),
            0xFEA0..=0xFEFF => 0xFF,
            0xFF00 => self.joypad.read(),
            0xFF01..=0xFF02 => self.serial.read(addr),
            0xFF04..=0xFF07 => self.timer.read(addr),
            0xFF0F => self.interrupt_flag | 0xE0,
            0xFF10..=0xFF3F => self.apu.read(addr),
            0xFF46 => self.dma_page,
            0xFF40..=0xFF4B => self.ppu.read_reg(addr),
            0xFF4D if self.cgb => {
                0x7E | u8::from(self.double_speed) << 7 | u8::from(self.speed_switch_armed)
            }
            0xFF4F | 0xFF68..=0xFF6C if self.cgb => self.ppu.read_reg(addr),
            0xFF55 if self.cgb => u8::from(!self.hdma.active) << 7 | self.hdma.remaining,
            0xFF70 if self.cgb => 0xF8 | self.svbk,
            0xFF00..=0xFF7F => 0xFF, // unmapped on the DMG
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            0xFFFF => self.interrupt_enable,
        }
    }

    #[allow(clippy::match_overlapping_arm)]
    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x7FFF => self.cart.write_rom(addr, value),
            0x8000..=0x9FFF => self.ppu.write_vram(addr, value),
            0xA000..=0xBFFF => self.cart.write_ram(addr, value),
            0xC000..=0xFDFF => {
                let i = self.wram_index(addr);
                self.wram[i] = value;
            }
            0xFE00..=0xFE9F => self.ppu.write_oam(addr, value),
            0xFEA0..=0xFEFF => {}
            0xFF00 => self.joypad.write(value),
            0xFF01..=0xFF02 => self.serial.write(addr, value),
            0xFF04 => {
                // Resetting the counter can make the serial clock (bit 8) fall.
                let serial_clock_high = self.timer.counter() & 0x100 != 0;
                self.timer.write(addr, value);
                self.serial.clock(serial_clock_high as u32);
            }
            0xFF05..=0xFF07 => self.timer.write(addr, value),
            0xFF0F => self.interrupt_flag = value & 0x1F,
            0xFF10..=0xFF3F => self.apu.write(addr, value),
            0xFF46 => self.oam_dma(value),
            0xFF50 => self.unmap_boot_rom(value),
            0xFF4C if self.cgb && self.boot_rom_mapped => self.key0_dmg = value & 0x04 != 0,
            0xFF40..=0xFF4B => self.ppu.write_reg(addr, value),
            0xFF4D if self.cgb => self.speed_switch_armed = value & 1 != 0,
            0xFF4F | 0xFF68..=0xFF6C if self.cgb => self.ppu.write_reg(addr, value),
            0xFF51 if self.cgb => {
                self.hdma.source = (self.hdma.source & 0xFF) | (value as u16) << 8
            }
            0xFF52 if self.cgb => {
                self.hdma.source = (self.hdma.source & 0xFF00) | (value & 0xF0) as u16
            }
            0xFF53 if self.cgb => {
                self.hdma.dest = (self.hdma.dest & 0xFF) | ((value & 0x1F) as u16) << 8;
            }
            0xFF54 if self.cgb => {
                self.hdma.dest = (self.hdma.dest & 0x1F00) | (value & 0xF0) as u16
            }
            0xFF55 if self.cgb => self.start_hdma(value),
            0xFF70 if self.cgb => self.svbk = value & 7,
            0xFF00..=0xFF7F => {}
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,
            0xFFFF => self.interrupt_enable = value,
        }
    }

    /// HDMA5 write: starts a general-purpose or HBlank VRAM DMA, or stops an HBlank one.
    fn start_hdma(&mut self, value: u8) {
        let blocks = value & 0x7F;
        if self.hdma.active && value & 0x80 == 0 {
            self.hdma.active = false;
            return;
        }
        self.hdma.remaining = blocks;
        if value & 0x80 == 0 {
            // General purpose: everything now.
            for _ in 0..=blocks {
                self.hdma_block();
            }
            self.hdma.remaining = 0x7F;
        } else {
            self.hdma.active = true;
            // With the LCD off (or already in HBlank) no HBlank start will come soon;
            // hardware copies the first block straight away in that case.
            if !self.ppu.lcd_enabled() {
                self.hdma_hblank_block();
            }
        }
    }

    /// Copies one 16-byte block and charges the CPU for it.
    fn hdma_block(&mut self) {
        for _ in 0..16 {
            let byte = match self.hdma.source {
                0x8000..=0x9FFF => 0xFF, // VRAM as source is not supported by hardware
                src => self.read(src),
            };
            self.ppu.write_vram(0x8000 | self.hdma.dest, byte);
            self.hdma.source = self.hdma.source.wrapping_add(1);
            self.hdma.dest = (self.hdma.dest + 1) & 0x1FFF;
        }
        self.dma_stall += HDMA_BLOCK_CYCLES << u32::from(self.double_speed);
    }

    /// One block of an HBlank transfer.
    fn hdma_hblank_block(&mut self) {
        self.hdma_block();
        if self.hdma.remaining == 0 {
            self.hdma.active = false;
            self.hdma.remaining = 0x7F;
        } else {
            self.hdma.remaining -= 1;
        }
    }

    /// T-cycles the CPU has to pause for VRAM DMA (resets the count).
    pub fn take_dma_stall(&mut self) -> u32 {
        std::mem::take(&mut self.dma_stall)
    }

    /// Starts an OAM DMA from `page`×$100 (see [`Dma`]). A transfer already running keeps
    /// going during the new one's setup cycle.
    fn oam_dma(&mut self, page: u8) {
        self.dma_page = page;
        self.dma.pending = Some((page, 8));
    }

    /// What OAM DMA reads at `addr`. On the DMG, sources from $E000 up read work RAM.
    fn dma_source(&self, addr: u16) -> u8 {
        match addr {
            0xE000..=0xFFFF => self.wram[self.wram_index(0xC000 | (addr & 0x1FFF))],
            _ => self.read(addr),
        }
    }

    /// Advances OAM DMA by one T-cycle.
    fn tick_dma(&mut self) {
        if let Some((page, delay)) = self.dma.pending {
            if delay > 1 {
                self.dma.pending = Some((page, delay - 1));
            } else {
                self.dma.pending = None;
                self.dma.active = Some(((page as u16) << 8, 0));
                return;
            }
        }
        if let Some((src, t)) = self.dma.active {
            if t < 3 {
                self.dma.active = Some((src, t + 1));
                return;
            }
            let byte = self.dma_source(src);
            self.ppu.write_oam(0xFE00 | (src & 0xFF), byte);
            self.dma.active = ((src & 0xFF) < 0x9F).then_some((src + 1, 0));
        }
    }

    /// If OAM DMA blocks the CPU from `addr`: `Some(value)` the CPU sees instead.
    fn dma_conflict(&self, addr: u16) -> Option<u8> {
        let (src, _) = self.dma.active?;
        let on_vram_bus = |a: u16| (0x8000..0xA000).contains(&a);
        match addr {
            0xFE00..=0xFEFF => Some(0xFF),
            0xFF00..=0xFFFF => None,
            // Same bus as the DMA source: the CPU sees the byte being transferred.
            _ if on_vram_bus(addr) == on_vram_bus(src) => Some(self.dma_source(src)),
            _ => None,
        }
    }

    /// Whether the PPU locks the CPU out of `addr` for a read or a write (VRAM while
    /// drawing, OAM during OAM scan and drawing; see [`Ppu`] for the exact windows).
    fn ppu_locked(&self, addr: u16, write: bool) -> bool {
        match addr {
            0x8000..=0x9FFF if write => !self.ppu.cpu_vram_writable(),
            0x8000..=0x9FFF => !self.ppu.cpu_vram_readable(),
            0xFE00..=0xFE9F if write => !self.ppu.cpu_oam_writable(),
            0xFE00..=0xFE9F => !self.ppu.cpu_oam_readable(),
            _ => false,
        }
    }

    /// A read by the CPU (subject to OAM DMA bus conflicts and PPU locking).
    pub fn cpu_read(&self, addr: u16) -> u8 {
        if let Some(value) = self.dma_conflict(addr) {
            value
        } else if self.ppu_locked(addr, false) {
            0xFF
        } else {
            self.read(addr)
        }
    }

    /// A write by the CPU (dropped when OAM DMA or the PPU blocks the address).
    pub fn cpu_write(&mut self, addr: u16, value: u8) {
        if self.dma_conflict(addr).is_none() && !self.ppu_locked(addr, true) {
            self.write(addr, value);
        }
    }

    /// After loading a save state into `self`: takes over what states do not contain
    /// from the bus being replaced (the ROM, audio output configuration, cheats and
    /// breakpoints).
    pub(crate) fn adopt_runtime_from(&mut self, old: &mut Bus) {
        self.cart.adopt_rom_from(&mut old.cart);
        self.boot_rom = old.boot_rom.take();
        self.apu.adopt_output_from(&old.apu);
        self.cheats = std::mem::take(&mut old.cheats);
        self.breaks = std::mem::take(&mut old.breaks);
    }

    pub fn read_u16(&self, addr: u16) -> u16 {
        u16::from_le_bytes([self.read(addr), self.read(addr.wrapping_add(1))])
    }

    /// Advances all non-CPU hardware by `cycles` T-cycles and collects interrupts.
    pub fn tick(&mut self, cycles: u32) {
        // In double speed the PPU, APU and cartridge clock keep running at normal speed.
        let video_cycles = if self.double_speed {
            let total = cycles + self.half_cycle;
            self.half_cycle = total & 1;
            total / 2
        } else {
            cycles
        };
        self.cart.tick(video_cycles);
        if self.dma.pending.is_some() || self.dma.active.is_some() {
            for _ in 0..cycles {
                self.tick_dma();
            }
        }
        self.apu.tick(video_cycles);
        let counter = self.timer.counter() as u32;
        self.interrupt_flag |= self.timer.tick(cycles);
        // Falling edges of counter bit 8 clock the serial port.
        self.serial
            .clock(((counter + cycles) >> 9) - (counter >> 9));
        self.interrupt_flag |= self.ppu.tick(video_cycles);
        if self.ppu.take_hblank_started() && self.hdma.active {
            self.hdma_hblank_block();
        }
        self.interrupt_flag |= self.joypad.take_interrupt();
        self.interrupt_flag |= self.serial.take_interrupt();
    }
}
