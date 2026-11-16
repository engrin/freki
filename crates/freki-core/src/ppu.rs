//! The Pixel Processing Unit: LCD timing, STAT interrupts and scanline rendering.
//!
//! Each visible scanline is rendered in one go when mode 3 (drawing) ends, using the
//! registers as they are at that moment. Mid-frame register changes (e.g. from LY=LYC
//! interrupts) therefore take effect from the next line, which is what most games and
//! `dmg-acid2` rely on. Mid-scanline effects are not emulated.

use crate::bus::interrupt;

pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;

const DOTS_PER_LINE: u32 = 456;
const LINES_PER_FRAME: u8 = 154;
const OAM_SCAN_DOTS: u32 = 80;
const DRAWING_END_DOT: u32 = OAM_SCAN_DOTS + 172;
const MAX_SPRITES_PER_LINE: usize = 10;

mod lcdc {
    pub const BG_WINDOW_ENABLE: u8 = 1 << 0;
    pub const OBJ_ENABLE: u8 = 1 << 1;
    pub const OBJ_SIZE_8X16: u8 = 1 << 2;
    pub const BG_MAP_9C00: u8 = 1 << 3;
    pub const TILE_DATA_8000: u8 = 1 << 4;
    pub const WINDOW_ENABLE: u8 = 1 << 5;
    pub const WINDOW_MAP_9C00: u8 = 1 << 6;
    pub const LCD_ENABLE: u8 = 1 << 7;
}

mod stat {
    pub const HBLANK_INT: u8 = 1 << 3;
    pub const VBLANK_INT: u8 = 1 << 4;
    pub const OAM_INT: u8 = 1 << 5;
    pub const LYC_INT: u8 = 1 << 6;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    HBlank = 0,
    VBlank = 1,
    OamScan = 2,
    Drawing = 3,
}

pub struct Ppu {
    vram: [u8; 0x2000],
    oam: [u8; 0xA0],
    framebuffer: [u8; SCREEN_WIDTH * SCREEN_HEIGHT],
    frame_ready: bool,
    /// Dot position within the current scanline.
    dot: u32,
    mode: Mode,
    /// Previous level of the combined STAT interrupt line (interrupts fire on rising edges).
    stat_line: bool,
    /// Internal window line counter: only advances on lines where the window was drawn.
    window_line: u8,
    /// Set once LY == WY has been seen this frame.
    window_y_triggered: bool,

    lcdc: u8,
    /// Writable STAT bits 3–6 (interrupt selects).
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
            mode: Mode::OamScan,
            stat_line: false,
            window_line: 0,
            window_y_triggered: false,
            lcdc: 0x91,
            stat: 0,
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

    pub fn mode(&self) -> Mode {
        self.mode
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

    fn lcd_on(&self) -> bool {
        self.lcdc & lcdc::LCD_ENABLE != 0
    }

    pub fn read_reg(&self, addr: u16) -> u8 {
        match addr {
            0xFF40 => self.lcdc,
            0xFF41 => {
                let coincidence = u8::from(self.ly == self.lyc) << 2;
                let mode = if self.lcd_on() { self.mode as u8 } else { 0 };
                0x80 | self.stat | coincidence | mode
            }
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
            0xFF40 => {
                let was_on = self.lcd_on();
                self.lcdc = value;
                if was_on && !self.lcd_on() {
                    // LCD off: LY resets and the PPU idles until it is switched back on.
                    self.ly = 0;
                    self.dot = 0;
                    self.mode = Mode::HBlank;
                    self.window_line = 0;
                    self.window_y_triggered = false;
                } else if !was_on && self.lcd_on() {
                    self.mode = Mode::OamScan;
                }
            }
            0xFF41 => self.stat = value & 0x78,
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
        if !self.lcd_on() {
            return irq;
        }
        for _ in 0..cycles {
            irq |= self.tick_dot();
        }
        irq
    }

    fn tick_dot(&mut self) -> u8 {
        let mut irq = 0;
        if (self.ly as usize) < SCREEN_HEIGHT {
            match self.dot {
                0 => {
                    if self.ly == self.wy {
                        self.window_y_triggered = true;
                    }
                    self.mode = Mode::OamScan;
                }
                OAM_SCAN_DOTS => self.mode = Mode::Drawing,
                DRAWING_END_DOT => {
                    self.render_scanline();
                    self.mode = Mode::HBlank;
                }
                _ => {}
            }
        }

        self.dot += 1;
        if self.dot == DOTS_PER_LINE {
            self.dot = 0;
            self.ly += 1;
            if self.ly as usize == SCREEN_HEIGHT {
                self.mode = Mode::VBlank;
                self.frame_ready = true;
                irq |= interrupt::VBLANK;
            } else if self.ly == LINES_PER_FRAME {
                self.ly = 0;
                self.window_line = 0;
                self.window_y_triggered = false;
            }
        }

        // STAT interrupt fires on a rising edge of the OR of all enabled sources.
        let line = (self.stat & stat::LYC_INT != 0 && self.ly == self.lyc)
            || (self.stat & stat::HBLANK_INT != 0 && self.mode == Mode::HBlank)
            || (self.stat & stat::VBLANK_INT != 0 && self.mode == Mode::VBlank)
            || (self.stat & stat::OAM_INT != 0 && self.mode == Mode::OamScan);
        if line && !self.stat_line {
            irq |= interrupt::STAT;
        }
        self.stat_line = line;
        irq
    }

    /// Color index (0–3) of pixel (`x`, `y`) in the tile `tile` from the tile data.
    /// `obj` selects the $8000 addressing mode unconditionally, as sprites always use it.
    fn tile_pixel(&self, tile: u8, x: u8, y: u8, obj: bool) -> u8 {
        let base = if obj || self.lcdc & lcdc::TILE_DATA_8000 != 0 {
            tile as usize * 16
        } else {
            (0x1000 + (tile as i8 as isize) * 16) as usize
        };
        let row = base + y as usize * 2;
        let (lo, hi) = (self.vram[row], self.vram[row + 1]);
        let bit = 7 - x;
        (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1)
    }

    /// Tile number at (`col`, `row`) of the 32×32 tile map selected by `high_map`.
    fn map_tile(&self, high_map: bool, col: u8, row: u8) -> u8 {
        let base = if high_map { 0x1C00 } else { 0x1800 };
        self.vram[base + row as usize * 32 + col as usize]
    }

    fn render_scanline(&mut self) {
        let ly = self.ly;
        // Background/window color indices before the palette (needed for sprite priority).
        let mut bg_index = [0u8; SCREEN_WIDTH];

        if self.lcdc & lcdc::BG_WINDOW_ENABLE != 0 {
            let high_map = self.lcdc & lcdc::BG_MAP_9C00 != 0;
            let y = self.scy.wrapping_add(ly);
            for (x, idx) in bg_index.iter_mut().enumerate() {
                let px = self.scx.wrapping_add(x as u8);
                let tile = self.map_tile(high_map, px / 8, y / 8);
                *idx = self.tile_pixel(tile, px % 8, y % 8, false);
            }

            let window_visible =
                self.lcdc & lcdc::WINDOW_ENABLE != 0 && self.window_y_triggered && self.wx <= 166;
            if window_visible {
                let high_map = self.lcdc & lcdc::WINDOW_MAP_9C00 != 0;
                let start = self.wx as isize - 7;
                let wy = self.window_line;
                for (x, idx) in bg_index.iter_mut().enumerate() {
                    let wx = x as isize - start;
                    if wx < 0 {
                        continue;
                    }
                    let wx = wx as u8;
                    let tile = self.map_tile(high_map, wx / 8, wy / 8);
                    *idx = self.tile_pixel(tile, wx % 8, wy % 8, false);
                }
                self.window_line += 1;
            }
        }

        let row = ly as usize * SCREEN_WIDTH;
        for (x, &idx) in bg_index.iter().enumerate() {
            self.framebuffer[row + x] = (self.bgp >> (idx * 2)) & 3;
        }

        if self.lcdc & lcdc::OBJ_ENABLE != 0 {
            self.render_sprites(ly, &bg_index);
        }
    }

    fn render_sprites(&mut self, ly: u8, bg_index: &[u8; SCREEN_WIDTH]) {
        let height: u8 = if self.lcdc & lcdc::OBJ_SIZE_8X16 != 0 {
            16
        } else {
            8
        };

        // OAM scan: the first 10 sprites (in OAM order) that overlap this line.
        let mut sprites: Vec<[u8; 4]> = self
            .oam
            .as_chunks::<4>()
            .0
            .iter()
            .copied()
            .filter(|s| {
                let top = s[0] as i16 - 16;
                (top..top + height as i16).contains(&(ly as i16))
            })
            .take(MAX_SPRITES_PER_LINE)
            .collect();
        // Priority: lower X wins; ties go to the earlier OAM entry (stable sort keeps order).
        sprites.sort_by_key(|s| s[1]);

        let mut drawn = [false; SCREEN_WIDTH];
        for [y, x, tile, attr] in sprites {
            let mut line = ly + 16 - y;
            if attr & 0x40 != 0 {
                line = height - 1 - line; // vertical flip
            }
            let tile = if height == 16 { tile & 0xFE } else { tile };
            let palette = if attr & 0x10 != 0 {
                self.obp1
            } else {
                self.obp0
            };
            let behind_bg = attr & 0x80 != 0;

            for px in 0..8u8 {
                let sx = x as i16 - 8 + px as i16;
                if !(0..SCREEN_WIDTH as i16).contains(&sx) {
                    continue;
                }
                let sx = sx as usize;
                if drawn[sx] {
                    continue;
                }
                let tx = if attr & 0x20 != 0 { 7 - px } else { px }; // horizontal flip
                let idx = self.tile_pixel(tile.wrapping_add(line / 8), tx, line % 8, true);
                if idx == 0 {
                    continue; // transparent
                }
                // The highest-priority opaque sprite pixel owns this column, even when
                // it ends up hidden behind the background.
                drawn[sx] = true;
                if behind_bg && bg_index[sx] != 0 {
                    continue;
                }
                self.framebuffer[ly as usize * SCREEN_WIDTH + sx] = (palette >> (idx * 2)) & 3;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_takes_70224_dots_and_raises_vblank() {
        let mut ppu = Ppu::new();
        let mut vblanks = 0;
        for _ in 0..70_224 {
            if ppu.tick(1) & interrupt::VBLANK != 0 {
                vblanks += 1;
            }
        }
        assert_eq!(vblanks, 1);
        assert_eq!(ppu.ly, 0);
        assert_eq!(ppu.dot, 0);
    }

    #[test]
    fn lyc_stat_interrupt_on_rising_edge() {
        let mut ppu = Ppu::new();
        ppu.write_reg(0xFF45, 2);
        ppu.write_reg(0xFF41, stat::LYC_INT);
        let mut stat_irqs = 0;
        for _ in 0..70_224 {
            if ppu.tick(1) & interrupt::STAT != 0 {
                stat_irqs += 1;
            }
        }
        assert_eq!(stat_irqs, 1);
    }

    #[test]
    fn renders_background_tile_through_palette() {
        let mut ppu = Ppu::new();
        // Tile 1: first row = color index 3 everywhere.
        ppu.write_vram(0x8010, 0xFF);
        ppu.write_vram(0x8011, 0xFF);
        ppu.write_vram(0x9800, 1); // top-left map entry
        ppu.write_reg(0xFF47, 0b1110_0100); // identity palette
        ppu.render_scanline();
        assert_eq!(&ppu.framebuffer[..9], &[3, 3, 3, 3, 3, 3, 3, 3, 0]);
    }
}
