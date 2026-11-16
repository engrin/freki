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
//! Registers are read at the dot they are used, so mid-scanline changes show up exactly
//! where they happen. Some DMG register writes land slightly off the CPU's M-cycle grid;
//! the CPU handles that (see `Cpu::mem_write`).

use std::collections::VecDeque;

use crate::bus::interrupt;

pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;

const DOTS_PER_LINE: u32 = 456;
const LINES_PER_FRAME: u8 = 154;
/// STAT reports mode 3 from this dot (OAM scan takes 80 dots after the mode 2 interrupt).
const MODE3_STAT_DOT: u32 = 81;
/// First dot of the pixel pipeline.
const PIPELINE_START_DOT: u32 = 86;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    HBlank = 0,
    VBlank = 1,
    OamScan = 2,
    Drawing = 3,
}

/// Background fetcher state. VRAM accesses take two dots: the address is formed on the
/// first (`*1`) and the data read on the second (`*2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
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

/// Progress of a sprite fetch, which pauses pixel output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
}

/// A sprite selected during OAM scan for the current line.
#[derive(Debug, Clone, Copy)]
struct Sprite {
    y: u8,
    x: u8,
    tile: u8,
    attr: u8,
}

#[derive(Debug, Clone, Copy, Default)]
struct ObjPixel {
    color: u8,
    palette1: bool,
    behind_bg: bool,
}

pub struct Ppu {
    vram: [u8; 0x2000],
    oam: [u8; 0xA0],
    framebuffer: [u8; SCREEN_WIDTH * SCREEN_HEIGHT],
    frame_ready: bool,
    /// Dot position within the current scanline.
    dot: u32,
    /// Mode as reported by STAT.
    mode: Mode,
    /// Mode as seen by the STAT interrupt logic (HBlank's interrupt lags by one dot).
    irq_mode: Mode,
    /// Previous level of the combined STAT interrupt line (interrupts fire on rising edges).
    stat_line: bool,

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
    /// Window line counter; starts at −1 each frame and counts window activations.
    window_y: u8,
    window_being_fetched: bool,
    insert_bg_pixel: bool,
    disable_insertion_glitch: bool,
    wx_just_changed: bool,

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
            irq_mode: Mode::OamScan,
            stat_line: false,
            pipeline_active: false,
            bg_fifo: VecDeque::with_capacity(16),
            obj_fifo: VecDeque::with_capacity(16),
            position: POSITION_START,
            lcd_x: 0,
            fetch: Fetch::Tile1,
            tile_map_addr: 0,
            tile: 0,
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
            window_y: 0xFF,
            window_being_fetched: false,
            insert_bg_pixel: false,
            disable_insertion_glitch: false,
            wx_just_changed: false,
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

    /// Recreates the VRAM contents the DMG boot ROM leaves behind: the cartridge header's
    /// logo (`$0104–$0133`) decompressed into tiles 1–24, the ® symbol in tile 25, and
    /// their tile map entries. Some games and tests rely on these tiles being there.
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

    /// LY as read by the CPU and compared with LYC. Line 153 reads as 0 after its first
    /// 4 dots, so LYC = 0 matches during the last line of VBlank.
    fn ly_register(&self) -> u8 {
        if self.ly == LINES_PER_FRAME - 1 && self.dot >= 4 {
            0
        } else {
            self.ly
        }
    }

    pub fn read_reg(&self, addr: u16) -> u8 {
        match addr {
            0xFF40 => self.lcdc,
            0xFF41 => {
                let coincidence = u8::from(self.ly_register() == self.lyc) << 2;
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
                    self.irq_mode = Mode::HBlank;
                    self.pipeline_active = false;
                    self.window_y = 0xFF;
                    self.wy_triggered = false;
                    self.wx_triggered = false;
                } else if !was_on && self.lcd_on() {
                    self.mode = Mode::OamScan;
                    self.irq_mode = Mode::OamScan;
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

    /// Value LCDC holds for one T-cycle while the CPU writes it on a DMG: the old value
    /// with the fetcher-facing bits already new (OBJ enable clears early in some states).
    pub fn dmg_lcdc_write_conflict(&self, old: u8, value: u8) -> u8 {
        let mut old = old;
        let obj_off = value & lcdc::OBJ_ENABLE == 0;
        if obj_off && (self.position == 0 || self.during_object_fetch) {
            old &= !lcdc::OBJ_ENABLE;
        }
        // Fetcher-facing bits (maps, tile data, OBJ size) take the new value
        // at once; BG and OBJ enable keep their old value until the write completes.
        let early =
            lcdc::BG_MAP_9C00 | lcdc::TILE_DATA_8000 | lcdc::WINDOW_MAP_9C00 | lcdc::OBJ_SIZE_8X16;
        (old & !early) | (value & early)
    }

    /// Notes an LCDC write: disabling the window while it is being fetched turns off the
    /// DMG's window pixel-insertion glitch for the rest of the line.
    pub fn lcdc_written(&mut self, old: u8, value: u8) {
        // DMG: disabling objects while one is being fetched aborts that fetch.
        if old & lcdc::OBJ_ENABLE != 0 && value & lcdc::OBJ_ENABLE == 0 && self.during_object_fetch
        {
            self.obj_fetch = ObjFetch::Idle;
            self.during_object_fetch = false;
        }
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
            return irq;
        }
        for _ in 0..cycles {
            irq |= self.tick_dot();
        }
        irq
    }

    fn tick_dot(&mut self) -> u8 {
        let mut irq = 0;
        // HBlank's interrupt fires one dot after STAT reports mode 0.
        if self.mode == Mode::HBlank {
            self.irq_mode = Mode::HBlank;
        }
        if (self.ly as usize) < SCREEN_HEIGHT {
            match self.dot {
                0 if self.ly == 0 => {
                    // Line 0 is special on the DMG: STAT reads mode 0 for one dot and the
                    // OAM interrupt fires one dot later than on other lines.
                    self.mode = Mode::HBlank;
                    self.window_y = 0xFF;
                    self.wy_triggered = false;
                }
                0 | 1 if self.ly != 0 || self.dot == 1 => {
                    self.mode = Mode::OamScan;
                    self.irq_mode = Mode::OamScan;
                    if self.dot == 1 {
                        self.sprites.clear();
                    }
                }
                // OAM scan checks entry i at dot 2i + 3, with the OBJ size at that moment.
                3..=81 if self.dot % 2 == 1 => self.scan_oam_entry((self.dot as usize - 3) / 2),
                _ => {}
            }
            match self.dot {
                MODE3_STAT_DOT => {
                    self.mode = Mode::Drawing;
                    self.irq_mode = Mode::Drawing;
                    self.sprites.sort_by_key(|s| s.x); // stable: equal X keeps OAM order
                    self.next_sprite = 0;
                }
                PIPELINE_START_DOT => self.start_pipeline(),
                _ => {}
            }
            // The WY == LY comparison runs continuously (not just at line start), so
            // enabling the window or changing WY mid-line can still trigger it.
            if !self.wy_triggered && self.lcdc & lcdc::WINDOW_ENABLE != 0 && self.wy == self.ly {
                self.wy_triggered = true;
            }
            if self.pipeline_active {
                self.mode3_dot();
            }
        }

        self.dot += 1;
        if self.dot == DOTS_PER_LINE {
            self.dot = 0;
            self.ly += 1;
            if self.ly as usize == SCREEN_HEIGHT {
                self.mode = Mode::VBlank;
                self.irq_mode = Mode::VBlank;
                self.frame_ready = true;
                irq |= interrupt::VBLANK;
                // DMG quirk: entering VBlank also triggers the OAM (mode 2) STAT interrupt.
                if self.stat & stat::OAM_INT != 0 && !self.stat_line {
                    irq |= interrupt::STAT;
                }
            } else if self.ly == LINES_PER_FRAME {
                self.ly = 0;
            }
        }

        // STAT interrupt fires on a rising edge of the OR of all enabled sources.
        let line = (self.stat & stat::LYC_INT != 0 && self.ly_register() == self.lyc)
            || (self.stat & stat::HBLANK_INT != 0 && self.irq_mode == Mode::HBlank)
            || (self.stat & stat::VBLANK_INT != 0 && self.irq_mode == Mode::VBlank)
            || (self.stat & stat::OAM_INT != 0 && self.irq_mode == Mode::OamScan);
        if line && !self.stat_line {
            irq |= interrupt::STAT;
        }
        self.stat_line = line;
        irq
    }

    /// OAM scan step: selects entry `index` if it covers this line (max 10 per line).
    fn scan_oam_entry(&mut self, index: usize) {
        if index >= 40 || self.sprites.len() == MAX_SPRITES_PER_LINE {
            return;
        }
        let height: i16 = if self.lcdc & lcdc::OBJ_SIZE_8X16 != 0 {
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
            });
        }
    }

    fn start_pipeline(&mut self) {
        self.pipeline_active = true;
        self.bg_fifo.clear();
        self.obj_fifo.clear();
        self.bg_fifo.extend([0; 8]); // junk pixels, dropped at negative positions
        self.position = POSITION_START;
        self.lcd_x = 0;
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
        if self.wx == self.position.wrapping_add(7)
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
        if self.lcdc & lcdc::OBJ_ENABLE != 0
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

    /// X coordinate (in OAM terms) of sprites that start at the current position.
    fn x_for_object_match(&self) -> u8 {
        let x = self.position.wrapping_add(8);
        if x > POSITION_START { 0 } else { x }
    }

    /// Starts the window if WX matches the current position. Returns `true` if the rest
    /// of this dot is delayed by a pipeline stall.
    fn check_window(&mut self) -> bool {
        if self.wx_triggered || !self.wy_triggered || self.lcdc & lcdc::WINDOW_ENABLE == 0 {
            return false;
        }
        let pos = self.position;
        let activate = if self.wx == 0 {
            // pos == −7, or −16 with fine scroll, or anywhere in −15..=−8.
            pos == 0u8.wrapping_sub(7)
                || (pos == POSITION_START && self.scx & 7 != 0)
                || (POSITION_START.wrapping_add(1)..=0u8.wrapping_sub(8)).contains(&pos)
        } else if self.wx < 166 {
            if self.wx == pos.wrapping_add(7) {
                true
            } else if self.wx == pos.wrapping_add(6) && !self.wx_just_changed {
                // DMG LCD/PPU desync: the window starts a pixel early.
                self.lcd_x = self.lcd_x.saturating_sub(1);
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
        base + (self.fetcher_y() & 7) as usize * 2
    }

    /// One step of the background/window fetcher.
    fn advance_fetcher(&mut self) {
        match self.fetch {
            Fetch::Tile1 => {
                if self.lcdc & lcdc::WINDOW_ENABLE == 0 {
                    self.wx_triggered = false;
                }
                let high_map = if self.wx_triggered {
                    self.lcdc & lcdc::WINDOW_MAP_9C00 != 0
                } else {
                    self.lcdc & lcdc::BG_MAP_9C00 != 0
                };
                let x = if self.wx_triggered {
                    self.window_tile_x
                } else if self.position.wrapping_add(16) < 8 {
                    self.scx >> 3
                } else {
                    (self.scx.wrapping_add(self.position).wrapping_add(8) / 8) & 0x1F
                };
                let map = if high_map { 0x1C00 } else { 0x1800 };
                self.tile_map_addr = map + x as usize + (self.fetcher_y() / 8) as usize * 32;
                self.fetch = Fetch::Tile2;
            }
            Fetch::Tile2 => {
                self.tile = self.vram[self.tile_map_addr];
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
        if self.wy_triggered
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
        for bit in (0..8).rev() {
            self.bg_fifo
                .push_back((((hi >> bit) & 1) << 1) | ((lo >> bit) & 1));
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
        tile as usize * 16 + row as usize * 2
    }

    /// Mixes a sprite row into the object FIFO; earlier sprites keep their pixels.
    fn overlay_object(&mut self, lo: u8, hi: u8) {
        while self.obj_fifo.len() < 8 {
            self.obj_fifo.push_back(ObjPixel::default());
        }
        let attr = self.obj_attr[1];
        let flip = attr & 0x20 != 0;
        for i in 0..8u8 {
            let bit = if flip { i } else { 7 - i };
            let color = (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1);
            let target = &mut self.obj_fifo[i as usize];
            if color != 0 && target.color == 0 {
                *target = ObjPixel {
                    color,
                    palette1: attr & 0x10 != 0,
                    behind_bg: attr & 0x80 != 0,
                };
            }
        }
    }

    /// Shifts one pixel out of the FIFOs; draws it if its position is on screen.
    fn render_pixel(&mut self) {
        // Output stalls while a sprite at X = 0 is still to be fetched.
        if self.lcdc & lcdc::OBJ_ENABLE != 0
            && self.sprites.get(self.next_sprite).is_some_and(|s| s.x == 0)
        {
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

        let bg = if self.lcdc & lcdc::BG_WINDOW_ENABLE != 0 {
            bg
        } else {
            0
        };
        let draw_obj =
            obj.color != 0 && self.lcdc & lcdc::OBJ_ENABLE != 0 && !(obj.behind_bg && bg != 0);
        let shade = if draw_obj {
            let palette = if obj.palette1 { self.obp1 } else { self.obp0 };
            (palette >> (obj.color * 2)) & 3
        } else {
            (self.bgp >> (bg * 2)) & 3
        };
        if (self.lcd_x as usize) < SCREEN_WIDTH {
            self.framebuffer[self.ly as usize * SCREEN_WIDTH + self.lcd_x as usize] = shade;
        }
        self.position += 1;
        self.lcd_x += 1;
    }

    /// Mode 3 is over: switch to HBlank and prepare the window for the next line.
    fn end_line(&mut self) {
        self.pipeline_active = false;
        // If the LCD lost a pixel (window desync), repeat the last one to fill the line.
        let row = self.ly as usize * SCREEN_WIDTH;
        while (self.lcd_x as usize) < SCREEN_WIDTH {
            let x = self.lcd_x as usize;
            self.framebuffer[row + x] = if x == 0 {
                0
            } else {
                self.framebuffer[row + x - 1]
            };
            self.lcd_x += 1;
        }
        self.position = POSITION_START;
        if self.wy_triggered && self.lcdc & lcdc::WINDOW_ENABLE != 0 && self.wx == 166 {
            // WX = 166 starts the window at the end of this line for the next one.
            self.wx_triggered = true;
            self.window_tile_x = 1;
            self.window_y = self.window_y.wrapping_add(1);
        } else {
            self.wx_triggered = false;
        }
        self.mode = Mode::HBlank;
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
