//! The Pixel Processing Unit: LCD timing, STAT interrupts and pixel-by-pixel rendering.
//!
//! Mode 3 (drawing) is emulated dot by dot, following the DMG pixel pipeline:
//!
//! - A **background fetcher** reads a tile number, then the tile row's low and high bytes
//!   (2 dots per step), and pushes 8 pixels into the **background FIFO** once it is empty.
//! - Each dot, one pixel is shifted out of the background FIFO, mixed with the **object
//!   FIFO**, run through the palette and written to the screen.
//! - At the start of a line the first fetch is thrown away (6 dots) and `SCX % 8` pixels
//!   are discarded; the **window** restarts the fetcher when it begins; each **sprite**
//!   pauses the pipeline while its pixels are fetched. Mode 3 therefore lasts 172 dots or
//!   more, as on hardware.
//!
//! Registers are read at the dot they are used, so mid-scanline changes (palettes,
//! scroll, LCDC bits, window position) show up exactly where they happen.

use std::collections::VecDeque;

use crate::bus::interrupt;

pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;

const DOTS_PER_LINE: u32 = 456;
const LINES_PER_FRAME: u8 = 154;
const OAM_SCAN_DOTS: u32 = 80;
const MAX_SPRITES_PER_LINE: usize = 10;
/// Dots spent on the discarded first tile fetch of every line.
const STARTUP_DOTS: u8 = 6;
/// Dots the pipeline is paused to fetch one sprite.
const SPRITE_FETCH_DOTS: u8 = 6;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FetchStep {
    Tile,
    DataLow,
    DataHigh,
    Push,
}

/// A sprite selected during OAM scan for the current line.
#[derive(Debug, Clone, Copy)]
struct Sprite {
    y: u8,
    x: u8,
    tile: u8,
    attr: u8,
    fetched: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct ObjPixel {
    color: u8,
    palette1: bool,
    behind_bg: bool,
}

/// Pixel-pipeline state for the scanline being drawn.
#[derive(Debug, Default)]
struct Line {
    bg_fifo: VecDeque<u8>,
    obj_fifo: VecDeque<ObjPixel>,
    step: Option<FetchStep>,
    /// Second dot of a 2-dot fetch step.
    step_half: bool,
    tile: u8,
    lo: u8,
    hi: u8,
    /// Tile column counter of the fetcher; reset to 0 when the window starts.
    fetch_x: u8,
    in_window: bool,
    window_used: bool,
    /// Next screen x to output.
    lx: u8,
    /// Pixels still to drop from the front of the FIFO (SCX % 8, or the window's WX < 7).
    discard: u8,
    startup: u8,
    sprites: Vec<Sprite>,
    /// Sprite being fetched and dots remaining.
    sprite_fetch: Option<(usize, u8)>,
    /// All 160 pixels are out; mode 0 starts on the next dot.
    finished: bool,
}

pub struct Ppu {
    vram: [u8; 0x2000],
    oam: [u8; 0xA0],
    framebuffer: [u8; SCREEN_WIDTH * SCREEN_HEIGHT],
    frame_ready: bool,
    /// Dot position within the current scanline.
    dot: u32,
    mode: Mode,
    line: Line,
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
            line: Line::default(),
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
                OAM_SCAN_DOTS => self.start_drawing(),
                _ => {}
            }
            if self.mode == Mode::Drawing {
                self.draw_dot();
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

    /// End of OAM scan: select this line's sprites and reset the pixel pipeline.
    fn start_drawing(&mut self) {
        self.mode = Mode::Drawing;
        let height = self.sprite_height();
        let ly = self.ly;
        let sprites = self
            .oam
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|s| {
                let top = s[0] as i16 - 16;
                (top..top + height as i16).contains(&(ly as i16))
            })
            .take(MAX_SPRITES_PER_LINE)
            .map(|&[y, x, tile, attr]| Sprite {
                y,
                x,
                tile,
                attr,
                fetched: false,
            })
            .collect();
        let mut line = std::mem::take(&mut self.line);
        line.bg_fifo.clear();
        line.obj_fifo.clear();
        self.line = Line {
            bg_fifo: line.bg_fifo,
            obj_fifo: line.obj_fifo,
            step: Some(FetchStep::Tile),
            discard: self.scx & 7,
            startup: STARTUP_DOTS,
            sprites,
            ..Line::default()
        };
    }

    fn sprite_height(&self) -> u8 {
        if self.lcdc & lcdc::OBJ_SIZE_8X16 != 0 {
            16
        } else {
            8
        }
    }

    /// One dot of mode 3.
    fn draw_dot(&mut self) {
        if self.line.finished {
            self.mode = Mode::HBlank;
            if self.line.window_used {
                self.window_line += 1;
            }
            return;
        }
        if self.line.startup > 0 {
            self.line.startup -= 1;
            return;
        }

        if let Some((i, left)) = self.line.sprite_fetch {
            if left > 1 {
                self.line.sprite_fetch = Some((i, left - 1));
            } else {
                self.line.sprite_fetch = None;
                self.fetch_sprite(i);
            }
            return;
        }

        self.check_window_start();

        if self.lcdc & lcdc::OBJ_ENABLE != 0
            && let Some(i) = self.next_sprite()
        {
            // The sprite fetch waits for the background fetcher to have a tile ready.
            if self.line.step == Some(FetchStep::Push) && !self.line.bg_fifo.is_empty() {
                self.line.sprite_fetch = Some((i, SPRITE_FETCH_DOTS));
            } else {
                self.fetcher_dot();
            }
            return;
        }

        self.fetcher_dot();
        self.output_pixel();
    }

    /// First unfetched sprite (in OAM order) that starts at the current x.
    fn next_sprite(&self) -> Option<usize> {
        let lx = self.line.lx as u16;
        self.line
            .sprites
            .iter()
            .position(|s| !s.fetched && (s.x as u16) <= lx + 8)
    }

    fn check_window_start(&mut self) {
        let l = &self.line;
        if l.in_window
            || self.lcdc & lcdc::WINDOW_ENABLE == 0
            || !self.window_y_triggered
            || self.wx > 166
        {
            return;
        }
        let starts = if self.wx < 7 {
            // WX 0–6: the window starts at x = 0 with its first 7 − WX pixels hidden.
            l.lx == 0 && l.discard == 0
        } else {
            l.lx as u16 + 7 == self.wx as u16
        };
        if starts {
            let line = &mut self.line;
            line.bg_fifo.clear();
            line.step = Some(FetchStep::Tile);
            line.step_half = false;
            line.fetch_x = 0;
            line.in_window = true;
            line.window_used = true;
            if self.wx < 7 {
                line.discard = 7 - self.wx;
            }
        }
    }

    /// Advances the background/window fetcher by one dot.
    fn fetcher_dot(&mut self) {
        let Some(step) = self.line.step else { return };
        if step != FetchStep::Push && !self.line.step_half {
            self.line.step_half = true;
            return;
        }
        self.line.step_half = false;
        match step {
            FetchStep::Tile => {
                if self.line.in_window && self.lcdc & lcdc::WINDOW_ENABLE == 0 {
                    self.line.in_window = false; // window switched off mid-line
                }
                let (high_map, col, row) = if self.line.in_window {
                    let map = self.lcdc & lcdc::WINDOW_MAP_9C00 != 0;
                    (map, self.line.fetch_x & 31, self.window_line / 8)
                } else {
                    let map = self.lcdc & lcdc::BG_MAP_9C00 != 0;
                    let col = ((self.scx >> 3).wrapping_add(self.line.fetch_x)) & 31;
                    (map, col, self.ly.wrapping_add(self.scy) / 8)
                };
                let base = if high_map { 0x1C00 } else { 0x1800 };
                self.line.tile = self.vram[base + row as usize * 32 + col as usize];
                self.line.step = Some(FetchStep::DataLow);
            }
            FetchStep::DataLow => {
                self.line.lo = self.vram[self.tile_row_addr()];
                self.line.step = Some(FetchStep::DataHigh);
            }
            FetchStep::DataHigh => {
                self.line.hi = self.vram[self.tile_row_addr() + 1];
                self.line.step = Some(FetchStep::Push);
            }
            FetchStep::Push => {
                if self.line.bg_fifo.is_empty() {
                    let (lo, hi) = (self.line.lo, self.line.hi);
                    for bit in (0..8).rev() {
                        let color = (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1);
                        self.line.bg_fifo.push_back(color);
                    }
                    self.line.fetch_x = self.line.fetch_x.wrapping_add(1);
                    self.line.step = Some(FetchStep::Tile);
                }
            }
        }
    }

    /// VRAM offset of the low byte of the current background/window tile row.
    fn tile_row_addr(&self) -> usize {
        let y = if self.line.in_window {
            self.window_line % 8
        } else {
            self.ly.wrapping_add(self.scy) % 8
        };
        let tile = self.line.tile;
        let base = if self.lcdc & lcdc::TILE_DATA_8000 != 0 {
            tile as usize * 16
        } else {
            (0x1000 + (tile as i8 as isize) * 16) as usize
        };
        base + y as usize * 2
    }

    /// Fetches sprite `i` and merges it into the object FIFO.
    fn fetch_sprite(&mut self, i: usize) {
        self.line.sprites[i].fetched = true;
        let s = self.line.sprites[i];
        let height = self.sprite_height();
        let mut row = self.ly.wrapping_add(16).wrapping_sub(s.y);
        if row >= height {
            return; // OBJ size shrank mid-line: this sprite no longer covers the line
        }
        if s.attr & 0x40 != 0 {
            row = height - 1 - row; // vertical flip
        }
        let tile = if height == 16 { s.tile & 0xFE } else { s.tile };
        let addr = tile as usize * 16 + row as usize * 2;
        let (lo, hi) = (self.vram[addr], self.vram[addr + 1]);

        let lx = self.line.lx as i16;
        for px in 0..8u8 {
            let screen_x = s.x as i16 - 8 + px as i16;
            if screen_x < lx {
                continue; // already drawn (sprite partly off the left edge)
            }
            let slot = (screen_x - lx) as usize;
            let bit = if s.attr & 0x20 != 0 { px } else { 7 - px }; // horizontal flip
            let color = (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1);
            while self.line.obj_fifo.len() <= slot {
                self.line.obj_fifo.push_back(ObjPixel::default());
            }
            // Earlier (higher-priority) sprites keep their opaque pixels.
            if self.line.obj_fifo[slot].color == 0 {
                self.line.obj_fifo[slot] = ObjPixel {
                    color,
                    palette1: s.attr & 0x10 != 0,
                    behind_bg: s.attr & 0x80 != 0,
                };
            }
        }
    }

    /// Shifts one pixel out of the FIFOs onto the screen.
    fn output_pixel(&mut self) {
        let Some(bg) = self.line.bg_fifo.pop_front() else {
            return;
        };
        if self.line.discard > 0 {
            self.line.discard -= 1;
            return;
        }
        let obj = self.line.obj_fifo.pop_front().unwrap_or_default();

        let bg = if self.lcdc & lcdc::BG_WINDOW_ENABLE != 0 {
            bg
        } else {
            0
        };
        let obj_visible =
            obj.color != 0 && self.lcdc & lcdc::OBJ_ENABLE != 0 && !(obj.behind_bg && bg != 0);
        let shade = if obj_visible {
            let palette = if obj.palette1 { self.obp1 } else { self.obp0 };
            (palette >> (obj.color * 2)) & 3
        } else {
            (self.bgp >> (bg * 2)) & 3
        };

        let x = self.line.lx as usize;
        self.framebuffer[self.ly as usize * SCREEN_WIDTH + x] = shade;
        self.line.lx += 1;
        if self.line.lx as usize == SCREEN_WIDTH {
            self.line.finished = true;
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
        ppu.tick(DOTS_PER_LINE);
        assert_eq!(&ppu.framebuffer[..9], &[3, 3, 3, 3, 3, 3, 3, 3, 0]);
    }

    /// Dots spent in mode 3 on line 0 with the given setup.
    fn mode3_length(setup: impl FnOnce(&mut Ppu)) -> u32 {
        let mut ppu = Ppu::new();
        setup(&mut ppu);
        let mut dots = 0;
        for _ in 0..DOTS_PER_LINE {
            ppu.tick(1);
            if ppu.mode == Mode::Drawing {
                dots += 1;
            }
        }
        dots
    }

    #[test]
    fn mode3_is_172_dots_plus_scx_and_sprite_penalties() {
        assert_eq!(mode3_length(|_| {}), 172);
        assert_eq!(mode3_length(|p| p.write_reg(0xFF43, 3)), 175);
        let with_sprite = mode3_length(|p| {
            p.write_reg(0xFF40, 0x93); // LCD, BG, OBJ on
            p.write_oam(0xFE00, 16); // y: covers line 0
            p.write_oam(0xFE01, 8); // x: screen x 0
        });
        assert!(with_sprite > 172, "sprite adds dots, got {with_sprite}");
    }
}
