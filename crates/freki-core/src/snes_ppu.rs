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
}

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
}
