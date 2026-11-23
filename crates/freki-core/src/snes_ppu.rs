//! The SNES picture processor (S-PPU1/2) as far as the Super Game Boy's SNES side needs it:
//! its registers ($2100–$213F) and memories (VRAM, CGRAM, OAM) with their address and latch
//! behaviour, after nocash's "fullsnes" documentation.

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

/// The picture: 256 × 224, 15-bit BGR colours.
pub const WIDTH: usize = 256;
pub const HEIGHT: usize = 224;

#[derive(Serialize, Deserialize, Clone)]
pub struct SnesPpu {
    /// 32 K words.
    pub vram: Vec<u16>,
    /// 256 colours.
    #[serde(with = "BigArray")]
    pub cgram: [u16; 256],
    /// 512 + 32 bytes.
    #[serde(with = "BigArray")]
    pub oam: [u8; 544],
    /// $2100: forced blank (bit 7) and brightness.
    pub inidisp: u8,
    pub obsel: u8,
    /// OAM word address as written, and the current byte address.
    oam_reload: u16,
    oam_addr: u16,
    oam_latch: u8,
    pub oam_priority: bool,
    pub bgmode: u8,
    pub mosaic: u8,
    /// BG1–4 tile map address and size.
    pub bg_sc: [u8; 4],
    /// BG1/2 and BG3/4 character data addresses.
    pub bg_nba: [u8; 2],
    pub bg_hofs: [u16; 4],
    pub bg_vofs: [u16; 4],
    /// The shared latches of the scroll registers.
    scroll_prev: u8,
    scroll_prev_h: u8,
    pub vmain: u8,
    vram_addr: u16,
    vram_prefetch: u16,
    pub m7sel: u8,
    /// Mode 7 matrix A–D and centre X, Y.
    pub m7: [u16; 6],
    m7_prev: u8,
    pub m7_hofs: u16,
    pub m7_vofs: u16,
    cg_addr: u16,
    cg_latch: u8,
    /// Window mask settings $2123–$2125, positions $2126–$2129, logic $212A/$212B.
    pub w12sel: u8,
    pub w34sel: u8,
    pub wobjsel: u8,
    pub window: [u8; 4],
    pub wbglog: u8,
    pub wobjlog: u8,
    /// Main and sub screen layers, and their window masks.
    pub tm: u8,
    pub ts: u8,
    pub tmw: u8,
    pub tsw: u8,
    pub cgwsel: u8,
    pub cgadsub: u8,
    /// Fixed colour ($2132).
    pub coldata: u16,
    pub setini: u8,
    /// Mode 7 multiply result source.
    mpy: i32,
    /// $2137 latched counters and their read flip-flops.
    pub latched_h: u16,
    pub latched_v: u16,
    h_flip: bool,
    v_flip: bool,
    pub counters_latched: bool,
    /// Open bus of PPU1 and PPU2 reads.
    ppu1_bus: u8,
    ppu2_bus: u8,
    /// The picture, 256 × 224 BGR555, as drawn line by line.
    #[serde(skip, default = "blank_frame")]
    pub frame: Vec<u16>,
}

fn blank_frame() -> Vec<u16> {
    vec![0; WIDTH * HEIGHT]
}

/// One layer's pixel: colour (CGRAM index or, for direct colour, unused), priority, and
/// whether colour math applies (OBJ palettes 4–7).
#[derive(Clone, Copy, Default)]
struct Px {
    color: u8,
    priority: u8,
    opaque: bool,
    obj_math: bool,
}

/// Layers in the order of the priority tables: 0–3 BG1–4, 4 OBJ.
const OBJ: usize = 4;

impl Default for SnesPpu {
    fn default() -> Self {
        Self::new()
    }
}

impl SnesPpu {
    pub fn new() -> Self {
        SnesPpu {
            vram: vec![0; 0x8000],
            cgram: [0; 256],
            oam: [0; 544],
            inidisp: 0x80,
            obsel: 0,
            oam_reload: 0,
            oam_addr: 0,
            oam_latch: 0,
            oam_priority: false,
            bgmode: 0,
            mosaic: 0,
            bg_sc: [0; 4],
            bg_nba: [0; 2],
            bg_hofs: [0; 4],
            bg_vofs: [0; 4],
            scroll_prev: 0,
            scroll_prev_h: 0,
            vmain: 0,
            vram_addr: 0,
            vram_prefetch: 0,
            m7sel: 0,
            m7: [0; 6],
            m7_prev: 0,
            m7_hofs: 0,
            m7_vofs: 0,
            cg_addr: 0,
            cg_latch: 0,
            w12sel: 0,
            w34sel: 0,
            wobjsel: 0,
            window: [0; 4],
            wbglog: 0,
            wobjlog: 0,
            tm: 0,
            ts: 0,
            tmw: 0,
            tsw: 0,
            cgwsel: 0,
            cgadsub: 0,
            coldata: 0,
            setini: 0,
            mpy: 0,
            latched_h: 0,
            latched_v: 0,
            h_flip: false,
            v_flip: false,
            counters_latched: false,
            ppu1_bus: 0,
            ppu2_bus: 0,
            frame: blank_frame(),
        }
    }

    /// VRAM word address with the address translation of VMAIN bits 2–3.
    fn vram_word(&self) -> usize {
        let a = self.vram_addr;
        let a = match (self.vmain >> 2) & 3 {
            0 => a,
            1 => (a & 0xFF00) | ((a & 0x001F) << 3) | ((a >> 5) & 7),
            2 => (a & 0xFE00) | ((a & 0x003F) << 3) | ((a >> 6) & 7),
            _ => (a & 0xFC00) | ((a & 0x007F) << 3) | ((a >> 7) & 7),
        };
        (a & 0x7FFF) as usize
    }

    fn vram_step(&self) -> u16 {
        match self.vmain & 3 {
            0 => 1,
            1 => 32,
            _ => 128,
        }
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        match reg {
            0x00 => self.inidisp = v,
            0x01 => self.obsel = v,
            0x02 => {
                self.oam_reload = (self.oam_reload & 0x100) | v as u16;
                self.oam_addr = self.oam_reload << 1;
            }
            0x03 => {
                self.oam_reload = (self.oam_reload & 0xFF) | ((v as u16 & 1) << 8);
                self.oam_priority = v & 0x80 != 0;
                self.oam_addr = self.oam_reload << 1;
            }
            0x04 => {
                let a = self.oam_addr as usize & 0x3FF;
                if a >= 0x200 {
                    self.oam[0x200 + (a & 0x1F)] = v;
                } else if a & 1 == 0 {
                    self.oam_latch = v;
                } else {
                    self.oam[a - 1] = self.oam_latch;
                    self.oam[a] = v;
                }
                self.oam_addr = (self.oam_addr + 1) & 0x3FF;
            }
            0x05 => self.bgmode = v,
            0x06 => self.mosaic = v,
            0x07..=0x0A => self.bg_sc[(reg - 0x07) as usize] = v,
            0x0B | 0x0C => self.bg_nba[(reg - 0x0B) as usize] = v,
            0x0D..=0x14 => {
                let i = ((reg - 0x0D) / 2) as usize;
                if (reg - 0x0D) % 2 == 0 {
                    // BGnHOFS
                    self.bg_hofs[i] = ((v as u16) << 8)
                        | (self.scroll_prev & !7) as u16
                        | (self.scroll_prev_h & 7) as u16;
                    self.scroll_prev_h = v;
                    if i == 0 {
                        self.m7_hofs = ((v as u16) << 8) | self.m7_prev as u16;
                        self.m7_prev = v;
                    }
                } else {
                    self.bg_vofs[i] = ((v as u16) << 8) | self.scroll_prev as u16;
                    if i == 0 {
                        self.m7_vofs = ((v as u16) << 8) | self.m7_prev as u16;
                        self.m7_prev = v;
                    }
                }
                self.scroll_prev = v;
            }
            0x15 => self.vmain = v,
            0x16 | 0x17 => {
                self.vram_addr = if reg == 0x16 {
                    (self.vram_addr & 0xFF00) | v as u16
                } else {
                    (self.vram_addr & 0x00FF) | (v as u16) << 8
                };
                self.vram_prefetch = self.vram[self.vram_word()];
            }
            0x18 | 0x19 => {
                let w = self.vram_word();
                self.vram[w] = if reg == 0x18 {
                    (self.vram[w] & 0xFF00) | v as u16
                } else {
                    (self.vram[w] & 0x00FF) | (v as u16) << 8
                };
                let high = self.vmain & 0x80 != 0;
                if high == (reg == 0x19) {
                    self.vram_addr = self.vram_addr.wrapping_add(self.vram_step());
                }
            }
            0x1A => self.m7sel = v,
            0x1B..=0x20 => {
                let i = (reg - 0x1B) as usize;
                self.m7[i] = (v as u16) << 8 | self.m7_prev as u16;
                self.m7_prev = v;
                if i <= 1 {
                    self.mpy = (self.m7[0] as i16 as i32) * ((self.m7[1] >> 8) as i8 as i32);
                }
            }
            0x21 => {
                self.cg_addr = (v as u16) << 1;
            }
            0x22 => {
                if self.cg_addr & 1 == 0 {
                    self.cg_latch = v;
                } else {
                    let i = (self.cg_addr >> 1) as usize & 0xFF;
                    self.cgram[i] = ((v as u16 & 0x7F) << 8) | self.cg_latch as u16;
                }
                self.cg_addr = (self.cg_addr + 1) & 0x1FF;
            }
            0x23 => self.w12sel = v,
            0x24 => self.w34sel = v,
            0x25 => self.wobjsel = v,
            0x26..=0x29 => self.window[(reg - 0x26) as usize] = v,
            0x2A => self.wbglog = v,
            0x2B => self.wobjlog = v,
            0x2C => self.tm = v,
            0x2D => self.ts = v,
            0x2E => self.tmw = v,
            0x2F => self.tsw = v,
            0x30 => self.cgwsel = v,
            0x31 => self.cgadsub = v,
            0x32 => {
                let i = v as u16 & 0x1F;
                if v & 0x20 != 0 {
                    self.coldata = (self.coldata & !0x001F) | i;
                }
                if v & 0x40 != 0 {
                    self.coldata = (self.coldata & !0x03E0) | i << 5;
                }
                if v & 0x80 != 0 {
                    self.coldata = (self.coldata & !0x7C00) | i << 10;
                }
            }
            0x33 => self.setini = v,
            _ => {}
        }
    }

    /// A read of $2134–$213F; `h`, `v`: the current counters for a latch by reading $2137.
    pub fn read(&mut self, reg: u8, h: u16, vc: u16, cpu_bus: u8) -> u8 {
        match reg {
            0x34 => self.mpy as u8,
            0x35 => (self.mpy >> 8) as u8,
            0x36 => (self.mpy >> 16) as u8,
            0x37 => {
                self.latched_h = h;
                self.latched_v = vc;
                self.counters_latched = true;
                cpu_bus
            }
            0x38 => {
                let a = self.oam_addr as usize & 0x3FF;
                let v = if a >= 0x200 {
                    self.oam[0x200 + (a & 0x1F)]
                } else {
                    self.oam[a]
                };
                self.oam_addr = (self.oam_addr + 1) & 0x3FF;
                self.ppu1_bus = v;
                v
            }
            0x39 | 0x3A => {
                let v = if reg == 0x39 {
                    self.vram_prefetch as u8
                } else {
                    (self.vram_prefetch >> 8) as u8
                };
                let high = self.vmain & 0x80 != 0;
                if high == (reg == 0x3A) {
                    self.vram_prefetch = self.vram[self.vram_word()];
                    self.vram_addr = self.vram_addr.wrapping_add(self.vram_step());
                }
                self.ppu1_bus = v;
                v
            }
            0x3B => {
                let c = self.cgram[(self.cg_addr >> 1) as usize & 0xFF];
                let v = if self.cg_addr & 1 == 0 {
                    c as u8
                } else {
                    (c >> 8) as u8 & 0x7F | (self.ppu2_bus & 0x80)
                };
                self.cg_addr = (self.cg_addr + 1) & 0x1FF;
                self.ppu2_bus = v;
                v
            }
            0x3C => {
                let v = if self.h_flip {
                    (self.latched_h >> 8) as u8 & 1 | (self.ppu2_bus & 0xFE)
                } else {
                    self.latched_h as u8
                };
                self.h_flip = !self.h_flip;
                self.ppu2_bus = v;
                v
            }
            0x3D => {
                let v = if self.v_flip {
                    (self.latched_v >> 8) as u8 & 1 | (self.ppu2_bus & 0xFE)
                } else {
                    self.latched_v as u8
                };
                self.v_flip = !self.v_flip;
                self.ppu2_bus = v;
                v
            }
            0x3E => (self.ppu1_bus & 0x10) | 0x01,
            0x3F => {
                let v = (self.ppu2_bus & 0x20) | u8::from(self.counters_latched) << 6 | 0x03;
                self.counters_latched = false;
                self.h_flip = false;
                self.v_flip = false;
                v
            }
            _ => cpu_bus,
        }
    }

    /// Start of VBlank: OAM address reload (unless the display is forced blank).
    pub fn vblank_start(&mut self) {
        if self.inidisp & 0x80 == 0 {
            self.oam_addr = self.oam_reload << 1;
        }
    }

    // ---- drawing -------------------------------------------------------------------------

    /// Bits per pixel of each BG in the current mode (0: not present).
    fn bg_bpp(&self) -> [u8; 4] {
        match self.bgmode & 7 {
            0 => [2, 2, 2, 2],
            1 => [4, 4, 2, 0],
            2 => [4, 4, 0, 0],
            3 => [8, 4, 0, 0],
            4 => [8, 2, 0, 0],
            5 => [4, 2, 0, 0],
            6 => [4, 0, 0, 0],
            _ => [8, 0, 0, 0],
        }
    }

    /// One BG line: 256 pixels.
    fn bg_line(&self, bg: usize, bpp: u8, y: u16, out: &mut [Px; WIDTH]) {
        let big = self.bgmode & (0x10 << bg) != 0;
        let tile_px: u16 = if big { 16 } else { 8 };
        let sc = self.bg_sc[bg];
        let map_base = ((sc as u16 >> 2) << 10) & 0x7FFF;
        let (wide, tall) = (sc & 1 != 0, sc & 2 != 0);
        let nba = self.bg_nba[bg / 2] >> ((bg & 1) * 4) & 0x0F;
        let char_base = (nba as u16) << 12;
        let hofs = self.bg_hofs[bg] & 0x3FF;
        let vofs = self.bg_vofs[bg] & 0x3FF;
        let yy = y.wrapping_add(vofs) & 0x3FF;
        let words = bpp as u16 * 4;
        let palette_base: u8 = if self.bgmode & 7 == 0 {
            bg as u8 * 32
        } else {
            0
        };
        let colors = 1u16 << bpp;
        for x in 0..WIDTH as u16 {
            let xx = x.wrapping_add(hofs) & 0x3FF;
            let (tx, ty) = (xx / tile_px, yy / tile_px);
            let mut map = map_base + (ty & 31) * 32 + (tx & 31);
            if wide && tx & 32 != 0 {
                map += 0x400;
            }
            if tall && ty & 32 != 0 {
                map += if wide { 0x800 } else { 0x400 };
            }
            let entry = self.vram[(map & 0x7FFF) as usize];
            let (hflip, vflip) = (entry & 0x4000 != 0, entry & 0x8000 != 0);
            let mut px = xx % tile_px;
            let mut py = yy % tile_px;
            if hflip {
                px = tile_px - 1 - px;
            }
            if vflip {
                py = tile_px - 1 - py;
            }
            let mut tile = entry & 0x3FF;
            if big {
                tile = tile.wrapping_add((px / 8) + (py / 8) * 16) & 0x3FF;
            }
            let addr = char_base.wrapping_add(tile * words).wrapping_add(py % 8);
            let bit = 7 - (px % 8);
            let mut index = 0u16;
            for plane in 0..bpp as u16 {
                let w = self.vram[(addr.wrapping_add((plane / 2) * 8) & 0x7FFF) as usize];
                let byte = if plane % 2 == 0 {
                    w as u8
                } else {
                    (w >> 8) as u8
                };
                index |= (((byte >> bit) & 1) as u16) << plane;
            }
            if index == 0 {
                continue;
            }
            let palette = (entry >> 10) & 7;
            let color = if bpp == 8 {
                index as u8
            } else {
                palette_base.wrapping_add((palette * colors + index) as u8)
            };
            out[x as usize] = Px {
                color,
                priority: (entry >> 13) as u8 & 1,
                opaque: true,
                obj_math: true,
            };
        }
    }

    /// Mode 7's BG1 line.
    fn mode7_line(&self, y: u16, out: &mut [Px; WIDTH]) {
        let s13 = |v: u16| ((v << 3) as i16 >> 3) as i32;
        let clip = |v: i32| {
            if v & 0x2000 != 0 {
                v | !0x3FF
            } else {
                v & 0x3FF
            }
        };
        let a = self.m7[0] as i16 as i32;
        let b = self.m7[1] as i16 as i32;
        let c = self.m7[2] as i16 as i32;
        let d = self.m7[3] as i16 as i32;
        let x0 = s13(self.m7[4]);
        let y0 = s13(self.m7[5]);
        let hofs = s13(self.m7_hofs);
        let vofs = s13(self.m7_vofs);
        let yy = if self.m7sel & 2 != 0 {
            255 - y as i32
        } else {
            y as i32
        };
        let px0 = ((a * clip(hofs - x0)) & !63)
            + ((b * clip(vofs - y0)) & !63)
            + ((b * yy) & !63)
            + (x0 << 8);
        let py0 = ((c * clip(hofs - x0)) & !63)
            + ((d * clip(vofs - y0)) & !63)
            + ((d * yy) & !63)
            + (y0 << 8);
        for x in 0..WIDTH as i32 {
            let xx = if self.m7sel & 1 != 0 { 255 - x } else { x };
            let mx = (px0 + a * xx) >> 8;
            let my = (py0 + c * xx) >> 8;
            let outside = !(0..1024).contains(&mx) || !(0..1024).contains(&my);
            let tile = match self.m7sel >> 6 {
                2 | 3 if outside => {
                    if self.m7sel >> 6 == 2 {
                        continue;
                    }
                    0
                }
                _ => {
                    let (tx, ty) = ((mx & 1023) >> 3, (my & 1023) >> 3);
                    self.vram[(ty * 128 + tx) as usize] as u8
                }
            };
            let (fx, fy) = (mx & 7, my & 7);
            let color = (self.vram[(tile as i32 * 64 + fy * 8 + fx) as usize & 0x7FFF] >> 8) as u8;
            if color != 0 {
                out[x as usize] = Px {
                    color,
                    priority: 0,
                    opaque: true,
                    obj_math: true,
                };
            }
        }
    }

    /// Sprite sizes (width, height) for size bit 0 and 1.
    fn obj_sizes(&self) -> [(u16, u16); 2] {
        match self.obsel >> 5 {
            0 => [(8, 8), (16, 16)],
            1 => [(8, 8), (32, 32)],
            2 => [(8, 8), (64, 64)],
            3 => [(16, 16), (32, 32)],
            4 => [(16, 16), (64, 64)],
            5 => [(32, 32), (64, 64)],
            6 => [(16, 32), (32, 64)],
            _ => [(16, 32), (32, 32)],
        }
    }

    fn obj_line(&self, y: u16, out: &mut [Px; WIDTH]) {
        let sizes = self.obj_sizes();
        let base = ((self.obsel as u16 & 7) << 13) & 0x7FFF;
        let gap = (((self.obsel as u16 >> 3) & 3) + 1) << 12;
        let first = if self.oam_priority {
            (self.oam_reload & 0xFE) as usize >> 1
        } else {
            0
        };
        let mut count = 0;
        // Lower OAM indexes win: draw from the last so earlier ones overwrite.
        let mut list = Vec::with_capacity(32);
        for i in 0..128 {
            let n = (first + i) & 127;
            let e = &self.oam[n * 4..n * 4 + 4];
            let hi = self.oam[0x200 + n / 4] >> ((n % 4) * 2);
            let (w, h) = sizes[(hi >> 1) as usize & 1];
            let row = y.wrapping_sub(e[1] as u16) & 0xFF;
            if row >= h {
                continue;
            }
            let x = e[0] as u16 | ((hi as u16 & 1) << 8);
            let sx = if x & 0x100 != 0 {
                x as i32 - 512
            } else {
                x as i32
            };
            if sx + w as i32 <= 0 && sx != -256 {
                continue;
            }
            list.push((n, sx, row, w, h));
            count += 1;
            if count == 32 {
                break;
            }
        }
        for &(n, sx, row, w, h) in list.iter().rev() {
            let e = &self.oam[n * 4..n * 4 + 4];
            let attr = e[3];
            let (hflip, vflip) = (attr & 0x40 != 0, attr & 0x80 != 0);
            let r = if vflip { h - 1 - row } else { row };
            let tile0 = e[2] as u16;
            let table = if attr & 1 != 0 { base + gap } else { base };
            for c in 0..w {
                let x = sx + c as i32;
                if !(0..WIDTH as i32).contains(&x) {
                    continue;
                }
                let col = if hflip { w - 1 - c } else { c };
                let tile = ((tile0 & 0xF0) + ((r / 8) << 4)) & 0xF0 | ((tile0 + col / 8) & 0x0F);
                let addr = table.wrapping_add(tile * 16).wrapping_add(r % 8) & 0x7FFF;
                let bit = 7 - (col % 8);
                let mut index = 0u8;
                for plane in 0..4u16 {
                    let wd = self.vram[(addr.wrapping_add((plane / 2) * 8) & 0x7FFF) as usize];
                    let byte = if plane % 2 == 0 {
                        wd as u8
                    } else {
                        (wd >> 8) as u8
                    };
                    index |= ((byte >> bit) & 1) << plane;
                }
                if index == 0 {
                    continue;
                }
                let palette = (attr >> 1) & 7;
                out[x as usize] = Px {
                    color: 128 + palette * 16 + index,
                    priority: (attr >> 4) & 3,
                    opaque: true,
                    obj_math: palette >= 4,
                };
            }
        }
    }

    /// The layer order, front to back, as (layer, priority) for the current mode.
    fn order(&self) -> &'static [(usize, u8)] {
        match self.bgmode & 7 {
            0 => &[
                (OBJ, 3),
                (0, 1),
                (1, 1),
                (OBJ, 2),
                (0, 0),
                (1, 0),
                (OBJ, 1),
                (2, 1),
                (3, 1),
                (OBJ, 0),
                (2, 0),
                (3, 0),
            ],
            1 if self.bgmode & 8 != 0 => &[
                (2, 1),
                (OBJ, 3),
                (0, 1),
                (1, 1),
                (OBJ, 2),
                (0, 0),
                (1, 0),
                (OBJ, 1),
                (OBJ, 0),
                (2, 0),
            ],
            1 => &[
                (OBJ, 3),
                (0, 1),
                (1, 1),
                (OBJ, 2),
                (0, 0),
                (1, 0),
                (OBJ, 1),
                (2, 1),
                (OBJ, 0),
                (2, 0),
            ],
            7 => &[(OBJ, 3), (OBJ, 2), (OBJ, 1), (0, 0), (OBJ, 0)],
            _ => &[
                (OBJ, 3),
                (0, 1),
                (OBJ, 2),
                (1, 1),
                (OBJ, 1),
                (0, 0),
                (OBJ, 0),
                (1, 0),
            ],
        }
    }

    /// Composes one screen (main or sub) from the layer lines enabled in `mask`:
    /// (colour index or None for backdrop, layer index 0–4 or 5 = backdrop, math allowed).
    fn compose(&self, lines: &[[Px; WIDTH]; 5], mask: u8, x: usize) -> (Option<u8>, usize, bool) {
        for &(layer, prio) in self.order() {
            if mask & (1 << layer) == 0 {
                continue;
            }
            let px = lines[layer][x];
            if px.opaque
                && (layer == OBJ || self.bgmode & 7 == 7 || px.priority == prio)
                && (layer != OBJ || px.priority == prio)
            {
                return (Some(px.color), layer, px.obj_math);
            }
        }
        (None, 5, true)
    }

    /// Which layers (bits 0–3 BG1–BG4, 4 OBJ, 5 the colour window) the windows cover at
    /// `x`: each combines window 1 and 2 as its settings in $2123–$2125 and $212A–$212B say.
    fn windows_at(&self, x: usize) -> u8 {
        let x = x as u8;
        let inside = |w: usize| self.window[w * 2] <= x && x <= self.window[w * 2 + 1];
        let (w1, w2) = (inside(0), inside(1));
        let settings = [self.w12sel, self.w34sel, self.wobjsel];
        let logic = u16::from(self.wbglog) | u16::from(self.wobjlog) << 8;
        let mut bits = 0;
        for layer in 0..6 {
            let sel = settings[layer / 2] >> (layer % 2 * 4);
            let one = |on: bool, invert: u8| on != (invert != 0);
            let a = (sel & 2 != 0).then(|| one(w1, sel & 1));
            let b = (sel & 8 != 0).then(|| one(w2, sel & 4));
            let covered = match (a, b) {
                (None, None) => false,
                (Some(v), None) | (None, Some(v)) => v,
                (Some(a), Some(b)) => match (logic >> (layer * 2)) & 3 {
                    0 => a || b,
                    1 => a && b,
                    2 => a != b,
                    _ => a == b,
                },
            };
            if covered {
                bits |= 1 << layer;
            }
        }
        bits
    }

    /// Draws screen line `y` (0–223) into [`SnesPpu::frame`].
    pub fn draw_line(&mut self, y: usize) {
        let row = &mut [0u16; WIDTH];
        if self.inidisp & 0x80 == 0 {
            let v = y as u16 + 1;
            let mut lines = [[Px::default(); WIDTH]; 5];
            let layers = self.tm | self.ts;
            if self.bgmode & 7 == 7 {
                if layers & 1 != 0 {
                    self.mode7_line(v, &mut lines[0]);
                }
            } else {
                for (bg, &bpp) in self.bg_bpp().iter().enumerate() {
                    if bpp != 0 && layers & (1 << bg) != 0 {
                        self.bg_line(bg, bpp, v, &mut lines[bg]);
                    }
                }
            }
            if layers & 0x10 != 0 {
                self.obj_line(y as u16, &mut lines[OBJ]);
            }
            let brightness = (self.inidisp & 0x0F) as u32;
            // Where the colour window is: never, outside it, inside it, always.
            let applies = |mode: u8, inside: bool| match mode & 3 {
                0 => false,
                1 => !inside,
                2 => inside,
                _ => true,
            };
            for (x, out) in row.iter_mut().enumerate() {
                let windows = self.windows_at(x);
                let main_layers = self.tm & !(self.tmw & windows);
                let sub_layers = self.ts & !(self.tsw & windows);
                let in_color_window = windows & 0x20 != 0;
                let (main, layer, obj_math) = self.compose(&lines, main_layers, x);
                let mut color = main.map_or(self.cgram[0], |c| self.cgram[c as usize]);
                // "Clip to black" ($2130 bits 6–7), then math unless prevented (bits 4–5).
                let clipped = applies(self.cgwsel >> 6, in_color_window);
                if clipped {
                    color = 0;
                }
                let math_on = self.cgadsub & (1 << layer) != 0
                    && obj_math
                    && !applies(self.cgwsel >> 4, in_color_window);
                if math_on {
                    let (sub, half) = if self.cgwsel & 2 != 0 {
                        match self.compose(&lines, sub_layers, x) {
                            (Some(c), _, _) => (self.cgram[c as usize], true),
                            (None, _, _) => (self.coldata, false),
                        }
                    } else {
                        (self.coldata, true)
                    };
                    color = blend(
                        color,
                        sub,
                        self.cgadsub & 0x80 != 0,
                        // Halving is skipped where the main screen was clipped.
                        half && self.cgadsub & 0x40 != 0 && !clipped,
                    );
                }
                *out = if brightness == 15 {
                    color
                } else {
                    let ch = |c: u16| ((c as u32 & 31) * (brightness + 1) / 16) as u16;
                    ch(color) | ch(color >> 5) << 5 | ch(color >> 10) << 10
                };
            }
        }
        self.frame[y * WIDTH..(y + 1) * WIDTH].copy_from_slice(row);
    }
}

/// Colour math on two BGR555 colours.
fn blend(a: u16, b: u16, subtract: bool, half: bool) -> u16 {
    let mut out = 0;
    for shift in [0, 5, 10] {
        let (x, y) = ((a >> shift) as i32 & 31, (b >> shift) as i32 & 31);
        let mut v = if subtract { (x - y).max(0) } else { x + y };
        if half {
            v >>= 1;
        }
        out |= (v.min(31) as u16) << shift;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_a_mode1_tile_and_a_sprite_over_it() {
        let mut ppu = SnesPpu::new();
        ppu.write(0x00, 0x0F); // display on, full brightness
        ppu.write(0x05, 0x01); // mode 1
        ppu.write(0x07, 0x04); // BG1 map at word $0400
        ppu.write(0x0B, 0x01); // BG1 characters at word $1000
        ppu.write(0x2C, 0x11); // BG1 and OBJ on the main screen
        // BG1 tile 1: row 1 all colour 1 (plane 0 set); picture line 0 is scanline 1,
        // which shows BG row 1 when not scrolled.
        ppu.vram[0x1000 + 16 + 1] = 0x00FF;
        // Map entry (0, 0) = tile 1, palette 0.
        ppu.vram[0x0400] = 1;
        ppu.cgram[0] = 0x1111;
        ppu.cgram[1] = 0x001F; // red
        // Sprite 0 at (4, 0), 8×8, tile 0 at OBJ base $0000 (word $0000), palette 0.
        ppu.vram[0] = 0x00FF; // row 0, colour 1
        ppu.cgram[129] = 0x7C00; // blue
        ppu.oam[0] = 4;
        ppu.oam[1] = 0;
        ppu.oam[2] = 0;
        ppu.oam[3] = 0x30; // priority 3
        for i in 1..128 {
            ppu.oam[i * 4 + 1] = 0xF0; // off screen
        }
        ppu.draw_line(0);
        assert_eq!(ppu.frame[0], 0x001F, "BG1 tile");
        assert_eq!(ppu.frame[4], 0x7C00, "sprite in front");
        assert_eq!(ppu.frame[3], 0x001F, "BG1 left of the sprite");
        assert_eq!(ppu.frame[8], 0x7C00);
        assert_eq!(ppu.frame[20], 0x1111, "backdrop");
        ppu.write(0x00, 0x07); // half brightness
        ppu.draw_line(0);
        assert_eq!(ppu.frame[0], 15, "red at half brightness");
    }

    #[test]
    fn windows_mask_layers_and_clip_colours() {
        let mut ppu = SnesPpu::new();
        ppu.write(0x00, 0x0F);
        ppu.write(0x05, 0x01);
        ppu.write(0x07, 0x04);
        ppu.write(0x0B, 0x01);
        ppu.write(0x2C, 0x01); // BG1 on the main screen
        for x in 0..32 {
            ppu.vram[0x0400 + x] = 1; // a row of tile 1
        }
        ppu.vram[0x1000 + 16 + 1] = 0x00FF; // tile 1, row 1: colour 1
        ppu.cgram[0] = 0x1111;
        ppu.cgram[1] = 0x001F;
        // Window 1 over x 10–20, masking BG1 on the main screen.
        ppu.write(0x26, 10);
        ppu.write(0x27, 20);
        ppu.write(0x23, 0x02);
        ppu.write(0x2E, 0x01);
        ppu.draw_line(0);
        assert_eq!(ppu.frame[9], 0x001F);
        assert_eq!((ppu.frame[10], ppu.frame[20]), (0x1111, 0x1111), "masked");
        assert_eq!(ppu.frame[21], 0x001F);
        // Inverted: only 10–20 show BG1.
        ppu.write(0x23, 0x03);
        ppu.draw_line(0);
        assert_eq!((ppu.frame[9], ppu.frame[15]), (0x1111, 0x001F));
        // Window 2 over 15–30, OR: BG1 masked in 15–30 too.
        ppu.write(0x28, 15);
        ppu.write(0x29, 30);
        ppu.write(0x23, 0x0B);
        ppu.draw_line(0);
        assert_eq!(ppu.frame[15], 0x1111, "inside window 2");
        assert_eq!(ppu.frame[12], 0x001F, "outside both");
        // AND: masked only where both cover it (window 1 inverted: outside 10–20).
        ppu.write(0x2A, 0x01);
        ppu.draw_line(0);
        assert_eq!((ppu.frame[15], ppu.frame[25]), (0x001F, 0x1111));
        // Colour window = window 1, clip to black inside it.
        ppu.write(0x2E, 0x00);
        ppu.write(0x25, 0x20);
        ppu.write(0x30, 0x80);
        ppu.draw_line(0);
        assert_eq!((ppu.frame[9], ppu.frame[10]), (0x001F, 0x0000));
    }
}
