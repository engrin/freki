//! Debug views: tile data, background/window tile maps and sprites (OAM).
//!
//! Everything is read through `GameBoy::peek`, which has no side effects, so the views
//! never disturb the running game.

use eframe::egui::{
    self, Color32, ColorImage, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions,
    load::SizedTexture, pos2, vec2,
};
use freki_core::GameBoy;

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

/// Color index (0–3) of pixel (`x`, `y`) of the tile at `addr`.
fn tile_pixel(gb: &GameBoy, addr: u16, x: u8, y: u8) -> u8 {
    let lo = gb.peek(addr + y as u16 * 2);
    let hi = gb.peek(addr + y as u16 * 2 + 1);
    let bit = 7 - x;
    (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1)
}

fn put(rgba: &mut [u8], width: usize, x: usize, y: usize, colors: &Colors, palette: u8, index: u8) {
    let shade = (palette >> (index * 2)) & 3;
    let [r, g, b] = colors[shade as usize];
    let i = (y * width + x) * 4;
    rgba[i..i + 4].copy_from_slice(&[r, g, b, 0xFF]);
}

/// All 384 tiles through `palette` (usually BGP).
pub fn render_tiles(gb: &GameBoy, colors: &Colors, palette: u8) -> Vec<u8> {
    let mut rgba = vec![0; TILES_W * TILES_H * 4];
    for tile in 0..384u16 {
        let (tx, ty) = ((tile % 16) as usize * 8, (tile / 16) as usize * 8);
        for y in 0..8 {
            for x in 0..8 {
                let index = tile_pixel(gb, 0x8000 + tile * 16, x, y);
                put(
                    &mut rgba,
                    TILES_W,
                    tx + x as usize,
                    ty + y as usize,
                    colors,
                    palette,
                    index,
                );
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

/// The 32×32 tile map at `map_base` ($9800 or $9C00) as a 256×256 image.
pub fn render_map(
    gb: &GameBoy,
    map_base: u16,
    unsigned_8000: bool,
    colors: &Colors,
    palette: u8,
) -> Vec<u8> {
    let mut rgba = vec![0; MAP_SIZE * MAP_SIZE * 4];
    for row in 0..32u16 {
        for col in 0..32u16 {
            let addr = tile_addr(gb.peek(map_base + row * 32 + col), unsigned_8000);
            for y in 0..8 {
                for x in 0..8 {
                    let index = tile_pixel(gb, addr, x, y);
                    let (px, py) = (col as usize * 8 + x as usize, row as usize * 8 + y as usize);
                    put(&mut rgba, MAP_SIZE, px, py, colors, palette, index);
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

/// All 40 sprites as 8×16 cells side by side, each through its own OBP palette.
/// With 8×8 sprites only the top half of each cell is used.
pub fn render_sprites(gb: &GameBoy, colors: &Colors, tall: bool) -> Vec<u8> {
    let mut rgba = vec![0; SPRITES_W * SPRITES_H * 4];
    let (obp0, obp1) = (gb.peek(OBP0), gb.peek(OBP1));
    for (i, s) in sprites(gb).iter().enumerate() {
        let palette = if s.flags & 0x10 != 0 { obp1 } else { obp0 };
        let rows = if tall { 16 } else { 8 };
        let first_tile = if tall { s.tile & 0xFE } else { s.tile };
        for y in 0..rows {
            for x in 0..8u8 {
                let sx = if s.flags & 0x20 != 0 { 7 - x } else { x };
                let sy = if s.flags & 0x40 != 0 { rows - 1 - y } else { y };
                let addr = 0x8000 + (first_tile as u16 + sy as u16 / 8) * 16;
                let index = tile_pixel(gb, addr, sx, sy % 8);
                put(
                    &mut rgba,
                    SPRITES_W,
                    i * 8 + x as usize,
                    y as usize,
                    colors,
                    palette,
                    index,
                );
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
    map: MapChoice,
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
            map: MapChoice::Background,
            tiles_tex: None,
            map_tex: None,
            sprites_tex: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, gb: &GameBoy, colors: &Colors) {
        if self.show_tiles {
            self.tiles_window(ctx, gb, colors);
        }
        if self.show_map {
            self.map_window(ctx, gb, colors);
        }
        if self.show_sprites {
            self.sprites_window(ctx, gb, colors);
        }
    }

    fn tiles_window(&mut self, ctx: &egui::Context, gb: &GameBoy, colors: &Colors) {
        let rgba = render_tiles(gb, colors, gb.peek(BGP));
        let tex = upload(
            ctx,
            &mut self.tiles_tex,
            "debug_tiles",
            [TILES_W, TILES_H],
            &rgba,
        );
        let mut open = true;
        egui::Window::new("Tiles")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                let scale = 2.0;
                let size = vec2(TILES_W as f32, TILES_H as f32) * scale;
                let response = ui.add(
                    egui::Image::from_texture(SizedTexture::new(tex, size)).sense(Sense::hover()),
                );
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
                        ui.label(format!("tile ${addr:04X}: {names}"));
                    }
                    _ => {
                        ui.label("hover a tile for its address");
                    }
                }
            });
        self.show_tiles = open;
    }

    fn map_window(&mut self, ctx: &egui::Context, gb: &GameBoy, colors: &Colors) {
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
        let rgba = render_map(gb, base, lcdc & 0x10 != 0, colors, gb.peek(BGP));
        let tex = upload(
            ctx,
            &mut self.map_tex,
            "debug_map",
            [MAP_SIZE, MAP_SIZE],
            &rgba,
        );
        let mut open = true;
        egui::Window::new("Tile map")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.map, MapChoice::Background, "Background");
                    ui.radio_value(&mut self.map, MapChoice::Window, "Window");
                    ui.radio_value(&mut self.map, MapChoice::Map9800, "$9800");
                    ui.radio_value(&mut self.map, MapChoice::Map9C00, "$9C00");
                });
                let scale = 2.0;
                let size = vec2(MAP_SIZE as f32, MAP_SIZE as f32) * scale;
                let response = ui.add(
                    egui::Image::from_texture(SizedTexture::new(tex, size)).sense(Sense::hover()),
                );
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
                    ui.label(format!(
                        "({col}, {row}) at ${addr:04X}: tile ${:02X}",
                        gb.peek(addr)
                    ));
                }
            });
        self.show_map = open;
    }

    fn sprites_window(&mut self, ctx: &egui::Context, gb: &GameBoy, colors: &Colors) {
        let lcdc = gb.peek(LCDC);
        let tall = lcdc & 0x04 != 0;
        let rgba = render_sprites(gb, colors, tall);
        let tex = upload(
            ctx,
            &mut self.sprites_tex,
            "debug_sprites",
            [SPRITES_W, SPRITES_H],
            &rgba,
        );
        let height = if tall { 16 } else { 8 };
        let mut open = true;
        egui::Window::new("Sprites")
            .open(&mut open)
            .resizable(true)
            .default_height(420.0)
            .show(ctx, |ui| {
                ui.label(format!(
                "8×{height} sprites, objects {} · flags: P palette, X/Y flip, B behind background",
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
                            ui.add(
                                egui::Image::from_texture(SizedTexture::new(tex, size)).uv(cell),
                            );
                            ui.monospace(format!("{:>3}", s.x));
                            ui.monospace(format!("{:>3}", s.y));
                            ui.monospace(format!("${:02X}", s.tile));
                            let flag = |bit: u8, c: char| if s.flags & bit != 0 { c } else { '·' };
                            ui.monospace(format!(
                                "{}{}{}{}",
                                flag(0x10, 'P'),
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
        let rgba = render_tiles(&gb, &COLORS, IDENTITY);
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
        let signed = render_map(&gb, 0x9800, false, &COLORS, IDENTITY);
        assert_eq!(pixel(&signed, MAP_SIZE, 0, 0), 0);
        let unsigned = render_map(&gb, 0x9800, true, &COLORS, IDENTITY);
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
        let rgba = render_sprites(&gb, &COLORS, false);
        assert_eq!(
            pixel(&rgba, SPRITES_W, 8 + 7, 0),
            170,
            "flipped to the right edge"
        );
        assert_eq!(pixel(&rgba, SPRITES_W, 8, 0), 255);
    }
}
