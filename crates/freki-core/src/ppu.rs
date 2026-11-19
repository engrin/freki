//! The Pixel Processing Unit: LCD timing, STAT interrupts and pixel-by-pixel rendering.
//!
//! Mode 3 (drawing) is emulated dot by dot, following the DMG pixel pipeline as measured
//! by the mealybug-tearoom tests and documented by SameBoy:
//!
//! - Each line: the mode 2 interrupt fires at dot 0, STAT reports mode 3 from dot 81 and
//!   the pixel pipeline starts at dot 86.
//! - The **background FIFO** starts with 8 junk pixels and the pipeline's position counter
//!   at −16. Pixels at negative positions are dropped; this is where `SCX % 8` fine
//!   scrolling and sprites partly off the left edge are handled.
//! - The **fetcher** works in 2-dot steps (address on the first dot, VRAM read on the
//!   second) and pushes a tile row once the FIFO is empty.
//! - The **window** restarts the fetcher when `WX` matches the position; **sprites** pause
//!   the pipeline while their tile row is fetched.
//!
//! In Game Boy Color mode the fetcher also reads each tile's attributes from VRAM bank 1
//! (palette, tile bank, flips, priority), sprites pick their palette and tile bank from
//! OAM, and pixels are coloured through palette RAM into an RGB555 framebuffer.
//!
//! Registers are read at the dot they are used, so mid-scanline changes show up exactly
//! where they happen. Some DMG register writes land slightly off the CPU's M-cycle grid;
//! the CPU handles that (see `Cpu::mem_write`).

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use std::collections::VecDeque;

use crate::bus::interrupt;

pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;

const DOTS_PER_LINE: u32 = 456;
const LINES_PER_FRAME: u8 = 154;
/// STAT reports mode 3 from this dot (visible to the CPU 80 dots after the mode 2
/// interrupt, as on hardware).
const MODE3_STAT_DOT: u32 = 79;
/// OAM scan has checked all 40 entries; selected sprites are sorted for mode 3.
const SPRITE_SORT_DOT: u32 = 81;
/// First dot of the pixel pipeline.
const PIPELINE_START_DOT: u32 = 86;
/// CGB: dot of line 143 at which entering VBlank triggers the OAM STAT interrupt
/// (Mooneye's vblank_stat_intr-C allows 448–451; the DMG fires it at line 144, dot 0).
const CGB_VBLANK_OAM_INT_DOT: u32 = 450;
/// Most dots by which mode 0 in STAT can lead the end of the pipeline (`mode0_lead`).
const MAX_MODE0_LEAD: u32 = 5;
/// The dot at which the next line's mode 2 interrupt is found blocked (by the LYC or
/// HBlank source) or not; later STAT/LYC writes do not change that (Gambatte's hwtests,
/// m2enable/*late_lycdisable*, *late_m0disable*).
const M2_LATCH_DOT: u32 = 453;
/// The mode 2 source only holds the STAT line high for the first M-cycle of mode 2: an
/// LYC source enabled later in mode 2 still raises an interrupt (Gambatte's hwtests,
/// lycEnable/late_ff41_enable_after_m2int).
const OAM_PULSE_DOTS: u32 = 4;
/// The HBlank source drops this many dots into the line (before line 143's end only at
/// the line change), so an LYC match starting with the next line raises an interrupt.
/// The mode 2 interrupt and LYC matches under an enabled mode 2 source stay blocked
/// explicitly (Gambatte's hwtests, lcdirq_precedence/*, miscmstatirq/*; Mooneye's
/// stat_irq_blocking).
const HBLANK_SOURCE_END: u32 = 454;
/// Dots until a STAT or LYC write reaches the STAT interrupt logic: DMG, CGB, CGB in
/// double speed (Gambatte's hwtests; Gambatte has the same 2-cycle STAT lag on the CGB).
const STAT_IRQ_DELAY: (u8, u8, u8) = (0, 2, 1);
const LYC_IRQ_DELAY: (u8, u8, u8) = (0, 2, 1);
/// Setting LYC to the next LY within this many dots of the line's end keeps the LYC
/// source high across the line change (no interrupt), except within the last `TAIL`
/// dots: DMG, CGB, double speed (Gambatte's hwtests, lycEnable/*late_ff45_enable*,
/// *weirdpoint*).
const LYC_FOLLOW_WINDOW: (u8, u8, u8) = (4, 8, 4);
const LYC_FOLLOW_TAIL: (u8, u8, u8) = (0, 4, 0);
/// LY and dot at which line 0's mode 2 interrupt is decided (normal and double speed;
/// Gambatte's hwtests, m2enable/*ly0*).
const LY0_OAM_INT_LATCH: (u8, u32) = (153, 455);
const LY0_OAM_INT_LATCH_DS: (u8, u32) = (0, 0);
/// CGB palette RAM locks one dot after STAT shows mode 3 and unlocks 4 dots after it
/// shows mode 0 (Gambatte's hwtests, cgbpal_m3/*).
const PALETTE_LOCK_DOT: u32 = 80;
const PALETTE_UNLOCK_DELAY: u8 = 4;
/// Dots from a STAT/LYC write that raises the STAT line to the interrupt request: DMG,
/// CGB, CGB in double speed (fitted to Gambatte's hwtests).
const WRITE_IRQ_DMG: u8 = 5;
const WRITE_IRQ_CGB: u8 = 5;
const WRITE_IRQ_DS: u8 = 3;
/// The dot of line 0 at which the frame's WY latch is reset and checked.
const WY_LINE0_DOT: u32 = 1;
const WY2_DMG: u8 = 8;
const WY2_CGB: u8 = 12;
const WY2_CGB_DS: u8 = 6;
/// Dots of each line at which WY is compared with LY, and with LY + 1.
const WY_CHECK_DOT: u32 = 451;
const WY_CHECK_NEXT_DOT: u32 = 455;
const MAX_SPRITES_PER_LINE: usize = 10;
/// The pipeline's position counter starts at −16 (as an 8-bit value).
const POSITION_START: u8 = 0u8.wrapping_sub(16);

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

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    HBlank = 0,
    VBlank = 1,
    OamScan = 2,
    Drawing = 3,
}

/// Background fetcher state. VRAM accesses take two dots: the address is formed on the
/// first (`*1`) and the data read on the second (`*2`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
enum Fetch {
    #[default]
    Tile1,
    Tile2,
    Low1,
    Low2,
    High1,
    High2,
    Push,
}

/// Dots pixel output stays paused after objects are disabled while a sprite fetch waits
/// for the background fetcher (DMG; Gambatte's hwtests allow 2–5).
const ABORTED_OBJ_STALL: u8 = 3;

/// Progress of a sprite fetch, which pauses pixel output.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ObjFetch {
    #[default]
    Idle,
    /// Waiting for the background fetcher to finish its tile.
    Waiting,
    Attributes,
    Attributes2,
    Low,
    Low2,
    High,
    /// Objects were disabled during the fetch: the sprite is dropped, but the fetcher
    /// stays stalled for this many more dots.
    Aborted(u8),
}

/// A sprite selected during OAM scan for the current line.
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
struct Sprite {
    y: u8,
    x: u8,
    tile: u8,
    attr: u8,
    /// OAM entry (0–39): decides priority between overlapping sprites on the CGB.
    index: u8,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
struct ObjPixel {
    color: u8,
    palette1: bool,
    behind_bg: bool,
    /// CGB palette (0–7) and OAM entry of the sprite the pixel came from.
    cgb_palette: u8,
    index: u8,
}

/// Background FIFO entries: colour index in bits 0–1; in CGB mode also the palette in
/// bits 2–4 and the BG-over-OBJ priority attribute in bit 7.
mod bg_pixel {
    pub const COLOR: u8 = 0x03;
    pub const PALETTE_SHIFT: u8 = 2;
    pub const PRIORITY: u8 = 0x80;
}

/// CGB tile attributes (VRAM bank 1 tile maps; bits 3–6 also apply to sprites in OAM).
mod attr {
    pub const PALETTE: u8 = 0x07;
    pub const BANK1: u8 = 1 << 3;
    pub const X_FLIP: u8 = 1 << 5;
    pub const Y_FLIP: u8 = 1 << 6;
    pub const PRIORITY: u8 = 1 << 7;
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Ppu {
    /// Game Boy Color mode (colour palettes, VRAM bank 1, tile attributes).
    cgb: bool,
    /// CGB hardware in DMG compatibility mode: DMG rendering, coloured through palette RAM.
    dmg_compat: bool,
    /// OPRI ($FF6C) bit 0: sprites overlap by X like on the DMG instead of by OAM index.
    opri_by_x: bool,
    /// The CPU runs at double speed (CGB), so it sees the PPU at a finer phase.
    #[serde(default)]
    pub(crate) double_speed: bool,
    /// An OAM DMA has OAM: the OAM scan reads 0xFF and finds no sprites (Gambatte's
    /// hwtests, oamdma/late_sp*).
    #[serde(default)]
    pub(crate) oam_blocked: bool,
    /// VRAM: bank 0 at 0–$1FFF, bank 1 (CGB) at $2000–$3FFF.
    #[serde(with = "BigArray")]
    vram: [u8; 0x4000],
    /// VBK ($FF4F): VRAM bank the CPU sees (CGB).
    vbk: u8,
    /// CGB palette RAM: 8 palettes × 4 colours × 2 bytes (RGB555, little-endian).
    #[serde(with = "BigArray")]
    bg_palettes: [u8; 64],
    #[serde(with = "BigArray")]
    obj_palettes: [u8; 64],
    /// BCPS/OCPS ($FF68/$FF6A): palette RAM index and auto-increment flag (bit 7).
    bcps: u8,
    ocps: u8,
    #[serde(with = "BigArray")]
    oam: [u8; 0xA0],
    #[serde(with = "BigArray")]
    framebuffer: [u8; SCREEN_WIDTH * SCREEN_HEIGHT],
    /// CGB mode output: RGB555 colours.
    rgb_framebuffer: Vec<u16>,
    frame_ready: bool,
    /// Like `frame_ready`, for the bus (the Super Game Boy picks up each frame).
    #[serde(skip)]
    frame_done: bool,
    /// Dot position within the current scanline.
    dot: u32,
    /// Mode as reported by STAT.
    mode: Mode,
    /// Mode as seen by the STAT interrupt logic (HBlank's interrupt lags by one dot).
    irq_mode: Mode,
    /// Previous level of the combined STAT interrupt line (interrupts fire on rising edges).
    stat_line: bool,
    /// Dots until a STAT interrupt raised by a register write is requested (0: none).
    #[serde(default)]
    write_irq_in: u8,
    /// STAT and LYC as the interrupt logic sees them: writes reach it a few dots late.
    #[serde(default)]
    irq_stat: u8,
    #[serde(default)]
    irq_lyc: u8,
    /// A STAT / LYC write on its way to `irq_stat` / `irq_lyc`: (value, dots left).
    #[serde(default)]
    stat_pending: (u8, u8),
    #[serde(default)]
    lyc_pending: (u8, u8),
    /// LYC followed LY across the line change (see the LYC write): no rising edge then.
    #[serde(default)]
    lyc_edge_masked: bool,
    /// Dots after STAT's switch to mode 0 until palette RAM unlocks.
    #[serde(default)]
    palette_unlock_in: u8,
    /// The OAM (mode 2) interrupt was enabled when mode 2 started: the source only counts
    /// then, so enabling it later in mode 2 does not request an interrupt.
    oam_int_armed: bool,
    /// Whether the next line's mode 2 interrupt is not blocked (`M2_LATCH_DOT`); for line
    /// 0 also whether it is enabled (`LY0_OAM_INT_LATCH`).
    #[serde(default)]
    m2_int_unblocked: bool,
    /// LCDC's OBJ size bit over the last dots (bit 0 = this dot): OAM scan sees a change
    /// three dots late (Gambatte's hwtests, sprites/late_sizechange*).
    obj_size_history: u8,
    /// STAT's coincidence flag as it was when the LCD was switched off (it is frozen
    /// while the LCD is off).
    lyc_flag_off: bool,
    /// Set when mode 3 of a visible line ends (starts HBlank DMA on the CGB).
    #[serde(skip)]
    hblank_started: bool,
    /// The dot of the line at which the last HBlank began.
    hblank_start_dot: u32,
    /// The first line after the LCD is switched on: it has no mode 2 (STAT goes from mode 0
    /// straight to mode 3 and OAM stays accessible).
    first_line: bool,

    // ---- pixel pipeline (mode 3) ----
    pipeline_active: bool,
    bg_fifo: VecDeque<u8>,
    obj_fifo: VecDeque<ObjPixel>,
    /// Pipeline position, starting at −16 each line; pixels are drawn at 0–159.
    position: u8,
    /// Next LCD column to draw.
    lcd_x: u8,
    fetch: Fetch,
    tile_map_addr: usize,
    tile: u8,
    /// CGB attributes of the tile being fetched (0 on the DMG).
    tile_attr: u8,
    tile_data_addr: usize,
    tile_data: [u8; 2],
    sprites: Vec<Sprite>,
    next_sprite: usize,
    /// Pipeline paused for one dot (window start with WX = 0 and fine scrolling).
    window_stall: bool,
    obj_fetch: ObjFetch,
    obj_attr: [u8; 2],
    obj_low: u8,
    during_object_fetch: bool,

    // ---- window ----
    /// LY matched WY this frame (while the window was enabled).
    wy_triggered: bool,
    /// The window is being drawn on this line.
    wx_triggered: bool,
    window_tile_x: u8,
    /// Background tiles fetched on this line (the fetcher adds SCX's upper 5 bits to it).
    bg_tile_x: u8,
    /// Window line counter; starts at −1 each frame and counts window activations.
    window_y: u8,
    window_being_fetched: bool,
    insert_bg_pixel: bool,
    disable_insertion_glitch: bool,
    /// Interrupts raised in the DMG's last dot before the LCD switched off, not yet
    /// requested.
    #[serde(default)]
    off_irq: u8,
    /// Set while `check_write_irq` asks `stat_sources` for every source but LYC.
    #[serde(skip)]
    exclude_lyc_source: bool,
    /// Mode 0 in STAT comes one dot later on this line (see `check_window`).
    #[serde(default)]
    late_mode0: bool,
    /// DMG: dots until objects just enabled in LCDC can start a sprite fetch.
    #[serde(default)]
    obj_enable_delay: u8,
    wx_just_changed: bool,
    /// This copy only looks ahead to find when mode 3 ends (see `mode0_due`).
    #[serde(skip)]
    lookahead: bool,

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
    /// The WX match sees WY a few dots late: (old value, dots it is still seen).
    #[serde(default)]
    wy_for_wx: (u8, u8),
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
            cgb: false,
            dmg_compat: false,
            opri_by_x: true,
            double_speed: false,
            oam_blocked: false,
            vram: [0; 0x4000],
            vbk: 0,
            bg_palettes: [0xFF; 64],
            obj_palettes: [0xFF; 64],
            bcps: 0,
            ocps: 0,
            oam: [0; 0xA0],
            framebuffer: [0; SCREEN_WIDTH * SCREEN_HEIGHT],
            rgb_framebuffer: vec![0x7FFF; SCREEN_WIDTH * SCREEN_HEIGHT],
            frame_ready: false,
            frame_done: false,
            dot: 0,
            mode: Mode::OamScan,
            irq_mode: Mode::OamScan,
            stat_line: false,
            write_irq_in: 0,
            palette_unlock_in: 0,
            irq_stat: 0,
            irq_lyc: 0,
            stat_pending: (0, 0),
            lyc_pending: (0, 0),
            lyc_edge_masked: false,
            oam_int_armed: false,
            m2_int_unblocked: false,
            obj_size_history: 0,
            lyc_flag_off: false,
            hblank_started: false,
            hblank_start_dot: 0,
            first_line: false,
            pipeline_active: false,
            bg_fifo: VecDeque::with_capacity(16),
            obj_fifo: VecDeque::with_capacity(16),
            position: POSITION_START,
            lcd_x: 0,
            fetch: Fetch::Tile1,
            tile_map_addr: 0,
            tile: 0,
            tile_attr: 0,
            tile_data_addr: 0,
            tile_data: [0; 2],
            sprites: Vec::with_capacity(MAX_SPRITES_PER_LINE),
            next_sprite: 0,
            window_stall: false,
            obj_fetch: ObjFetch::Idle,
            obj_attr: [0; 2],
            obj_low: 0,
            during_object_fetch: false,
            wy_triggered: false,
            wx_triggered: false,
            window_tile_x: 0,
            bg_tile_x: 0,
            window_y: 0xFF,
            window_being_fetched: false,
            insert_bg_pixel: false,
            disable_insertion_glitch: false,
            obj_enable_delay: 0,
            late_mode0: false,
            off_irq: 0,
            exclude_lyc_source: false,
            wx_just_changed: false,
            lookahead: false,
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
            wy_for_wx: (0, 0),
            wx: 0,
        }
    }

    /// Recreates the VRAM contents the DMG boot ROM leaves behind: the cartridge header's
    /// logo (`$0104–$0133`) decompressed into tiles 1–24, the ® symbol in tile 25, and
    /// their tile map entries. Some games and tests rely on these tiles being there.
    /// Moves the PPU to where the DMG boot ROM leaves it when it hands over to the game.
    /// Running the real boot ROM in freki ends at line 153, dot 397; dot 396 is used
    /// because Mooneye's hblank_ly_scx_timing needs lines aligned with M-cycles. (So after
    /// a real boot ROM the PPU runs one dot ahead of hardware; the cause is not found yet.)
    pub fn skip_to_post_boot(&mut self) {
        self.ly = LINES_PER_FRAME - 1;
        self.dot = 396;
        self.mode = Mode::VBlank;
        self.irq_mode = Mode::VBlank;
    }

    /// Where the CGB boot ROM leaves the PPU for a CGB game (from Gambatte's hwtests,
    /// display_startstate/*_cgb04c) or, with `compat`, a DMG game (measured with the real
    /// boot ROM), with the palette indexes and OBJ palettes it wrote.
    pub fn skip_to_post_boot_cgb(&mut self, compat: bool) {
        (self.ly, self.dot) = if compat { (148, 364) } else { (144, 168) };
        self.mode = Mode::VBlank;
        self.irq_mode = Mode::VBlank;
        (self.bcps, self.ocps) = if compat { (0x88, 0x90) } else { (0x80, 0x81) };
        self.obp0 = 0;
        self.obp1 = 0;
    }

    /// Power-on state: LCD off, palettes and VRAM cleared.
    pub fn power_on() -> Self {
        Self {
            lcdc: 0,
            bgp: 0,
            obp0: 0,
            obp1: 0,
            mode: Mode::HBlank,
            irq_mode: Mode::HBlank,
            ..Self::new()
        }
    }

    pub fn load_boot_logo(&mut self, logo: &[u8; 48]) {
        /// Doubles each bit of a nibble horizontally: 0b1011 -> 0b11001111.
        fn widen(nibble: u8) -> u8 {
            (0..4).fold(0, |acc, bit| {
                acc | (((nibble >> bit) & 1) * 0b11) << (bit * 2)
            })
        }
        // Each logo byte becomes 4 tile rows: one nibble per two (doubled) rows,
        // written to the low bitplane only.
        for (i, &byte) in logo.iter().enumerate() {
            for (half, nibble) in [byte >> 4, byte & 0x0F].into_iter().enumerate() {
                let row = widen(nibble);
                let addr = 0x10 + i * 8 + half * 4;
                self.vram[addr] = row;
                self.vram[addr + 2] = row;
            }
        }
        const REGISTERED: [u8; 8] = [0x3C, 0x42, 0xB9, 0xA5, 0xB9, 0xA5, 0x42, 0x3C];
        for (i, &row) in REGISTERED.iter().enumerate() {
            self.vram[0x190 + i * 2] = row;
        }
        // Tile map: tiles 1–12 at $9904, 13–24 at $9924, ® at $9910.
        for i in 0..12u8 {
            self.vram[0x1904 + i as usize] = i + 1;
            self.vram[0x1924 + i as usize] = i + 13;
        }
        self.vram[0x1910] = 0x19;
    }

    pub fn framebuffer(&self) -> &[u8; SCREEN_WIDTH * SCREEN_HEIGHT] {
        &self.framebuffer
    }

    /// Returns `true` once per completed frame.
    /// The screen in CGB mode, as RGB555 colours (bits 0–4 red, 5–9 green, 10–14 blue).
    pub fn rgb_framebuffer(&self) -> &[u16] {
        &self.rgb_framebuffer
    }

    /// Whether an HBlank began since the last call.
    /// The dot of the line at which the last HBlank began.
    pub fn hblank_start_dot(&self) -> u32 {
        self.hblank_start_dot
    }

    pub fn take_hblank_started(&mut self) -> bool {
        std::mem::take(&mut self.hblank_started)
    }

    pub fn is_cgb(&self) -> bool {
        self.cgb
    }

    pub fn take_frame_done(&mut self) -> bool {
        std::mem::take(&mut self.frame_done)
    }

    pub fn take_frame_ready(&mut self) -> bool {
        std::mem::take(&mut self.frame_ready)
    }

    /// The current line (LY's internal counter) and dot within it.
    pub fn line_and_dot(&self) -> (u8, u32) {
        (self.ly, self.dot)
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Dots left in the current line during a visible line's HBlank, else `None`.
    pub fn hblank_dots_left(&self) -> Option<u32> {
        (self.mode == Mode::HBlank && self.irq_mode == Mode::HBlank && self.lcd_on())
            .then(|| DOTS_PER_LINE.saturating_sub(self.dot))
    }

    /// Switches to Game Boy Color mode.
    pub fn set_cgb(&mut self) {
        self.cgb = true;
        self.opri_by_x = false;
    }

    /// Switches CGB hardware to DMG compatibility mode (see `Bus::enter_dmg_compat`).
    pub fn enter_dmg_compat(&mut self) {
        self.cgb = false;
        self.dmg_compat = true;
        self.opri_by_x = true;
        self.vbk = 0;
    }

    /// Fills the palettes DMG compatibility mode uses with greys, as a neutral choice when
    /// no CGB boot ROM picks colours.
    pub fn set_grey_compat_palettes(&mut self) {
        let greys: [u16; 4] = [0x7FFF, 0x56B5, 0x294A, 0x0000];
        for (ram, palettes) in [(&mut self.bg_palettes, 1), (&mut self.obj_palettes, 2)] {
            for palette in 0..palettes {
                for (i, grey) in greys.iter().enumerate() {
                    let at = palette * 8 + i * 2;
                    ram[at..at + 2].copy_from_slice(&grey.to_le_bytes());
                }
            }
        }
    }

    /// Whether this is DMG hardware (some PPU quirks are DMG-only).
    fn dmg_hardware(&self) -> bool {
        !self.cgb && !self.dmg_compat
    }

    /// Whether the PPU outputs colours (CGB hardware) rather than DMG shades.
    pub fn outputs_rgb(&self) -> bool {
        self.cgb || self.dmg_compat
    }

    /// The VRAM bank the CPU sees at $8000–$9FFF (always 0 on the original Game Boy).
    pub fn vram_bank(&self) -> usize {
        usize::from(self.vbk & 1)
    }

    fn vram_index(&self, addr: u16) -> usize {
        (self.vbk as usize & 1) * 0x2000 + (addr as usize - 0x8000)
    }

    pub fn read_vram(&self, addr: u16) -> u8 {
        self.vram[self.vram_index(addr)]
    }
    pub fn write_vram(&mut self, addr: u16, value: u8) {
        let i = self.vram_index(addr);
        self.vram[i] = value;
    }

    /// A VRAM byte from a given bank (0 or 1), regardless of VBK (for debuggers).
    pub fn vram_in_bank(&self, bank: u8, addr: u16) -> u8 {
        self.vram[(bank as usize & 1) * 0x2000 + (addr as usize & 0x1FFF)]
    }

    /// CGB palette RAM: background and object palettes (8 × 4 colours, RGB555 LE).
    pub fn palette_ram(&self) -> (&[u8; 64], &[u8; 64]) {
        (&self.bg_palettes, &self.obj_palettes)
    }

    /// Whether CGB rendering features (attributes, palettes, VRAM bank 1) are in use.
    pub fn cgb_mode(&self) -> bool {
        self.cgb
    }

    /// Whether palette RAM is locked: from shortly after mode 3 starts until a few dots
    /// after STAT shows mode 0 (Gambatte's hwtests, cgbpal_m3/*).
    fn palette_locked(&self) -> bool {
        self.lcd_on()
            && match self.mode {
                Mode::Drawing => self.dot >= PALETTE_LOCK_DOT,
                Mode::HBlank => self.palette_unlock_in > 0,
                _ => false,
            }
    }

    /// Reads palette RAM through BCPD/OCPD (inaccessible while drawing).
    fn read_palette(&self, obj: bool) -> u8 {
        if self.palette_locked() {
            return 0xFF;
        }
        let (ram, index) = if obj {
            (&self.obj_palettes, self.ocps)
        } else {
            (&self.bg_palettes, self.bcps)
        };
        ram[(index & 0x3F) as usize]
    }

    /// Writes palette RAM through BCPD/OCPD; the index auto-increments if enabled (even
    /// when the write is blocked during drawing).
    fn write_palette(&mut self, obj: bool, value: u8) {
        let accessible = !self.palette_locked();
        let (ram, index) = if obj {
            (&mut self.obj_palettes, &mut self.ocps)
        } else {
            (&mut self.bg_palettes, &mut self.bcps)
        };
        if accessible {
            ram[(*index & 0x3F) as usize] = value;
        }
        if *index & 0x80 != 0 {
            *index = 0x80 | (index.wrapping_add(1) & 0x3F);
        }
    }
    pub fn read_oam(&self, addr: u16) -> u8 {
        self.oam[(addr - 0xFE00) as usize]
    }
    pub fn write_oam(&mut self, addr: u16, value: u8) {
        self.oam[(addr - 0xFE00) as usize] = value;
    }

    /// Whether the CPU can read OAM: not during OAM scan and drawing, nor in the last
    /// 4 dots before a visible line (its OAM scan locks OAM early).
    pub fn cpu_oam_readable(&self) -> bool {
        let next_line_visible = (self.ly as usize) < SCREEN_HEIGHT - 1;
        !self.lcd_on()
            || (matches!(self.mode, Mode::HBlank | Mode::VBlank)
                && !(self.ly_changing() && next_line_visible))
    }

    /// Whether the CPU can write OAM: not while drawing, nor during OAM scan except in
    /// its last M-cycle.
    pub fn cpu_oam_writable(&self) -> bool {
        !self.lcd_on()
            || match self.mode {
                Mode::HBlank | Mode::VBlank => true,
                Mode::OamScan => self.dot >= MODE3_STAT_DOT - 3,
                Mode::Drawing => false,
            }
    }

    /// Whether the CPU can read VRAM: not while drawing, nor in the last M-cycle of OAM
    /// scan.
    pub fn cpu_vram_readable(&self) -> bool {
        !self.lcd_on()
            || match self.mode {
                Mode::HBlank | Mode::VBlank => true,
                Mode::OamScan => self.dot < MODE3_STAT_DOT - 3,
                Mode::Drawing => false,
            }
    }

    /// Whether the CPU can write VRAM (not while drawing).
    pub fn cpu_vram_writable(&self) -> bool {
        !self.lcd_on() || self.mode != Mode::Drawing
    }

    pub fn lcd_enabled(&self) -> bool {
        self.lcd_on()
    }

    fn lcd_on(&self) -> bool {
        self.lcdc & lcdc::LCD_ENABLE != 0
    }

    /// Whether LY, as compared with LYC for the STAT interrupt, equals `lyc`. Line 153
    /// compares as 153 for its first 4 dots and then as 0, so LYC = 0 matches during the last
    /// line of VBlank; at normal speed there is a gap of a few dots in between where neither
    /// matches (Gambatte's hwtests, ly0/lycint152_lyc0*, lycint152_lyc153flag_*).
    fn ly_matches(&self, lyc: u8) -> bool {
        if self.ly != LINES_PER_FRAME - 1 {
            return self.ly == lyc;
        }
        let zero_from = if self.double_speed {
            4
        } else if self.dmg_hardware() {
            6
        } else {
            5
        };
        if self.dot < 4 {
            lyc == self.ly
        } else {
            self.dot >= zero_from && lyc == 0
        }
    }

    /// Whether the line is in its last 4 dots, when LY already reads as the next line and
    /// the STAT coincidence flag reads as clear (except after line 153, whose LY is already 0).
    fn ly_changing(&self) -> bool {
        self.dot >= DOTS_PER_LINE - 4 && self.ly != LINES_PER_FRAME - 1
    }

    /// STAT's LY = LYC coincidence flag (bit 2).
    fn lyc_flag(&self) -> bool {
        if self.lcd_on() {
            !self.ly_changing() && self.ly_matches(self.lyc)
        } else {
            self.lyc_flag_off
        }
    }

    /// LY as read by the CPU.
    fn ly_register(&self) -> u8 {
        // At normal speed LY reads 153 only in line 152's last 4 dots (Gambatte's hwtests,
        // ly0/lycint152_ly153_*); the LYC comparison still sees 153 for 4 dots.
        if self.ly == LINES_PER_FRAME - 1 && !self.double_speed {
            0
        } else if self.ly_changing() {
            self.ly + 1
        } else if self.ly == LINES_PER_FRAME - 1 && self.dot >= 4 {
            0
        } else {
            self.ly
        }
    }

    pub fn read_reg(&self, addr: u16) -> u8 {
        match addr {
            0xFF40 => self.lcdc,
            0xFF41 => {
                let coincidence = u8::from(self.lyc_flag()) << 2;
                let mode = if self.lcd_on() { self.mode as u8 } else { 0 };
                0x80 | self.stat | coincidence | mode
            }
            0xFF42 => self.scy,
            0xFF43 => self.scx,
            0xFF44 => self.ly_register(),
            0xFF45 => self.lyc,
            0xFF47 => self.bgp,
            0xFF48 => self.obp0,
            0xFF49 => self.obp1,
            0xFF4A => self.wy,
            0xFF4B => self.wx,
            0xFF4F => 0xFE | self.vbk,
            0xFF68 => 0x40 | self.bcps,
            0xFF69 => self.read_palette(false),
            0xFF6A => 0x40 | self.ocps,
            0xFF6B => self.read_palette(true),
            0xFF6C => 0xFE | u8::from(self.opri_by_x),
            _ => 0xFF,
        }
    }

    pub fn write_reg(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF40 => {
                let was_on = self.lcd_on();
                // DMG: disabling objects while one is being fetched aborts that fetch.
                // (Checked on every write, including the CPU's one-dot intermediate value.)
                // While the fetch still waits for the background fetcher's tile data, pixel
                // output stays paused a few more dots (Gambatte's hwtests,
                // sprites/sprite_late_*disable*; mealybug's m3_lcdc_obj_en_change_variant
                // has the aborts once the tile data is in).
                if self.lcdc & lcdc::OBJ_ENABLE != 0
                    && value & lcdc::OBJ_ENABLE == 0
                    && self.during_object_fetch
                {
                    if self.obj_fetch == ObjFetch::Waiting && self.fetch < Fetch::High2 {
                        self.obj_fetch = ObjFetch::Aborted(ABORTED_OBJ_STALL);
                    } else {
                        self.obj_fetch = ObjFetch::Idle;
                        self.during_object_fetch = false;
                    }
                }
                // DMG: switching the LCD off takes effect a dot after the write; an interrupt
                // raised in that dot is still requested (Gambatte's hwtests,
                // lycEnable/ff40_disable_2).
                if was_on && value & lcdc::LCD_ENABLE == 0 && self.dmg_hardware() {
                    self.off_irq = self.tick_dot();
                }
                let lyc_flag = self.lyc_flag();
                // DMG: objects just enabled can only start a sprite fetch a few dots later
                // (Gambatte's hwtests, sprites/sprite_late_enable_*).
                if self.dmg_hardware()
                    && self.lcdc & lcdc::OBJ_ENABLE == 0
                    && value & lcdc::OBJ_ENABLE != 0
                {
                    self.obj_enable_delay = 4;
                }
                self.lcdc = value;
                if was_on && !self.lcd_on() {
                    self.lyc_flag_off = lyc_flag;
                    // LCD off: LY resets and the PPU idles until it is switched back on.
                    self.ly = 0;
                    self.dot = 0;
                    self.mode = Mode::HBlank;
                    self.irq_mode = Mode::HBlank;
                    self.pipeline_active = false;
                    self.window_y = 0xFF;
                    self.wy_triggered = false;
                    self.wx_triggered = false;
                } else if !was_on && self.lcd_on() {
                    self.first_line = true;
                    // STAT reads mode 0 until mode 3, but no HBlank interrupt source is
                    // active then (the DMG's STAT write bug does not fire).
                    self.irq_mode = Mode::Drawing;
                }
            }
            0xFF41 => {
                self.stat = value & 0x78;
                let delay = self.reg_irq_delay(STAT_IRQ_DELAY);
                if delay == 0 {
                    self.apply_irq_stat(self.stat);
                } else {
                    self.stat_pending = (self.stat, delay);
                }
            }
            0xFF42 => self.scy = value,
            0xFF43 => self.scx = value,
            0xFF44 => {} // LY is read-only
            0xFF45 => {
                // Moving LYC along with LY just before it increments keeps the LYC source
                // high: no new interrupt.
                let window = u32::from(self.reg_irq_delay(LYC_FOLLOW_WINDOW));
                if self.lcd_on()
                    && self.ly < LINES_PER_FRAME - 1
                    && self.dot + window >= DOTS_PER_LINE
                    && self.dot + u32::from(self.reg_irq_delay(LYC_FOLLOW_TAIL)) < DOTS_PER_LINE
                    && self.irq_lyc == self.ly
                    && value == self.ly + 1
                {
                    self.lyc_edge_masked = true;
                }
                self.lyc = value;
                let delay = self.reg_irq_delay(LYC_IRQ_DELAY);
                if delay == 0 {
                    self.apply_irq_lyc(value);
                } else {
                    self.lyc_pending = (value, delay);
                }
            }
            0xFF47 => self.bgp = value,
            0xFF48 => self.obp0 = value,
            0xFF49 => self.obp1 = value,
            0xFF4A => {
                let delay = if self.dmg_hardware() {
                    WY2_DMG
                } else if self.double_speed {
                    WY2_CGB_DS
                } else {
                    WY2_CGB
                };
                self.wy_for_wx = (self.wy, delay);
                self.wy = value;
            }
            0xFF4B => self.wx = value,
            0xFF4F => self.vbk = value & 1,
            0xFF68 => self.bcps = value & 0xBF,
            0xFF69 => self.write_palette(false, value),
            0xFF6A => self.ocps = value & 0xBF,
            0xFF6B => self.write_palette(true, value),
            0xFF6C => self.opri_by_x = value & 1 != 0,
            _ => {}
        }
    }

    /// Value LCDC holds for one T-cycle while the CPU writes it on a DMG: the old value
    /// with the fetcher-facing bits already new (OBJ enable clears early in some states).
    pub fn dmg_lcdc_write_conflict(&self, old: u8, value: u8) -> u8 {
        let mut old = old;
        let obj_off = value & lcdc::OBJ_ENABLE == 0;
        if obj_off && (self.position == 0 || self.during_object_fetch) {
            old &= !lcdc::OBJ_ENABLE;
        }
        // Fetcher-facing bits (maps, tile data, OBJ size) take the new value at once; BG,
        // window and OBJ enable keep their old value until the write completes (Gambatte's
        // hwtests, window/late_disable*).
        let early =
            lcdc::BG_MAP_9C00 | lcdc::TILE_DATA_8000 | lcdc::WINDOW_MAP_9C00 | lcdc::OBJ_SIZE_8X16;
        (old & !early) | (value & early)
    }

    /// Notes an LCDC write: disabling the window while it is being fetched turns off the
    /// DMG's window pixel-insertion glitch for the rest of the line.
    pub fn lcdc_written(&mut self, old: u8, value: u8) {
        if old & lcdc::WINDOW_ENABLE != 0
            && value & lcdc::WINDOW_ENABLE == 0
            && self.window_being_fetched
        {
            self.disable_insertion_glitch = true;
        }
    }

    /// Set for the T-cycle in which the CPU writes WX.
    pub fn set_wx_just_changed(&mut self, changed: bool) {
        self.wx_just_changed = changed;
    }

    /// Advances the PPU by `cycles` T-cycles. Returns interrupt bits to raise.
    pub fn tick(&mut self, cycles: u32) -> u8 {
        let mut irq = 0;
        if !self.lcd_on() {
            // With the LCD off only a STAT or LYC write can raise the interrupt (through
            // the LYC flag frozen when the LCD was switched off).
            if std::mem::take(&mut self.write_irq_in) > 0 {
                irq |= interrupt::STAT;
            }
            return irq | std::mem::take(&mut self.off_irq);
        }
        for _ in 0..cycles {
            irq |= self.tick_dot();
        }
        irq
    }

    fn tick_dot(&mut self) -> u8 {
        let mut lyc_line_start = false;
        if self.stat_pending.1 > 0 {
            self.stat_pending.1 -= 1;
            if self.stat_pending.1 == 0 {
                self.apply_irq_stat(self.stat_pending.0);
            }
        }
        if self.lyc_pending.1 > 0 {
            self.lyc_pending.1 -= 1;
            if self.lyc_pending.1 == 0 {
                self.apply_irq_lyc(self.lyc_pending.0);
            }
        }
        self.palette_unlock_in = self.palette_unlock_in.saturating_sub(1);
        self.wy_for_wx.1 = self.wy_for_wx.1.saturating_sub(1);
        self.obj_enable_delay = self.obj_enable_delay.saturating_sub(1);
        self.obj_size_history =
            self.obj_size_history << 1 | u8::from(self.lcdc & lcdc::OBJ_SIZE_8X16 != 0);
        let mut irq = 0;
        // At normal speed STAT reads mode 0 from line 153's last 4 dots and mode 2 from the
        // end of the line (Gambatte's hwtests, ly0/lycint152_ly0stat_*); the interrupt
        // sources keep their own timing.
        if self.ly == LINES_PER_FRAME - 1 && !self.double_speed {
            if self.dot == DOTS_PER_LINE - 5 {
                self.mode = Mode::HBlank;
            } else if self.dot == DOTS_PER_LINE - 1 {
                self.mode = Mode::OamScan;
            }
        }
        if (self.ly as usize) < SCREEN_HEIGHT {
            match self.dot {
                0 if self.ly == 0 => {
                    // Line 0 is special: its OAM interrupt fires one dot later than on other
                    // lines (in double speed STAT also reads mode 0 for one dot).
                    if self.double_speed {
                        self.mode = Mode::HBlank;
                    }
                    self.window_y = 0xFF;
                }
                0 | 1 if self.first_line => {
                    if self.dot == 1 {
                        self.sprites.clear();
                    }
                }
                0 | 1 if self.ly != 0 || self.dot == 1 => {
                    self.mode = Mode::OamScan;
                    self.irq_mode = Mode::OamScan;
                    if self.ly == 0 {
                        self.oam_int_armed = self.m2_int_unblocked;
                    }
                    if self.dot == 1 {
                        self.sprites.clear();
                    }
                }
                // OAM scan checks entry i at dot 2i + 3, with the OBJ size of three dots earlier.
                3..=81 if self.dot % 2 == 1 => self.scan_oam_entry((self.dot as usize - 3) / 2),
                _ => {}
            }
            match self.dot {
                MODE3_STAT_DOT => {
                    self.mode = Mode::Drawing;
                    self.irq_mode = Mode::Drawing;
                }
                SPRITE_SORT_DOT => {
                    self.sprites.sort_by_key(|s| s.x); // stable: equal X keeps OAM order
                    self.next_sprite = 0;
                }
                PIPELINE_START_DOT => self.start_pipeline(),
                _ => {}
            }
            // WY is compared with LY at fixed points: early on line 0 (resetting the
            // latch for the frame), and near the end of each line with this and the next
            // LY (Gambatte's hwtests, window/arg/late_wy_*). Otherwise only the WX match
            // itself looks at WY (`window_y_match`).
            let window_enabled = self.lcdc & lcdc::WINDOW_ENABLE != 0;
            if self.ly == 0 && self.dot == WY_LINE0_DOT {
                self.wy_triggered = window_enabled && self.wy == 0;
            } else if self.dot == WY_CHECK_DOT {
                self.wy_triggered |= window_enabled && self.wy == self.ly;
            } else if self.dot == WY_CHECK_NEXT_DOT {
                self.wy_triggered |= window_enabled && self.wy == self.ly + 1;
            }
            if self.pipeline_active {
                self.mode3_dot();
                // STAT switches to mode 0 shortly before the pipeline finishes.
                if self.mode == Mode::Drawing
                    && (SCREEN_WIDTH as u8 - MAX_MODE0_LEAD as u8..SCREEN_WIDTH as u8)
                        .contains(&self.position)
                    && !self.lookahead
                    && self.mode0_due()
                {
                    self.mode = Mode::HBlank;
                    self.irq_mode = Mode::HBlank;
                    self.palette_unlock_in = PALETTE_UNLOCK_DELAY;
                    self.hblank_started = true;
                    self.hblank_start_dot = self.dot;
                }
            }
        }

        // Line 0's mode 2 interrupt is decided before LY wraps: it fires if the OAM source
        // is enabled then, unless the mode 1 source is too (it blocks it, though it ends
        // a dot before mode 2 starts). Later STAT writes do not change that.
        let (latch_ly, latch_dot) = if self.double_speed {
            LY0_OAM_INT_LATCH_DS
        } else {
            LY0_OAM_INT_LATCH
        };
        if self.ly == latch_ly && self.dot == latch_dot {
            self.m2_int_unblocked =
                self.irq_stat & (stat::OAM_INT | stat::VBLANK_INT) == stat::OAM_INT;
        }
        if (self.ly as usize) < SCREEN_HEIGHT - 1 && self.dot == M2_LATCH_DOT {
            // Only whether it is blocked: enabling it later still counts.
            self.m2_int_unblocked = self.irq_stat & stat::HBLANK_INT == 0
                && !(self.irq_stat & stat::LYC_INT != 0 && self.ly_matches(self.irq_lyc));
        }
        // On the CGB the same quirk fires a few dots before LY becomes 144.
        if !self.dmg_hardware()
            && self.ly as usize == SCREEN_HEIGHT - 1
            && self.dot == CGB_VBLANK_OAM_INT_DOT
            && self.irq_stat & stat::OAM_INT != 0
            && !self.stat_line
        {
            irq |= interrupt::STAT;
        }
        self.dot += 1;
        if self.dot == DOTS_PER_LINE {
            self.dot = 0;
            if std::mem::take(&mut self.lyc_edge_masked) {
                self.stat_line = true;
            }
            lyc_line_start = true;
            self.first_line = false;
            self.ly += 1;
            if self.ly as usize == SCREEN_HEIGHT {
                self.mode = Mode::VBlank;
                self.frame_ready = true;
                self.frame_done = true;
                irq |= interrupt::VBLANK;
                // DMG quirk: entering VBlank also triggers the OAM (mode 2) STAT interrupt.
                if self.dmg_hardware() && self.irq_stat & stat::OAM_INT != 0 && !self.stat_line {
                    irq |= interrupt::STAT;
                }
            } else if self.ly == LINES_PER_FRAME {
                self.ly = 0;
            } else if (self.ly as usize) < SCREEN_HEIGHT {
                // The new line's mode 2 interrupt source rises in the same instant LY changes
                // (and the LYC source of the previous line drops). Without a gap the combined
                // STAT line stays high, so an LY=LYC interrupt on the previous line blocks this
                // line's mode 2 interrupt ("STAT blocking"). Games rely on this, e.g. Pinball
                // Fantasies. (Line 0 is handled separately: its OAM interrupt comes a dot late.)
                // Blocking is decided a few dots early (`M2_LATCH_DOT`).
                self.irq_mode = Mode::OamScan;
                self.oam_int_armed = self.m2_int_unblocked && self.irq_stat & stat::OAM_INT != 0;
            }
        }

        // The mode 1 STAT interrupt source rises after the VBlank interrupt. Mooneye's
        // intr_1_2_timing bounds the delay to 1–4 dots; one M-cycle is used.
        if self.ly as usize == SCREEN_HEIGHT && self.dot == 4 {
            self.irq_mode = Mode::VBlank;
        }
        if self.write_irq_in > 0 {
            self.write_irq_in -= 1;
            if self.write_irq_in == 0 {
                irq |= interrupt::STAT;
            }
        }
        // An LYC match starting with lines 1–144 is blocked by an enabled mode 2 source,
        // even when that one's own interrupt is blocked.
        if lyc_line_start
            && (1..=SCREEN_HEIGHT as u8).contains(&self.ly)
            && self.irq_stat & stat::OAM_INT != 0
            && !self.oam_int_armed
        {
            self.stat_line = true;
        }
        // STAT interrupt fires on a rising edge of the OR of all enabled sources.
        let line = self.stat_sources();
        if line && !self.stat_line {
            irq |= interrupt::STAT;
        }
        self.stat_line = line;
        irq
    }

    /// The OR of all enabled STAT interrupt sources.
    fn stat_sources(&self) -> bool {
        (self.irq_stat & stat::LYC_INT != 0
            && self.ly_matches(self.irq_lyc)
            && !self.exclude_lyc_source)
            || (self.irq_stat & stat::HBLANK_INT != 0
                && self.irq_mode == Mode::HBlank
                && (self.dot < HBLANK_SOURCE_END || self.ly as usize >= SCREEN_HEIGHT - 1))
            || (self.irq_stat & stat::VBLANK_INT != 0
                && self.irq_mode == Mode::VBlank
                && self.ly as usize >= SCREEN_HEIGHT)
            || ((self.irq_stat & stat::OAM_INT != 0 || self.ly == 0)
                && self.oam_int_armed
                && self.irq_mode == Mode::OamScan
                && self.dot < OAM_PULSE_DOTS)
    }

    /// Dots until a STAT or LYC write reaches the interrupt logic (none with the LCD off).
    fn reg_irq_delay(&self, delays: (u8, u8, u8)) -> u8 {
        if !self.lcd_on() {
            0
        } else if self.dmg_hardware() {
            delays.0
        } else if self.double_speed {
            delays.2
        } else {
            delays.1
        }
    }

    /// A STAT write reaches the interrupt logic.
    fn apply_irq_stat(&mut self, value: u8) {
        self.irq_stat = value;
        // Enabling it in the first M-cycle of mode 2 still counts.
        if self.irq_mode == Mode::OamScan && self.ly != 0 && self.dot < 4 {
            self.oam_int_armed = self.irq_stat & stat::OAM_INT != 0;
        }
        self.check_write_irq();
    }

    /// An LYC write reaches the interrupt logic.
    fn apply_irq_lyc(&mut self, value: u8) {
        self.irq_lyc = value;
        self.check_write_irq();
    }

    /// After a STAT or LYC write: if it raised the STAT interrupt line, the interrupt is
    /// requested a few dots later (Gambatte's hwtests, lycEnable/*trigger_delay*).
    fn check_write_irq(&mut self) {
        if !self.lcd_on() {
            let line = self.irq_stat & stat::LYC_INT != 0 && self.lyc_flag_off;
            if line && !self.stat_line {
                self.write_irq_in = 1;
            }
            self.stat_line = line;
            return;
        }
        // DMG: in a line's last 4 dots LY is already changing; a write that makes only the
        // LYC source match there raises no interrupt (Gambatte's hwtests,
        // lycEnable/late_ff45_enable_3).
        if !self.stat_line && self.ly_changing() && self.dmg_hardware() {
            self.exclude_lyc_source = true;
            let others = self.stat_sources();
            self.exclude_lyc_source = false;
            if !others && self.stat_sources() {
                self.stat_line = true;
                return;
            }
        }
        if !self.stat_line && self.stat_sources() {
            self.stat_line = true;
            self.write_irq_in = if self.dmg_hardware() {
                WRITE_IRQ_DMG
            } else if self.double_speed {
                WRITE_IRQ_DS
            } else {
                WRITE_IRQ_CGB
            };
        }
    }

    /// Whether mode 3 fetches the sprites found by the OAM scan: on the CGB even with
    /// objects disabled (Gambatte's PPU; hwtests oamdma/late_sp* run with LCDC = $91).
    fn objs_fetched(&self) -> bool {
        self.lcdc & lcdc::OBJ_ENABLE != 0 || !self.dmg_hardware()
    }

    /// OAM scan step: selects entry `index` if it covers this line (max 10 per line).
    fn scan_oam_entry(&mut self, index: usize) {
        if index >= 40 || self.sprites.len() == MAX_SPRITES_PER_LINE || self.oam_blocked {
            return;
        }
        let height: i16 = if self.obj_size_history & 0b1000 != 0 {
            16
        } else {
            8
        };
        let s = &self.oam[index * 4..index * 4 + 4];
        let top = s[0] as i16 - 16;
        if (top..top + height).contains(&(self.ly as i16)) {
            self.sprites.push(Sprite {
                y: s[0],
                x: s[1],
                tile: s[2],
                attr: s[3],
                index: index as u8,
            });
        }
    }

    fn start_pipeline(&mut self) {
        self.pipeline_active = true;
        self.late_mode0 = false;
        self.bg_fifo.clear();
        self.obj_fifo.clear();
        self.bg_fifo.extend([0; 8]); // junk pixels, dropped at negative positions
        self.position = POSITION_START;
        self.lcd_x = 0;
        self.bg_tile_x = 0;
        self.fetch = Fetch::Tile1;
        self.obj_fetch = ObjFetch::Idle;
        self.during_object_fetch = false;
        self.window_stall = false;
        self.disable_insertion_glitch = false;
        self.insert_bg_pixel = false;
    }

    /// One dot of the pixel pipeline.
    fn mode3_dot(&mut self) {
        if self.obj_fetch != ObjFetch::Idle {
            self.object_fetch_dot();
            return;
        }

        if self.window_stall {
            self.window_stall = false;
        } else if self.check_window() {
            return;
        }

        // DMG quirk: a pixel is inserted when WX matches again after the window started.
        if self.dmg_hardware()
            && self.wx == self.position.wrapping_add(7)
            && self.wx_triggered
            && !self.window_being_fetched
            && self.fetch == Fetch::Tile1
            && self.bg_fifo.len() == 8
        {
            self.insert_bg_pixel = true;
        }

        // Sprites whose X has already been passed are skipped.
        let match_x = self.x_for_object_match();
        while self
            .sprites
            .get(self.next_sprite)
            .is_some_and(|s| s.x < match_x)
        {
            self.next_sprite += 1;
        }
        if self.objs_fetched()
            && self.obj_enable_delay == 0
            && self
                .sprites
                .get(self.next_sprite)
                .is_some_and(|s| s.x == match_x)
        {
            self.during_object_fetch = true;
            self.obj_fetch = ObjFetch::Waiting;
            self.object_fetch_dot();
            return;
        }

        self.render_pixel();
        self.advance_fetcher();
        if self.position == SCREEN_WIDTH as u8 {
            self.end_line();
        }
    }

    /// Whether STAT (and its interrupt) switch to mode 0 on this dot: two dots before the
    /// pipeline pushes its last pixel (252 dots into a line without stalls), or a few
    /// more if the line fetched sprites (`mode0_lead`). Mooneye's
    /// intr_2_mode0_timing_sprites pins this down, including sprites at X 164–167 that
    /// still stall the end of the line.
    fn mode0_due(&self) -> bool {
        let left = SCREEN_WIDTH as u32 - self.position as u32;
        let match_x = self.x_for_object_match();
        let sprite_ahead = self.objs_fetched()
            && self.sprites[self.next_sprite.min(self.sprites.len())..]
                .iter()
                .any(|s| s.x >= match_x && s.x < SCREEN_WIDTH as u8 + 8);
        let window_ahead = !self.wx_triggered
            && self.window_y_match()
            && self.lcdc & lcdc::WINDOW_ENABLE != 0
            && (160..=166).contains(&self.wx);
        // Enough pixels queued, or the next tile arrives before the FIFO runs dry.
        let fifo_ready = self.bg_fifo.len() as u32 >= left
            || (!self.bg_fifo.is_empty() && self.fetch >= Fetch::High1);
        if self.obj_fetch == ObjFetch::Idle
            && !self.window_stall
            && !sprite_ahead
            && !window_ahead
            && fifo_ready
        {
            return left <= self.mode0_lead();
        }
        // Stalls ahead: run a copy of the pipeline to the end of the line.
        let mut ahead = self.clone();
        ahead.lookahead = true;
        let mut dots = 0;
        while ahead.pipeline_active {
            ahead.mode3_dot();
            dots += 1;
        }
        dots <= ahead.mode0_lead()
    }

    /// Dots between mode 0 in STAT and the end of the pipeline (see `mode0_due`): 2, or
    /// with sprites 4 on the DMG and 5 on the CGB at normal speed (both exact for
    /// Mooneye's test; Gambatte's double-speed tests show 2 there too).
    /// Sprites fetched on the line are approximated by sprites passed while fetching them.
    fn mode0_lead(&self) -> u32 {
        let lead = if !self.objs_fetched() || self.next_sprite == 0 || self.double_speed {
            2
        } else if self.dmg_hardware() {
            4
        } else {
            MAX_MODE0_LEAD
        };
        lead - u32::from(self.late_mode0)
    }

    /// Whether the window may start on this line: WY matched at one of the latch points
    /// this frame, or WY equals LY right now.
    fn window_y_match(&self) -> bool {
        let wy = if self.wy_for_wx.1 == 0 {
            self.wy
        } else {
            self.wy_for_wx.0
        };
        self.wy_triggered || (self.lcdc & lcdc::WINDOW_ENABLE != 0 && wy == self.ly)
    }

    /// X coordinate (in OAM terms) of sprites that start at the current position.
    fn x_for_object_match(&self) -> u8 {
        let x = self.position.wrapping_add(8);
        if x > POSITION_START { 0 } else { x }
    }

    /// Starts the window if WX matches the current position. Returns `true` if the rest
    /// of this dot is delayed by a pipeline stall.
    fn check_window(&mut self) -> bool {
        if self.wx_triggered || !self.window_y_match() || self.lcdc & lcdc::WINDOW_ENABLE == 0 {
            return false;
        }
        let pos = self.position;
        let activate = if self.wx == 0 {
            // pos == −7, or −16 with fine scroll, or anywhere in −15..=−8.
            pos == 0u8.wrapping_sub(7)
                || (pos == POSITION_START && self.scx & 7 != 0)
                || (POSITION_START.wrapping_add(1)..=0u8.wrapping_sub(8)).contains(&pos)
        } else if self.wx < 166 || (self.wx == 166 && !self.dmg_hardware()) {
            // On the CGB, WX = 166 also starts the window (and its fetch stall) in the
            // line's last pixels (Gambatte's hwtests, window/m2int_wxA6_firstline_*).
            if self.wx == pos.wrapping_add(7) {
                true
            } else if self.wx == pos.wrapping_add(6) && !self.wx_just_changed {
                // One position late. Many DMGs also desync the LCD by a pixel here, but the
                // reference unit for the mealybug screenshots does not, so neither do we.
                true
            } else {
                false
            }
        } else {
            false
        };
        if activate {
            self.window_y = self.window_y.wrapping_add(1);
            self.window_tile_x = 0;
            self.bg_fifo.clear();
            self.wx_triggered = true;
            self.fetch = Fetch::Tile1;
            self.window_being_fetched = true;
            if self.wx == 0 && self.scx & 7 != 0 {
                // DMG: starting the window at WX = 0 with fine scroll costs one extra dot.
                self.window_stall = true;
                return true;
            }
            // DMG: started on an even dot, the window ends mode 3 in STAT one dot later
            // (Gambatte's hwtests, window/*scx2*); the pixels are not delayed (mealybug's
            // m3_window_timing).
            self.late_mode0 = self.dot & 1 == 0 && self.dmg_hardware();
        }
        false
    }

    /// Y used by the fetcher: the window line, or LY + SCY for the background.
    fn fetcher_y(&self) -> u8 {
        if self.wx_triggered {
            self.window_y
        } else {
            self.ly.wrapping_add(self.scy)
        }
    }

    /// VRAM offset of the current tile's row for the fetcher's Y.
    fn tile_row_addr(&self) -> usize {
        let base = if self.lcdc & lcdc::TILE_DATA_8000 != 0 {
            self.tile as usize * 16
        } else {
            (0x1000 + (self.tile as i8 as isize) * 16) as usize
        };
        let bank = if self.tile_attr & attr::BANK1 != 0 {
            0x2000
        } else {
            0
        };
        let mut row = self.fetcher_y() & 7;
        if self.tile_attr & attr::Y_FLIP != 0 {
            row = 7 - row;
        }
        bank + base + row as usize * 2
    }

    /// VRAM offset of the tile map entry the fetcher reads next.
    fn tile_map_address(&self) -> usize {
        let high_map = if self.wx_triggered {
            self.lcdc & lcdc::WINDOW_MAP_9C00 != 0
        } else {
            self.lcdc & lcdc::BG_MAP_9C00 != 0
        };
        let x = if self.wx_triggered {
            self.window_tile_x
        } else {
            ((self.scx >> 3) + self.bg_tile_x) & 0x1F
        };
        let map = if high_map { 0x1C00 } else { 0x1800 };
        map + x as usize + (self.fetcher_y() / 8) as usize * 32
    }

    /// One step of the background/window fetcher.
    fn advance_fetcher(&mut self) {
        match self.fetch {
            Fetch::Tile1 => {
                if self.lcdc & lcdc::WINDOW_ENABLE == 0 {
                    self.wx_triggered = false;
                }
                self.tile_map_addr = self.tile_map_address();
                self.fetch = Fetch::Tile2;
            }
            Fetch::Tile2 => {
                self.tile = self.vram[self.tile_map_addr];
                self.tile_attr = if self.cgb {
                    self.vram[0x2000 + self.tile_map_addr]
                } else {
                    0
                };
                self.fetch = Fetch::Low1;
            }
            Fetch::Low1 => {
                self.tile_data_addr = self.tile_row_addr();
                self.fetch = Fetch::Low2;
            }
            Fetch::Low2 => {
                self.tile_data[0] = self.vram[self.tile_data_addr];
                self.fetch = Fetch::High1;
            }
            Fetch::High1 => {
                self.tile_data_addr = self.tile_row_addr() + 1;
                self.fetch = Fetch::High2;
            }
            Fetch::High2 => {
                self.tile_data[1] = self.vram[self.tile_data_addr];
                if self.wx_triggered {
                    self.window_tile_x = (self.window_tile_x + 1) & 0x1F;
                } else {
                    self.bg_tile_x = self.bg_tile_x.wrapping_add(1);
                }
                self.fetch = Fetch::Push;
                self.try_push();
            }
            Fetch::Push => self.try_push(),
        }
    }

    /// Pushes the fetched tile row once the background FIFO is empty.
    fn try_push(&mut self) {
        if !self.bg_fifo.is_empty() {
            return;
        }
        if self.dmg_hardware()
            && self.wy_triggered
            && self.lcdc & lcdc::WINDOW_ENABLE == 0
            && !self.disable_insertion_glitch
        {
            // DMG quirk: with the window disabled mid-frame, reaching WX inserts one
            // background-colour pixel instead of the fetched row.
            let mut logical = self.position.wrapping_add(7);
            if logical > 167 {
                logical = 0;
            }
            if self.wx == logical {
                self.bg_fifo.push_back(0);
                return;
            }
        }
        let [lo, hi] = self.tile_data;
        let a = self.tile_attr;
        let extra = ((a & attr::PALETTE) << bg_pixel::PALETTE_SHIFT) | (a & attr::PRIORITY);
        for i in 0..8 {
            let bit = if a & attr::X_FLIP != 0 { i } else { 7 - i };
            self.bg_fifo
                .push_back(extra | (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1));
        }
        self.fetch = Fetch::Tile1;
    }

    /// One dot of a sprite fetch (the pixel pipeline is paused meanwhile).
    fn object_fetch_dot(&mut self) {
        match self.obj_fetch {
            ObjFetch::Idle => {}
            ObjFetch::Waiting => {
                let ready = self.fetch >= Fetch::High2 && !self.bg_fifo.is_empty();
                self.advance_fetcher();
                if ready {
                    self.obj_fetch = ObjFetch::Attributes;
                }
            }

            ObjFetch::Attributes => {
                self.advance_fetcher();
                let s = self.sprites[self.next_sprite];
                self.obj_attr = [s.tile, s.attr];
                self.obj_fetch = ObjFetch::Attributes2;
            }
            ObjFetch::Attributes2 => self.obj_fetch = ObjFetch::Low,
            ObjFetch::Low => {
                self.obj_low = self.vram[self.object_row_addr()];
                self.obj_fetch = ObjFetch::Low2;
            }
            ObjFetch::Low2 => self.obj_fetch = ObjFetch::High,
            ObjFetch::Aborted(n) => {
                if n <= 1 {
                    self.obj_fetch = ObjFetch::Idle;
                    self.during_object_fetch = false;
                } else {
                    self.obj_fetch = ObjFetch::Aborted(n - 1);
                }
            }
            ObjFetch::High => {
                self.during_object_fetch = false;
                let hi = self.vram[self.object_row_addr() + 1];
                self.overlay_object(self.obj_low, hi);
                self.next_sprite += 1;
                self.obj_fetch = ObjFetch::Idle;
            }
        }
    }

    /// VRAM offset of the current sprite's tile row (OBJ size is read at access time).
    fn object_row_addr(&self) -> usize {
        let s = self.sprites[self.next_sprite];
        let [tile, attr] = self.obj_attr;
        let tall = self.lcdc & lcdc::OBJ_SIZE_8X16 != 0;
        let mask = if tall { 0x0F } else { 0x07 };
        let mut row = self.ly.wrapping_sub(s.y.wrapping_sub(16)) & mask;
        if attr & 0x40 != 0 {
            row ^= mask; // vertical flip
        }
        let tile = if tall { tile & 0xFE } else { tile };
        let bank = if self.cgb && attr & self::attr::BANK1 != 0 {
            0x2000
        } else {
            0
        };
        bank + tile as usize * 16 + row as usize * 2
    }

    /// Mixes a sprite row into the object FIFO; earlier sprites keep their pixels.
    fn overlay_object(&mut self, lo: u8, hi: u8) {
        while self.obj_fifo.len() < 8 {
            self.obj_fifo.push_back(ObjPixel::default());
        }
        let attr = self.obj_attr[1];
        let index = self.sprites[self.next_sprite].index;
        let flip = attr & 0x20 != 0;
        for i in 0..8u8 {
            let bit = if flip { i } else { 7 - i };
            let color = (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1);
            let target = &mut self.obj_fifo[i as usize];
            // DMG: sprites fetched first (lower X) win. CGB: the lower OAM entry wins.
            let wins = target.color == 0 || (!self.opri_by_x && index < target.index);
            if color != 0 && wins {
                *target = ObjPixel {
                    color,
                    palette1: attr & 0x10 != 0,
                    behind_bg: attr & 0x80 != 0,
                    cgb_palette: attr & self::attr::PALETTE,
                    index,
                };
            }
        }
    }

    /// Shifts one pixel out of the FIFOs; draws it if its position is on screen.
    fn render_pixel(&mut self) {
        // Output stalls while a sprite at X = 0 is still to be fetched.
        if self.objs_fetched() && self.sprites.get(self.next_sprite).is_some_and(|s| s.x == 0) {
            return;
        }
        if self.bg_fifo.is_empty() {
            return;
        }
        let bg = if self.insert_bg_pixel {
            self.insert_bg_pixel = false;
            0
        } else {
            self.bg_fifo.pop_front().unwrap_or(0)
        };
        let obj = self.obj_fifo.pop_front().unwrap_or_default();

        if self.position.wrapping_add(16) < 8 {
            // Negative positions: fine scrolling drops pixels until SCX % 8 is reached.
            let pos = self.position;
            if pos == POSITION_START.wrapping_sub(1) {
                self.position = POSITION_START;
            } else if pos & 7 == self.scx & 7
                || (self.window_being_fetched && pos & 7 == 6 && self.scx & 7 == 7)
            {
                self.position = 0u8.wrapping_sub(8);
            } else if pos == 0u8.wrapping_sub(9) {
                self.position = POSITION_START;
                return;
            }
        }
        self.window_being_fetched = false;

        if self.position >= SCREEN_WIDTH as u8 {
            self.position = self.position.wrapping_add(1); // off screen: dropped
            return;
        }

        if (self.lcd_x as usize) < SCREEN_WIDTH {
            let i = self.ly as usize * SCREEN_WIDTH + self.lcd_x as usize;
            if self.cgb {
                self.rgb_framebuffer[i] = self.cgb_pixel(bg, obj);
            } else if self.dmg_compat {
                self.rgb_framebuffer[i] = self.compat_pixel(bg & bg_pixel::COLOR, obj);
            } else {
                self.framebuffer[i] = self.dmg_pixel(bg & bg_pixel::COLOR, obj);
            }
        }
        self.position += 1;
        self.lcd_x += 1;
    }

    /// DMG pixel mixing: the shade (0–3) of a background colour and an object pixel.
    fn dmg_pixel(&self, bg: u8, obj: ObjPixel) -> u8 {
        let bg = if self.lcdc & lcdc::BG_WINDOW_ENABLE != 0 {
            bg
        } else {
            0
        };
        let draw_obj =
            obj.color != 0 && self.lcdc & lcdc::OBJ_ENABLE != 0 && !(obj.behind_bg && bg != 0);
        if draw_obj {
            let palette = if obj.palette1 { self.obp1 } else { self.obp0 };
            (palette >> (obj.color * 2)) & 3
        } else {
            (self.bgp >> (bg * 2)) & 3
        }
    }

    /// DMG compatibility mode: DMG mixing and palettes, then the resulting shade picks a
    /// colour from CGB palette RAM (BG palette 0, OBJ palette 0 or 1).
    fn compat_pixel(&self, bg: u8, obj: ObjPixel) -> u16 {
        let bg = if self.lcdc & lcdc::BG_WINDOW_ENABLE != 0 {
            bg
        } else {
            0
        };
        let draw_obj =
            obj.color != 0 && self.lcdc & lcdc::OBJ_ENABLE != 0 && !(obj.behind_bg && bg != 0);
        let (ram, palette, shade) = if draw_obj {
            let obp = if obj.palette1 { self.obp1 } else { self.obp0 };
            (
                &self.obj_palettes,
                obj.palette1 as usize,
                (obp >> (obj.color * 2)) & 3,
            )
        } else {
            (&self.bg_palettes, 0, (self.bgp >> (bg * 2)) & 3)
        };
        let i = palette * 8 + shade as usize * 2;
        u16::from_le_bytes([ram[i], ram[i + 1]]) & 0x7FFF
    }

    /// CGB pixel mixing: the RGB555 colour of a background FIFO entry and an object pixel.
    /// LCDC bit 0 is the master priority switch: when clear, objects are always on top.
    /// Otherwise a non-zero background colour covers the object if either the tile's
    /// priority attribute or the object's behind-background flag is set.
    fn cgb_pixel(&self, bg: u8, obj: ObjPixel) -> u16 {
        let color = bg & bg_pixel::COLOR;
        let bg_on_top = self.lcdc & lcdc::BG_WINDOW_ENABLE != 0
            && color != 0
            && (bg & bg_pixel::PRIORITY != 0 || obj.behind_bg);
        let (ram, palette, color) =
            if obj.color != 0 && self.lcdc & lcdc::OBJ_ENABLE != 0 && !bg_on_top {
                (&self.obj_palettes, obj.cgb_palette, obj.color)
            } else {
                (&self.bg_palettes, bg >> bg_pixel::PALETTE_SHIFT & 7, color)
            };
        let i = palette as usize * 8 + color as usize * 2;
        u16::from_le_bytes([ram[i], ram[i + 1]]) & 0x7FFF
    }

    /// Mode 3 is over: switch to HBlank and prepare the window for the next line.
    fn end_line(&mut self) {
        self.pipeline_active = false;
        // If the LCD lost a pixel (window desync), repeat the last one to fill the line.
        let row = self.ly as usize * SCREEN_WIDTH;
        while (self.lcd_x as usize) < SCREEN_WIDTH {
            let x = self.lcd_x as usize;
            if x == 0 {
                self.framebuffer[row] = 0;
                self.rgb_framebuffer[row] = 0x7FFF;
            } else {
                self.framebuffer[row + x] = self.framebuffer[row + x - 1];
                self.rgb_framebuffer[row + x] = self.rgb_framebuffer[row + x - 1];
            }
            self.lcd_x += 1;
        }
        self.position = POSITION_START;
        if self.window_y_match()
            && self.lcdc & lcdc::WINDOW_ENABLE != 0
            && self.wx == 166
            && self.dmg_hardware()
        {
            // DMG: WX = 166 starts the window at the end of this line for the next one.
            self.wx_triggered = true;
            self.window_tile_x = 1;
            self.window_y = self.window_y.wrapping_add(1);
        } else {
            self.wx_triggered = false;
        }
        if self.mode == Mode::Drawing {
            self.hblank_started = true;
            self.hblank_start_dot = self.dot;
        }
        self.mode = Mode::HBlank;
        self.irq_mode = Mode::HBlank;
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

    /// Counts STAT interrupts per line over one frame.
    fn stat_irqs_per_line(setup: impl FnOnce(&mut Ppu)) -> Vec<u32> {
        let mut ppu = Ppu::new();
        setup(&mut ppu);
        let mut counts = vec![0; LINES_PER_FRAME as usize];
        for _ in 0..70_224 {
            // An interrupt raised as LY changes belongs to the new line.
            if ppu.tick(1) & interrupt::STAT != 0 {
                counts[ppu.ly as usize] += 1;
            }
        }
        counts
    }

    #[test]
    fn lyc_interrupt_blocks_next_lines_mode2_interrupt() {
        // Mode 2 interrupts alone: one per visible line.
        let oam = stat_irqs_per_line(|p| p.write_reg(0xFF41, stat::OAM_INT));
        assert!(oam[1..144].iter().all(|&n| n == 1), "{oam:?}");
        // With LYC = 10 also enabled, line 10 raises the line and line 11's mode 2 source
        // takes over without a gap: no rising edge, so no interrupt on line 11.
        let both = stat_irqs_per_line(|p| {
            p.write_reg(0xFF45, 10);
            p.write_reg(0xFF41, stat::OAM_INT | stat::LYC_INT);
        });
        assert_eq!(both[10], 1, "LYC (and mode 2) on line 10");
        assert_eq!(both[11], 0, "line 11's mode 2 interrupt is blocked");
        assert_eq!(both[12], 1);
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

    #[test]
    fn boot_logo_is_decompressed_like_the_boot_rom() {
        let mut ppu = Ppu::new();
        let mut logo = [0u8; 48];
        logo[0] = 0xCE; // first byte of the Nintendo logo
        ppu.load_boot_logo(&logo);
        // $C = 1100 -> 11110000, $E = 1110 -> 11111100; each row twice, low plane only.
        assert_eq!(&ppu.vram[0x10..0x18], &[0xF0, 0, 0xF0, 0, 0xFC, 0, 0xFC, 0]);
        assert_eq!(ppu.vram[0x190], 0x3C); // ® tile
        assert_eq!(ppu.vram[0x1904], 1);
        assert_eq!(ppu.vram[0x1910], 0x19);
    }

    /// Dots STAT reports mode 3 on line 0 with the given setup.
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
