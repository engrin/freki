//! The Pixel Processing Unit: timing, LCD registers and (eventually) rendering.

use crate::bus::interrupt;

pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;

const DOTS_PER_LINE: u32 = 456;
const LINES_PER_FRAME: u8 = 154;

pub struct Ppu {
    vram: [u8; 0x2000],
    oam: [u8; 0xA0],
    framebuffer: [u8; SCREEN_WIDTH * SCREEN_HEIGHT],
    frame_ready: bool,
    /// Dot position within the current scanline.
    dot: u32,

    lcdc: u8,
    stat: u8,
    scy: u8,
    scx: u8,
    ly: u8,
    lyc: u8,
    bgp: u8,
    obp0: u8,
    obp1: u8,
    wy: u8,
    wx: u8,
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

impl Ppu {
    pub fn new() -> Self {
        Self {
            vram: [0; 0x2000],
            oam: [0; 0xA0],
            framebuffer: [0; SCREEN_WIDTH * SCREEN_HEIGHT],
            frame_ready: false,
            dot: 0,
            lcdc: 0x91,
            stat: 0x85,
            scy: 0,
            scx: 0,
            ly: 0,
            lyc: 0,
            bgp: 0xFC,
            obp0: 0xFF,
            obp1: 0xFF,
            wy: 0,
            wx: 0,
        }
    }

    pub fn framebuffer(&self) -> &[u8; SCREEN_WIDTH * SCREEN_HEIGHT] {
        &self.framebuffer
    }

    /// Returns `true` once per completed frame.
    pub fn take_frame_ready(&mut self) -> bool {
        std::mem::take(&mut self.frame_ready)
    }

    pub fn read_vram(&self, addr: u16) -> u8 {
        self.vram[(addr - 0x8000) as usize]
    }
    pub fn write_vram(&mut self, addr: u16, value: u8) {
        self.vram[(addr - 0x8000) as usize] = value;
    }
    pub fn read_oam(&self, addr: u16) -> u8 {
        self.oam[(addr - 0xFE00) as usize]
    }
    pub fn write_oam(&mut self, addr: u16, value: u8) {
        self.oam[(addr - 0xFE00) as usize] = value;
    }

    pub fn read_reg(&self, addr: u16) -> u8 {
        match addr {
            0xFF40 => self.lcdc,
            0xFF41 => self.stat | 0x80,
            0xFF42 => self.scy,
            0xFF43 => self.scx,
            0xFF44 => self.ly,
            0xFF45 => self.lyc,
            0xFF47 => self.bgp,
            0xFF48 => self.obp0,
            0xFF49 => self.obp1,
            0xFF4A => self.wy,
            0xFF4B => self.wx,
            _ => 0xFF,
        }
    }

    pub fn write_reg(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF40 => self.lcdc = value,
            0xFF41 => self.stat = (self.stat & 0x07) | (value & 0x78),
            0xFF42 => self.scy = value,
            0xFF43 => self.scx = value,
            0xFF44 => {} // LY is read-only
            0xFF45 => self.lyc = value,
            0xFF47 => self.bgp = value,
            0xFF48 => self.obp0 = value,
            0xFF49 => self.obp1 = value,
            0xFF4A => self.wy = value,
            0xFF4B => self.wx = value,
            _ => {}
        }
    }

    /// Advances the PPU by `cycles` T-cycles. Returns interrupt bits to raise.
    pub fn tick(&mut self, cycles: u32) -> u8 {
        let mut irq = 0;
        if self.lcdc & 0x80 == 0 {
            return irq; // LCD off
        }
        self.dot += cycles;
        while self.dot >= DOTS_PER_LINE {
            self.dot -= DOTS_PER_LINE;
            if (self.ly as usize) < SCREEN_HEIGHT {
                self.render_scanline();
            }
            self.ly = (self.ly + 1) % LINES_PER_FRAME;
            if self.ly as usize == SCREEN_HEIGHT {
                irq |= interrupt::VBLANK;
                self.frame_ready = true;
            }
        }
        // TODO: STAT modes 0–3, LYC=LY coincidence and STAT interrupts.
        irq
    }

    fn render_scanline(&mut self) {
        // TODO: background, window and sprite rendering.
    }
}
