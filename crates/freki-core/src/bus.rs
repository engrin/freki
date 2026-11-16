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

use crate::cartridge::Cartridge;
use crate::joypad::Joypad;
use crate::ppu::Ppu;
use crate::serial::Serial;
use crate::timer::Timer;

/// Interrupt bits, shared by IE ($FFFF) and IF ($FF0F).
pub mod interrupt {
    pub const VBLANK: u8 = 1 << 0;
    pub const STAT: u8 = 1 << 1;
    pub const TIMER: u8 = 1 << 2;
    pub const SERIAL: u8 = 1 << 3;
    pub const JOYPAD: u8 = 1 << 4;
}

pub struct Bus {
    pub cart: Cartridge,
    pub ppu: Ppu,
    pub timer: Timer,
    pub joypad: Joypad,
    pub serial: Serial,
    wram: [u8; 0x2000],
    hram: [u8; 0x7F],
    /// Unimplemented I/O registers are stored here so reads return what was written.
    io: [u8; 0x80],
    pub interrupt_enable: u8,
    pub interrupt_flag: u8,
}

impl Bus {
    pub fn new(cart: Cartridge) -> Self {
        Self {
            cart,
            ppu: Ppu::new(),
            timer: Timer::new(),
            joypad: Joypad::new(),
            serial: Serial::new(),
            wram: [0; 0x2000],
            hram: [0; 0x7F],
            io: [0; 0x80],
            interrupt_enable: 0,
            interrupt_flag: 0x01,
        }
    }

    // Specific I/O registers are matched before the generic $FF00–$FF7F fallback.
    #[allow(clippy::match_overlapping_arm)]
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cart.read_rom(addr),
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
            0xFF46 => self.io[0x46],
            0xFF40..=0xFF4B => self.ppu.read_reg(addr),
            0xFF00..=0xFF7F => self.io[(addr - 0xFF00) as usize],
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
            0xFF04..=0xFF07 => self.timer.write(addr, value),
            0xFF0F => self.interrupt_flag = value & 0x1F,
            0xFF46 => self.oam_dma(value),
            0xFF40..=0xFF4B => self.ppu.write_reg(addr, value),
            0xFF00..=0xFF7F => self.io[(addr - 0xFF00) as usize] = value,
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,
            0xFFFF => self.interrupt_enable = value,
        }
    }

    /// OAM DMA: copies 160 bytes from `page`×$100 into OAM. Done instantly; on hardware it
    /// takes 160 M-cycles during which the CPU can only access HRAM.
    fn oam_dma(&mut self, page: u8) {
        self.io[0x46] = page;
        let src = (page as u16) << 8;
        for i in 0..0xA0 {
            let byte = self.read(src + i);
            self.ppu.write_oam(0xFE00 + i, byte);
        }
    }

    pub fn read_u16(&self, addr: u16) -> u16 {
        u16::from_le_bytes([self.read(addr), self.read(addr.wrapping_add(1))])
    }

    /// Advances all non-CPU hardware by `cycles` T-cycles and collects interrupts.
    pub fn tick(&mut self, cycles: u32) {
        self.cart.tick(cycles);
        self.interrupt_flag |= self.timer.tick(cycles);
        self.interrupt_flag |= self.ppu.tick(cycles);
        self.interrupt_flag |= self.joypad.take_interrupt();
        self.interrupt_flag |= self.serial.take_interrupt();
    }
}
