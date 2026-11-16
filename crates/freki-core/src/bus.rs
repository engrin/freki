//! The memory bus: maps the 16-bit address space onto the hardware.
//!
//! | Range       | Contents                          |
//! |-------------|-----------------------------------|
//! | 0000–7FFF   | Cartridge ROM (banked by the MBC) |
//! | 8000–9FFF   | VRAM                              |
//! | A000–BFFF   | Cartridge RAM                     |
//! | C000–DFFF   | Work RAM                          |
//! | E000–FDFF   | Echo of C000–DDFF                 |
//! | FE00–FE9F   | OAM (sprite attributes)           |
//! | FEA0–FEFF   | Unusable                          |
//! | FF00–FF7F   | I/O registers                     |
//! | FF80–FFFE   | High RAM                          |
//! | FFFF        | Interrupt enable (IE)             |

use crate::apu::Apu;
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
    #[serde(with = "BigArray")]
    wram: [u8; 0x2000],
    #[serde(with = "BigArray")]
    hram: [u8; 0x7F],
    /// Last value written to DMA ($FF46).
    dma_page: u8,
    pub interrupt_enable: u8,
    pub interrupt_flag: u8,
    dma: Dma,
}

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
            wram: [0; 0x2000],
            hram: [0; 0x7F],
            dma_page: 0xFF,
            interrupt_enable: 0,
            interrupt_flag: 0x01,
            dma: Dma::default(),
        }
    }

    // Specific I/O registers are matched before the generic $FF00–$FF7F fallback.
    #[allow(clippy::match_overlapping_arm)]
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
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
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize],
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize],
            0xFE00..=0xFE9F => self.ppu.read_oam(addr),
            0xFEA0..=0xFEFF => 0xFF,
            0xFF00 => self.joypad.read(),
            0xFF01..=0xFF02 => self.serial.read(addr),
            0xFF04..=0xFF07 => self.timer.read(addr),
            0xFF0F => self.interrupt_flag | 0xE0,
            0xFF10..=0xFF3F => self.apu.read(addr),
            0xFF46 => self.dma_page,
            0xFF40..=0xFF4B => self.ppu.read_reg(addr),
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
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize] = value,
            0xE000..=0xFDFF => self.wram[(addr - 0xE000) as usize] = value,
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
            0xFF40..=0xFF4B => self.ppu.write_reg(addr, value),
            0xFF00..=0xFF7F => {}
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,
            0xFFFF => self.interrupt_enable = value,
        }
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
            0xE000..=0xFFFF => self.wram[(addr - 0xE000) as usize & 0x1FFF],
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
        self.apu.adopt_output_from(&old.apu);
        self.cheats = std::mem::take(&mut old.cheats);
        self.breaks = std::mem::take(&mut old.breaks);
    }

    pub fn read_u16(&self, addr: u16) -> u16 {
        u16::from_le_bytes([self.read(addr), self.read(addr.wrapping_add(1))])
    }

    /// Advances all non-CPU hardware by `cycles` T-cycles and collects interrupts.
    pub fn tick(&mut self, cycles: u32) {
        self.cart.tick(cycles);
        if self.dma.pending.is_some() || self.dma.active.is_some() {
            for _ in 0..cycles {
                self.tick_dma();
            }
        }
        self.apu.tick(cycles);
        let counter = self.timer.counter() as u32;
        self.interrupt_flag |= self.timer.tick(cycles);
        // Falling edges of counter bit 8 clock the serial port.
        self.serial
            .clock(((counter + cycles) >> 9) - (counter >> 9));
        self.interrupt_flag |= self.ppu.tick(cycles);
        self.interrupt_flag |= self.joypad.take_interrupt();
        self.interrupt_flag |= self.serial.take_interrupt();
    }
}
