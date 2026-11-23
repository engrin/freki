//! Debug views: tile data, background/window tile maps, sprites (OAM) and palettes. On
//! the Game Boy Color they show both VRAM banks, tile attributes and palette RAM.
//!
//! Everything is read through `GameBoy::peek`, which has no side effects, so the views
//! never disturb the running game.

use crate::window::tool_window;
use eframe::egui::{
    self, Color32, ColorImage, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions,
    load::SizedTexture, pos2, vec2,
};
use freki_core::GameBoy;

use crate::settings;

/// RGB for shades 0 (lightest) to 3 (darkest).
pub type Colors = [[u8; 3]; 4];

const LCDC: u16 = 0xFF40;
const SCY: u16 = 0xFF42;
const SCX: u16 = 0xFF43;
const BGP: u16 = 0xFF47;
const OBP0: u16 = 0xFF48;
const OBP1: u16 = 0xFF49;
const WY: u16 = 0xFF4A;
const WX: u16 = 0xFF4B;

/// Tile data: 384 tiles at $8000–$97FF, shown 16 per row.
pub const TILES_W: usize = 16 * 8;
pub const TILES_H: usize = 24 * 8;
pub const MAP_SIZE: usize = 256;
/// Sprite strip: 40 cells of 8×16 pixels side by side.
pub const SPRITES_W: usize = 40 * 8;
pub const SPRITES_H: usize = 16;

/// How the views turn colour indices into RGB.
pub enum Palettes {
    /// DMG (and the GBC's compatibility mode): shades through BGP/OBP0/OBP1, shown in
    /// the frontend's colours.
    Dmg(Colors),
    /// CGB mode: colours from palette RAM.
    Cgb {
        bg: [u8; 64],
        obj: [u8; 64],
        /// Apply GBC colour correction.
        correct: bool,
    },
}

impl Palettes {
    pub fn new(gb: &GameBoy, dmg_colors: Colors, correct: bool) -> Self {
        if gb.cgb_mode() {
            let (bg, obj) = gb.palette_ram();
            Palettes::Cgb {
                bg: *bg,
                obj: *obj,
                correct,
            }
        } else {
            Palettes::Dmg(dmg_colors)
        }
    }

    pub fn is_cgb(&self) -> bool {
        matches!(self, Palettes::Cgb { .. })
    }

    /// RGB of colour `index` in CGB palette `palette` of `ram`.
    fn ram_rgb(ram: &[u8; 64], palette: u8, index: u8, correct: bool) -> [u8; 3] {
        let i = (palette as usize & 7) * 8 + index as usize * 2;
        settings::cgb_color(u16::from_le_bytes([ram[i], ram[i + 1]]), correct)
    }

    /// A background pixel: through BGP on the DMG, BG palette `cgb_palette` on the CGB.
    fn bg_rgb(&self, gb: &GameBoy, cgb_palette: u8, index: u8) -> [u8; 3] {
        match self {
            Palettes::Dmg(colors) => colors[((gb.peek(BGP) >> (index * 2)) & 3) as usize],
            Palettes::Cgb { bg, correct, .. } => Self::ram_rgb(bg, cgb_palette, index, *correct),
        }
    }

    /// A sprite pixel: through OBP0/OBP1 (by `flags` bit 4) on the DMG, OBJ palette
    /// `flags & 7` on the CGB.
    fn obj_rgb(&self, gb: &GameBoy, flags: u8, index: u8) -> [u8; 3] {
        match self {
            Palettes::Dmg(colors) => {
                let obp = gb.peek(if flags & 0x10 != 0 { OBP1 } else { OBP0 });
                colors[((obp >> (index * 2)) & 3) as usize]
            }
            Palettes::Cgb { obj, correct, .. } => Self::ram_rgb(obj, flags & 7, index, *correct),
        }
    }
}

/// Which palette the tile view uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TilePalette {
    /// BGP on the DMG; BG palette n on the CGB.
    Bg(u8),
    /// OBJ palette n (CGB).
    Obj(u8),
}

/// Color index (0–3) of pixel (`x`, `y`) of the tile at `addr` in VRAM bank `bank`.
fn tile_pixel(gb: &GameBoy, bank: u8, addr: u16, x: u8, y: u8) -> u8 {
    let lo = gb.peek_vram(bank, addr + y as u16 * 2);
    let hi = gb.peek_vram(bank, addr + y as u16 * 2 + 1);
    let bit = 7 - x;
    (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1)
}

fn put(rgba: &mut [u8], width: usize, x: usize, y: usize, [r, g, b]: [u8; 3]) {
    let i = (y * width + x) * 4;
    rgba[i..i + 4].copy_from_slice(&[r, g, b, 0xFF]);
}

/// All 384 tiles of VRAM bank `bank` through `palette`.
pub fn render_tiles(gb: &GameBoy, palettes: &Palettes, bank: u8, palette: TilePalette) -> Vec<u8> {
    let mut rgba = vec![0; TILES_W * TILES_H * 4];
    for tile in 0..384u16 {
        let (tx, ty) = ((tile % 16) as usize * 8, (tile / 16) as usize * 8);
        for y in 0..8 {
            for x in 0..8 {
                let index = tile_pixel(gb, bank, 0x8000 + tile * 16, x, y);
                let rgb = match palette {
                    TilePalette::Bg(n) => palettes.bg_rgb(gb, n, index),
                    TilePalette::Obj(n) => palettes.obj_rgb(gb, n, index),
                };
                put(&mut rgba, TILES_W, tx + x as usize, ty + y as usize, rgb);
            }
        }
    }
    rgba
}

/// VRAM address of tile `number` with LCDC.4 addressing ($8000 unsigned or $9000 signed).
fn tile_addr(number: u8, unsigned_8000: bool) -> u16 {
    if unsigned_8000 {
        0x8000 + number as u16 * 16
    } else {
        (0x9000_i32 + (number as i8 as i32) * 16) as u16
    }
}

/// CGB tile attributes of map entry `addr` (bank 1); 0 on the DMG.
fn map_attributes(gb: &GameBoy, palettes: &Palettes, addr: u16) -> u8 {
    if palettes.is_cgb() {
        gb.peek_vram(1, addr)
    } else {
        0
    }
}

/// The 32×32 tile map at `map_base` ($9800 or $9C00) as a 256×256 image, with the tile
/// attributes (palette, bank, flips) applied on the CGB.
pub fn render_map(
    gb: &GameBoy,
    map_base: u16,
    unsigned_8000: bool,
    palettes: &Palettes,
) -> Vec<u8> {
    let mut rgba = vec![0; MAP_SIZE * MAP_SIZE * 4];
    for row in 0..32u16 {
        for col in 0..32u16 {
            let entry = map_base + row * 32 + col;
            let addr = tile_addr(gb.peek_vram(0, entry), unsigned_8000);
            let attr = map_attributes(gb, palettes, entry);
            let bank = (attr >> 3) & 1;
            for y in 0..8 {
                for x in 0..8 {
                    let tx = if attr & 0x20 != 0 { 7 - x } else { x };
                    let ty = if attr & 0x40 != 0 { 7 - y } else { y };
                    let index = tile_pixel(gb, bank, addr, tx, ty);
                    let (px, py) = (col as usize * 8 + x as usize, row as usize * 8 + y as usize);
                    put(
                        &mut rgba,
                        MAP_SIZE,
                        px,
                        py,
                        palettes.bg_rgb(gb, attr & 7, index),
                    );
                }
            }
        }
    }
    rgba
}

/// One OAM entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sprite {
    pub y: u8,
    pub x: u8,
    pub tile: u8,
    pub flags: u8,
}

impl Sprite {
    /// Whether any part of the sprite is on screen (with the given height).
    pub fn visible(&self, height: u8) -> bool {
        self.x > 0 && self.x < 168 && self.y + height > 16 && self.y < 160
    }
}

pub fn sprites(gb: &GameBoy) -> [Sprite; 40] {
    std::array::from_fn(|i| {
        let base = 0xFE00 + i as u16 * 4;
        Sprite {
            y: gb.peek(base),
            x: gb.peek(base + 1),
            tile: gb.peek(base + 2),
            flags: gb.peek(base + 3),
        }
    })
}

/// All 40 sprites as 8×16 cells side by side, each through its own palette (and, on the
/// CGB, from its own VRAM bank). With 8×8 sprites only the top half of each cell is used.
pub fn render_sprites(gb: &GameBoy, palettes: &Palettes, tall: bool) -> Vec<u8> {
    let mut rgba = vec![0; SPRITES_W * SPRITES_H * 4];
    for (i, s) in sprites(gb).iter().enumerate() {
        let bank = if palettes.is_cgb() {
            (s.flags >> 3) & 1
        } else {
            0
        };
        let rows = if tall { 16 } else { 8 };
        let first_tile = if tall { s.tile & 0xFE } else { s.tile };
        for y in 0..rows {
            for x in 0..8u8 {
                let sx = if s.flags & 0x20 != 0 { 7 - x } else { x };
                let sy = if s.flags & 0x40 != 0 { rows - 1 - y } else { y };
                let addr = 0x8000 + (first_tile as u16 + sy as u16 / 8) * 16;
                let index = tile_pixel(gb, bank, addr, sx, sy % 8);
                let rgb = palettes.obj_rgb(gb, s.flags, index);
                put(&mut rgba, SPRITES_W, i * 8 + x as usize, y as usize, rgb);
            }
        }
    }
    rgba
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapChoice {
    /// The map LCDC selects for the background.
    Background,
    /// The map LCDC selects for the window.
    Window,
    Map9800,
    Map9C00,
}

/// State of the debug windows.
pub struct DebugViews {
    pub show_tiles: bool,
    pub show_map: bool,
    pub show_sprites: bool,
    pub show_palettes: bool,
    map: MapChoice,
    /// VRAM bank and palette for the tile view (CGB).
    tile_bank: u8,
    tile_palette: TilePalette,
    tiles_tex: Option<TextureHandle>,
    map_tex: Option<TextureHandle>,
    sprites_tex: Option<TextureHandle>,
}

impl DebugViews {
    pub fn new() -> Self {
        Self {
            show_tiles: false,
            show_map: false,
            show_sprites: false,
            show_palettes: false,
            map: MapChoice::Background,
            tile_bank: 0,
            tile_palette: TilePalette::Bg(0),
            tiles_tex: None,
            map_tex: None,
            sprites_tex: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, gb: &GameBoy, palettes: &Palettes) {
        if self.show_tiles {
            self.tiles_window(ctx, gb, palettes);
        }
        if self.show_map {
            self.map_window(ctx, gb, palettes);
        }
        if self.show_sprites {
            self.sprites_window(ctx, gb, palettes);
        }
        if self.show_palettes {
            self.palettes_window(ctx, gb, palettes);
        }
    }

    fn tiles_window(&mut self, ctx: &egui::Context, gb: &GameBoy, palettes: &Palettes) {
        if !palettes.is_cgb() {
            self.tile_bank = 0;
            self.tile_palette = TilePalette::Bg(0);
        }
        let rgba = render_tiles(gb, palettes, self.tile_bank, self.tile_palette);
        let tex = upload(
            ctx,
            &mut self.tiles_tex,
            "debug_tiles",
            [TILES_W, TILES_H],
            &rgba,
        );
        let open = tool_window(ctx, "tiles", "freki — Tiles", [300.0, 560.0], |ui| {
            if palettes.is_cgb() {
                ui.horizontal(|ui| {
                    ui.label("VRAM bank");
                    ui.radio_value(&mut self.tile_bank, 0, "0");
                    ui.radio_value(&mut self.tile_bank, 1, "1");
                    ui.separator();
                    egui::ComboBox::from_id_salt("tile_palette")
                        .selected_text(palette_name(self.tile_palette))
                        .show_ui(ui, |ui| {
                            for n in 0..8 {
                                let p = TilePalette::Bg(n);
                                ui.selectable_value(&mut self.tile_palette, p, palette_name(p));
                            }
                            for n in 0..8 {
                                let p = TilePalette::Obj(n);
                                ui.selectable_value(&mut self.tile_palette, p, palette_name(p));
                            }
                        });
                });
            }
            // The largest whole scale that fits the window (room left for the
            // details under the picture), at least 1.
            let room = ui.available_size() - vec2(0.0, 60.0);
            let scale = (room.x / TILES_W as f32)
                .min(room.y / TILES_H as f32)
                .floor()
                .max(1.0);
            let size = vec2(TILES_W as f32, TILES_H as f32) * scale;
            let response = ui
                .add(egui::Image::from_texture(SizedTexture::new(tex, size)).sense(Sense::hover()));
            let hovered = response.hover_pos().map(|p| {
                let local = (p - response.rect.min) / scale;
                (local.y as u16 / 8) * 16 + local.x as u16 / 8
            });
            match hovered {
                Some(tile) if tile < 384 => {
                    let addr = 0x8000 + tile * 16;
                    let ids = if tile < 256 {
                        format!("${tile:02X} at $8000")
                    } else {
                        String::new()
                    };
                    let signed = if tile >= 128 {
                        format!("${:02X} at $9000", (tile - 256) as u8)
                    } else {
                        String::new()
                    };
                    let names = [ids, signed]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let bank = if palettes.is_cgb() {
                        format!(" (bank {})", self.tile_bank)
                    } else {
                        String::new()
                    };
                    ui.label(format!("tile ${addr:04X}{bank}: {names}"));
                }
                _ => {
                    ui.label("hover a tile for its address");
                }
            }
        });
        self.show_tiles = open;
    }

    fn map_window(&mut self, ctx: &egui::Context, gb: &GameBoy, palettes: &Palettes) {
        let lcdc = gb.peek(LCDC);
        let base = match self.map {
            MapChoice::Background => {
                if lcdc & 0x08 != 0 {
                    0x9C00
                } else {
                    0x9800
                }
            }
            MapChoice::Window => {
                if lcdc & 0x40 != 0 {
                    0x9C00
                } else {
                    0x9800
                }
            }
            MapChoice::Map9800 => 0x9800,
            MapChoice::Map9C00 => 0x9C00,
        };
        let rgba = render_map(gb, base, lcdc & 0x10 != 0, palettes);
        let tex = upload(
            ctx,
            &mut self.map_tex,
            "debug_map",
            [MAP_SIZE, MAP_SIZE],
            &rgba,
        );
        let open = tool_window(ctx, "map", "freki — Tile map", [560.0, 660.0], |ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.map, MapChoice::Background, "Background");
                ui.radio_value(&mut self.map, MapChoice::Window, "Window");
                ui.radio_value(&mut self.map, MapChoice::Map9800, "$9800");
                ui.radio_value(&mut self.map, MapChoice::Map9C00, "$9C00");
            });
            // The largest whole scale that fits the window (room left for the
            // details under the picture), at least 1.
            let room = ui.available_size() - vec2(0.0, 60.0);
            let scale = (room.x / MAP_SIZE as f32)
                .min(room.y / MAP_SIZE as f32)
                .floor()
                .max(1.0);
            let size = vec2(MAP_SIZE as f32, MAP_SIZE as f32) * scale;
            let response = ui
                .add(egui::Image::from_texture(SizedTexture::new(tex, size)).sense(Sense::hover()));
            let painter = ui.painter().with_clip_rect(response.rect);
            let stroke = Stroke::new(2.0, Color32::from_rgb(0xFF, 0x40, 0x40));
            if self.map == MapChoice::Background
                || (self.map != MapChoice::Window && base == bg_map(lcdc))
            {
                // Visible screen area, wrapping around the 256×256 map.
                let (scx, scy) = (gb.peek(SCX) as f32, gb.peek(SCY) as f32);
                for dx in [0.0, -256.0] {
                    for dy in [0.0, -256.0] {
                        let min = response.rect.min + vec2(scx + dx, scy + dy) * scale;
                        let rect = Rect::from_min_size(min, vec2(160.0, 144.0) * scale);
                        painter.rect_stroke(rect, 0.0, stroke, StrokeKind::Inside);
                    }
                }
            }
            let source = format!(
                "map ${base:04X}, tiles at {} · SCX {} SCY {} · WX {} WY {} · window {}",
                if lcdc & 0x10 != 0 {
                    "$8000"
                } else {
                    "$9000 (signed)"
                },
                gb.peek(SCX),
                gb.peek(SCY),
                gb.peek(WX),
                gb.peek(WY),
                if lcdc & 0x20 != 0 { "on" } else { "off" },
            );
            ui.label(source);
            if let Some(p) = response.hover_pos() {
                let local = (p - response.rect.min) / scale;
                let (col, row) = (local.x as u16 / 8, local.y as u16 / 8);
                let addr = base + row * 32 + col;
                let attributes = if palettes.is_cgb() {
                    let a = gb.peek_vram(1, addr);
                    let flag = |bit: u8, c: char| if a & bit != 0 { c } else { '·' };
                    format!(
                        " · palette {} bank {} {}{}{}",
                        a & 7,
                        (a >> 3) & 1,
                        flag(0x20, 'X'),
                        flag(0x40, 'Y'),
                        flag(0x80, 'P')
                    )
                } else {
                    String::new()
                };
                ui.label(format!(
                    "({col}, {row}) at ${addr:04X}: tile ${:02X}{attributes}",
                    gb.peek_vram(0, addr)
                ));
            }
        });
        self.show_map = open;
    }

    fn sprites_window(&mut self, ctx: &egui::Context, gb: &GameBoy, palettes: &Palettes) {
        let lcdc = gb.peek(LCDC);
        let tall = lcdc & 0x04 != 0;
        let cgb = palettes.is_cgb();
        let rgba = render_sprites(gb, palettes, tall);
        let tex = upload(
            ctx,
            &mut self.sprites_tex,
            "debug_sprites",
            [SPRITES_W, SPRITES_H],
            &rgba,
        );
        let height = if tall { 16 } else { 8 };
        let open = tool_window(ctx, "sprites", "freki — Sprites", [720.0, 480.0], |ui| {
            let palette_flag = if cgb {
                "number: palette, 1 VRAM bank 1"
            } else {
                "P OBP1"
            };
            ui.label(format!(
                    "8×{height} sprites, objects {} · flags: {palette_flag}, X/Y flip, B behind background",
                    if lcdc & 0x02 != 0 { "on" } else { "off" }
                ));
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("sprites").striped(true).show(ui, |ui| {
                    for title in ["#", "", "X", "Y", "tile", "flags", ""] {
                        ui.strong(title);
                    }
                    ui.end_row();
                    for (i, s) in sprites(gb).iter().enumerate() {
                        ui.label(i.to_string());
                        let cell = Rect::from_min_size(
                            pos2(i as f32 * 8.0 / SPRITES_W as f32, 0.0),
                            vec2(8.0 / SPRITES_W as f32, height as f32 / SPRITES_H as f32),
                        );
                        let size = vec2(16.0, height as f32 * 2.0);
                        ui.add(egui::Image::from_texture(SizedTexture::new(tex, size)).uv(cell));
                        ui.monospace(format!("{:>3}", s.x));
                        ui.monospace(format!("{:>3}", s.y));
                        ui.monospace(format!("${:02X}", s.tile));
                        let flag = |bit: u8, c: char| if s.flags & bit != 0 { c } else { '·' };
                        let palette = if cgb {
                            format!("{}{}", s.flags & 7, flag(0x08, '1'))
                        } else {
                            flag(0x10, 'P').to_string()
                        };
                        ui.monospace(format!(
                            "{palette}{}{}{}",
                            flag(0x20, 'X'),
                            flag(0x40, 'Y'),
                            flag(0x80, 'B')
                        ));
                        ui.label(if s.visible(height) { "on screen" } else { "" });
                        ui.end_row();
                    }
                });
            });
        });
        self.show_sprites = open;
    }

    fn palettes_window(&mut self, ctx: &egui::Context, gb: &GameBoy, palettes: &Palettes) {
        let open =
            tool_window(
                ctx,
                "palettes",
                "freki — Palettes",
                [400.0, 330.0],
                |ui| match palettes {
                    Palettes::Dmg(colors) => {
                        egui::Grid::new("dmg_palettes").show(ui, |ui| {
                            for (name, addr) in [("BGP", BGP), ("OBP0", OBP0), ("OBP1", OBP1)] {
                                let value = gb.peek(addr);
                                ui.monospace(format!("{name} ${value:02X}"));
                                for index in 0..4 {
                                    let shade = (value >> (index * 2)) & 3;
                                    swatch(
                                        ui,
                                        colors[shade as usize],
                                        format!("colour {index}: shade {shade}"),
                                    );
                                }
                                ui.end_row();
                            }
                        });
                    }
                    Palettes::Cgb { bg, obj, correct } => {
                        egui::Grid::new("cgb_palettes").show(ui, |ui| {
                            for n in 0..8u8 {
                                for (kind, ram) in [("BG", bg), ("OBJ", obj)] {
                                    ui.monospace(format!("{kind} {n}"));
                                    for index in 0..4u8 {
                                        let i = n as usize * 8 + index as usize * 2;
                                        let color =
                                            u16::from_le_bytes([ram[i], ram[i + 1]]) & 0x7FFF;
                                        let rgb = settings::cgb_color(color, *correct);
                                        swatch(ui, rgb, format!("colour {index}: ${color:04X}"));
                                    }
                                    ui.add_space(12.0);
                                }
                                ui.end_row();
                            }
                        });
                        ui.weak("Hover a colour for its RGB555 value.");
                    }
                },
            );
        self.show_palettes = open;
    }
}

fn palette_name(p: TilePalette) -> String {
    match p {
        TilePalette::Bg(n) => format!("BG palette {n}"),
        TilePalette::Obj(n) => format!("OBJ palette {n}"),
    }
}

fn swatch(ui: &mut egui::Ui, [r, g, b]: [u8; 3], tooltip: String) {
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, 2.0, Color32::from_rgb(r, g, b));
    response.on_hover_text(tooltip);
}

fn bg_map(lcdc: u8) -> u16 {
    if lcdc & 0x08 != 0 { 0x9C00 } else { 0x9800 }
}

/// Creates or updates a nearest-filtered texture; returns its id.
fn upload(
    ctx: &egui::Context,
    slot: &mut Option<TextureHandle>,
    name: &str,
    size: [usize; 2],
    rgba: &[u8],
) -> egui::TextureId {
    let image = ColorImage::from_rgba_unmultiplied(size, rgba);
    match slot {
        Some(tex) => tex.set(image, TextureOptions::NEAREST),
        None => *slot = Some(ctx.load_texture(name, image, TextureOptions::NEAREST)),
    }
    slot.as_ref()
        .map(TextureHandle::id)
        .expect("texture just set")
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLORS: Colors = [[255, 255, 255], [170, 170, 170], [85, 85, 85], [0, 0, 0]];
    const IDENTITY: u8 = 0b1110_0100;

    fn gb() -> GameBoy {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
        rom[0x150..0x152].copy_from_slice(&[0x18, 0xFE]);
        let mut gb = GameBoy::new(rom).unwrap();
        gb.poke(LCDC, 0x00); // LCD off so VRAM and OAM are freely writable
        for a in 0x8000..0xA000u16 {
            gb.poke(a, 0);
        }
        gb
    }

    fn pixel(rgba: &[u8], width: usize, x: usize, y: usize) -> u8 {
        rgba[(y * width + x) * 4]
    }

    #[test]
    fn tiles_view_decodes_bitplanes() {
        let mut gb = gb();
        // Tile 1, row 0: low plane $80, high plane $C0 -> pixels 3, 2, 0, …
        gb.poke(0x8010, 0x80);
        gb.poke(0x8011, 0xC0);
        gb.poke(BGP, IDENTITY);
        let rgba = render_tiles(&gb, &Palettes::Dmg(COLORS), 0, TilePalette::Bg(0));
        assert_eq!(pixel(&rgba, TILES_W, 8, 0), 0); // shade 3 = black
        assert_eq!(pixel(&rgba, TILES_W, 9, 0), 85); // shade 2
        assert_eq!(pixel(&rgba, TILES_W, 10, 0), 255); // shade 0
    }

    #[test]
    fn map_view_uses_signed_addressing() {
        let mut gb = gb();
        // Tile $01 is at $9010 with signed ($9000) addressing and $8010 with unsigned.
        gb.poke(0x9800, 0x01);
        gb.poke(0x9010, 0xFF);
        gb.poke(0x9011, 0xFF);
        gb.poke(BGP, IDENTITY);
        let signed = render_map(&gb, 0x9800, false, &Palettes::Dmg(COLORS));
        assert_eq!(pixel(&signed, MAP_SIZE, 0, 0), 0);
        let unsigned = render_map(&gb, 0x9800, true, &Palettes::Dmg(COLORS));
        assert_eq!(
            pixel(&unsigned, MAP_SIZE, 0, 0),
            255,
            "tile at $8010 is empty"
        );
    }

    #[test]
    fn sprite_view_flips_and_reads_oam() {
        let mut gb = gb();
        gb.poke(0xFE04, 16); // sprite 1: y
        gb.poke(0xFE05, 8); // x
        gb.poke(0xFE06, 2); // tile
        gb.poke(0xFE07, 0x20); // X flip
        gb.poke(0x8020, 0x80); // tile 2 row 0: leftmost pixel color 1
        gb.poke(OBP0, IDENTITY);
        let s = sprites(&gb)[1];
        assert_eq!((s.x, s.y, s.tile, s.flags), (8, 16, 2, 0x20));
        assert!(s.visible(8));
        let rgba = render_sprites(&gb, &Palettes::Dmg(COLORS), false);
        assert_eq!(
            pixel(&rgba, SPRITES_W, 8 + 7, 0),
            170,
            "flipped to the right edge"
        );
        assert_eq!(pixel(&rgba, SPRITES_W, 8, 0), 255);
    }

    #[test]
    fn cgb_map_view_applies_tile_attributes() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
        rom[0x143] = 0x80; // CGB game
        rom[0x150..0x152].copy_from_slice(&[0x18, 0xFE]);
        let mut gb = GameBoy::new(rom).unwrap();
        gb.poke(LCDC, 0x00);
        // BG palette 1, colour 1 = pure red ($001F).
        gb.poke(0xFF68, 0x80 | 10); // auto-increment, palette 1 colour 1
        gb.poke(0xFF69, 0x1F);
        gb.poke(0xFF69, 0x00);
        // Tile 1 in VRAM bank 1: first row all colour 1.
        gb.poke(0xFF4F, 1);
        gb.poke(0x8010, 0xFF);
        gb.poke(0x9800, 0x09); // attributes of map entry 0: palette 1, bank 1
        gb.poke(0xFF4F, 0);
        gb.poke(0x9800, 0x01); // tile number 1
        let palettes = Palettes::new(&gb, COLORS, false);
        assert!(palettes.is_cgb());
        let rgba = render_map(&gb, 0x9800, true, &palettes);
        assert_eq!(&rgba[..3], &[255, 0, 0], "bank 1 tile through BG palette 1");
    }
}
