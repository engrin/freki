//! Settings kept between runs (in eframe's storage: the platform's app data directory).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use freki_core::{BootRom, Model, Screen, rgb555_to_rgb888};

use crate::input::Bindings;
use crate::session::Start;
use serde::{Deserialize, Serialize};

const KEY: &str = "freki-settings";
/// Entries kept in File → Recent.
const MAX_RECENT: usize = 10;
/// Fast-forward speeds offered in the Emulation menu.
pub const FAST_FORWARD_SPEEDS: [u32; 4] = [2, 3, 4, 8];

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Which console games run on.
    pub console: ConsoleModel,
    /// DMG boot ROM file chosen by the user.
    pub boot_rom_path: Option<PathBuf>,
    /// Game Boy Color boot ROM file chosen by the user.
    pub cgb_boot_rom_path: Option<PathBuf>,
    /// The Super Game Boy cartridge's SNES ROM, for the SGB's own sound.
    pub sgb_rom_path: Option<PathBuf>,
    /// Whether games start with the boot ROM (of the console they run on).
    pub use_boot_rom: bool,
    pub palette: Palette,
    /// Window scale (the window size itself is restored by eframe).
    pub scale: u32,
    /// Recently opened ROMs, most recent first.
    pub recent: Vec<PathBuf>,
    /// Emulation speed multiplier while fast-forwarding.
    pub fast_forward_speed: u32,
    /// Make Game Boy Color colours look like on its LCD (less saturated).
    pub color_correction: bool,
    /// Keyboard and gamepad controls.
    pub bindings: Bindings,
    /// Rumble cartridges shake the gamepad.
    pub rumble: bool,
    /// Scale the screen by whole numbers only (sharp, even pixels; black borders).
    pub integer_scaling: bool,
    /// Show each frame mixed with the one before, like the slow LCD of the real
    /// console (sprites that flicker every other frame look steady or see-through).
    pub frame_blend: bool,
    /// Settings kept for single games, by [`game_key`].
    pub games: BTreeMap<String, GameSettings>,
    /// Sound volume in percent (0–100).
    pub volume: u8,
    pub muted: bool,
    /// Pause while no freki window has focus.
    pub pause_in_background: bool,
    /// The picture the Game Boy Camera sees (`None`: its test pattern).
    pub camera_picture: Option<PathBuf>,
    /// The webcam the Game Boy Camera sees, by name; before `camera_picture`.
    pub camera_webcam: Option<String>,
}

/// The settings a game can have for itself (Emulation → Settings for this game).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSettings {
    pub console: ConsoleModel,
    pub palette: Palette,
    pub color_correction: bool,
    pub frame_blend: bool,
}

impl GameSettings {
    /// The values `settings` uses now.
    pub fn of(settings: &Settings) -> Self {
        Self {
            console: settings.console,
            palette: settings.palette,
            color_correction: settings.color_correction,
            frame_blend: settings.frame_blend,
        }
    }

    /// Makes `settings` use these values.
    pub fn apply(&self, settings: &mut Settings) {
        settings.console = self.console;
        settings.palette = self.palette;
        settings.color_correction = self.color_correction;
        settings.frame_blend = self.frame_blend;
    }
}

/// What identifies a game for its own settings: the header title and global checksum
/// (so renaming the file keeps them), e.g. `DRAGONWARRIORM-1A2B`.
pub fn game_key(rom: &[u8]) -> Option<String> {
    let header = rom.get(0x134..0x150)?;
    let title: String = header[..16]
        .iter()
        .take_while(|&&b| b != 0)
        .filter(|b| b.is_ascii_graphic() || **b == b' ')
        .map(|&b| b as char)
        .collect();
    let checksum = u16::from_be_bytes([header[0x1A], header[0x1B]]);
    Some(format!("{}-{checksum:04X}", title.trim()))
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            console: ConsoleModel::Auto,
            boot_rom_path: None,
            cgb_boot_rom_path: None,
            sgb_rom_path: None,
            use_boot_rom: false,
            palette: Palette::default(),
            scale: 4,
            recent: Vec::new(),
            fast_forward_speed: 4,
            color_correction: true,
            bindings: Bindings::default(),
            rumble: true,
            integer_scaling: true,
            frame_blend: false,
            games: BTreeMap::new(),
            volume: 100,
            muted: false,
            pause_in_background: true,
            camera_picture: None,
            camera_webcam: None,
        }
    }
}

impl Settings {
    /// Factor for the samples heard: off when muted, and the volume on a square curve, so
    /// the slider's steps sound even.
    pub fn gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            (f32::from(self.volume.min(100)) / 100.0).powi(2)
        }
    }

    /// Puts `path` at the top of the recent ROMs.
    pub fn add_recent(&mut self, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        self.recent.retain(|p| *p != path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
    }

    pub fn remove_recent(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
    }

    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        storage
            .and_then(|s| eframe::get_value(s, KEY))
            .unwrap_or_default()
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, KEY, self);
    }

    /// How games start: console and boot ROMs (if enabled). Problems with the chosen
    /// boot ROM files are returned as messages; those boot ROMs are then left out.
    pub fn start(&self) -> (Start, Vec<String>) {
        let mut problems = Vec::new();
        let mut load = |path: &Option<PathBuf>| {
            let path = path.as_ref().filter(|_| self.use_boot_rom)?;
            read_boot_rom(path)
                .map_err(|e| problems.push(format!("boot ROM not used: {e}")))
                .ok()
        };
        let dmg_boot_rom = load(&self.boot_rom_path);
        let cgb_boot_rom = load(&self.cgb_boot_rom_path);
        let sgb_rom = self.sgb_rom_path.as_ref().and_then(|path| {
            read_sgb_rom(path)
                .map_err(|e| problems.push(format!("Super Game Boy sound off: {e}")))
                .ok()
        });
        let start = Start {
            model: self.console.model(),
            dmg_boot_rom,
            cgb_boot_rom,
            sgb_rom,
        };
        (start, problems)
    }
}

/// Reads the Super Game Boy's SNES ROM, checking that it holds the sound engine.
pub fn read_sgb_rom(path: &Path) -> Result<std::sync::Arc<Vec<u8>>, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if freki_core::sgb_sound::find_engine(&bytes).is_none() {
        return Err(format!(
            "{} is not a Super Game Boy SNES ROM (no sound engine in it)",
            path.display()
        ));
    }
    Ok(std::sync::Arc::new(bytes))
}

/// Which console games run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ConsoleModel {
    /// Game Boy Color for colour games, Game Boy for the rest.
    #[default]
    Auto,
    GameBoy,
    /// Also original Game Boy games, coloured like a real GBC does.
    GameBoyColor,
    /// Every game in a Super Game Boy, with its colours and border where the game has them.
    SuperGameBoy,
}

impl ConsoleModel {
    pub const ALL: [ConsoleModel; 4] = [
        ConsoleModel::Auto,
        ConsoleModel::GameBoy,
        ConsoleModel::GameBoyColor,
        ConsoleModel::SuperGameBoy,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ConsoleModel::Auto => "Auto (GBC for colour games, SGB for SGB games)",
            ConsoleModel::GameBoy => "Game Boy",
            ConsoleModel::GameBoyColor => "Game Boy Color",
            ConsoleModel::SuperGameBoy => "Super Game Boy",
        }
    }

    /// The model to use, or `None` to choose per game.
    fn model(self) -> Option<Model> {
        match self {
            ConsoleModel::Auto => None,
            ConsoleModel::GameBoy => Some(Model::Dmg),
            ConsoleModel::GameBoyColor => Some(Model::Cgb),
            ConsoleModel::SuperGameBoy => Some(Model::Sgb),
        }
    }
}

pub fn read_boot_rom(path: &Path) -> Result<BootRom, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    BootRom::new(&bytes).map_err(|e| e.to_string())
}

/// Colours for the four DMG shades.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Palette {
    #[default]
    Green,
    Grey,
    Pocket,
    Light,
}

impl Palette {
    pub const ALL: [Palette; 4] = [
        Palette::Green,
        Palette::Grey,
        Palette::Pocket,
        Palette::Light,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Palette::Green => "DMG green",
            Palette::Grey => "Greyscale",
            Palette::Pocket => "Pocket",
            Palette::Light => "Light (backlit)",
        }
    }

    /// RGB for shades 0 (lightest) to 3 (darkest).
    pub fn colors(self) -> [[u8; 3]; 4] {
        match self {
            Palette::Green => [
                [0xE0, 0xF8, 0xD0],
                [0x88, 0xC0, 0x70],
                [0x34, 0x68, 0x56],
                [0x08, 0x18, 0x20],
            ],
            Palette::Grey => [
                [0xFF, 0xFF, 0xFF],
                [0xAA, 0xAA, 0xAA],
                [0x55, 0x55, 0x55],
                [0x00, 0x00, 0x00],
            ],
            Palette::Pocket => [
                [0xC4, 0xCF, 0xA1],
                [0x8B, 0x95, 0x6D],
                [0x4D, 0x53, 0x3C],
                [0x1F, 0x1F, 0x1F],
            ],
            Palette::Light => [
                [0x7B, 0xE8, 0xC8],
                [0x3C, 0xB8, 0x98],
                [0x16, 0x76, 0x62],
                [0x05, 0x34, 0x2C],
            ],
        }
    }
}

/// The picture as RGBA, with its width and height: DMG shades in `palette`, GBC colours
/// (optionally corrected), SGB colours as the SNES outputs them.
pub fn screen_rgba(screen: Screen, palette: Palette, correct: bool) -> (Vec<u8>, [usize; 2]) {
    let (width, height) = screen.size();
    let mut rgba = Vec::with_capacity(width * height * 4);
    let mut push = |[r, g, b]: [u8; 3]| rgba.extend([r, g, b, 0xFF]);
    match screen {
        Screen::Shades(shades) => {
            let colors = palette.colors();
            shades.iter().for_each(|&s| push(colors[(s & 3) as usize]));
        }
        Screen::Rgb555(pixels) => pixels.iter().for_each(|&c| push(cgb_color(c, correct))),
        Screen::Sgb(pixels) => pixels.iter().for_each(|&c| push(rgb555_to_rgb888(c))),
    }
    (rgba, [width, height])
}

/// Mixes `previous` (RGBA) half and half into `current`: frame blending.
pub fn blend(previous: &[u8], current: &mut [u8]) {
    for (c, &p) in current.iter_mut().zip(previous) {
        *c = (u16::from(*c) + u16::from(p)).div_ceil(2) as u8;
    }
}

/// Pixels between the two screens of linked Game Boys.
pub const LINK_GAP: usize = 8;

/// Two screens (RGBA with their sizes) side by side, `LINK_GAP` pixels apart and centred
/// vertically on a dark background: the picture of two linked Game Boys.
pub fn side_by_side(
    (left, [lw, lh]): (Vec<u8>, [usize; 2]),
    (right, [rw, rh]): (Vec<u8>, [usize; 2]),
) -> (Vec<u8>, [usize; 2]) {
    let (width, height) = (lw + LINK_GAP + rw, lh.max(rh));
    let mut rgba = [0x20, 0x20, 0x20, 0xFF].repeat(width * height);
    for (pixels, w, h, x0) in [(&left, lw, lh, 0), (&right, rw, rh, lw + LINK_GAP)] {
        let y0 = (height - h) / 2;
        for y in 0..h {
            let at = ((y0 + y) * width + x0) * 4;
            rgba[at..at + w * 4].copy_from_slice(&pixels[y * w * 4..(y + 1) * w * 4]);
        }
    }
    (rgba, [width, height])
}

/// Converts a Game Boy Color RGB555 colour to 8-bit RGB. With `correct`, approximates the
/// colours of the GBC's LCD, which mixes channels and is less saturated than a modern
/// screen (the formula used by Gambatte and higan); otherwise scales each channel.
pub fn cgb_color(color: u16, correct: bool) -> [u8; 3] {
    if !correct {
        return freki_core::rgb555_to_rgb888(color);
    }
    let r = (color & 0x1F) as u32;
    let g = ((color >> 5) & 0x1F) as u32;
    let b = ((color >> 10) & 0x1F) as u32;
    let mix = |v: u32| (v.min(960) >> 2) as u8;
    [
        mix(r * 26 + g * 4 + b * 2),
        mix(g * 24 + b * 8),
        mix(r * 6 + g * 4 + b * 22),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn games_are_known_by_title_and_checksum() {
        let mut rom = vec![0u8; 0x150];
        rom[0x134..0x13F].copy_from_slice(b"DONKEY KONG");
        rom[0x14E..0x150].copy_from_slice(&[0x12, 0x34]);
        assert_eq!(game_key(&rom).as_deref(), Some("DONKEY KONG-1234"));
        assert_eq!(game_key(&rom[..0x100]), None);
    }

    #[test]
    fn game_settings_round_trip() {
        let mut settings = Settings::default();
        let mine = GameSettings {
            console: ConsoleModel::ALL[3],
            palette: Palette::ALL[2],
            color_correction: false,
            frame_blend: true,
        };
        mine.apply(&mut settings);
        assert_eq!(GameSettings::of(&settings), mine);
    }

    #[test]
    fn blending_averages_two_frames() {
        let previous = [0, 255, 100, 255];
        let mut current = [255, 255, 50, 255];
        blend(&previous, &mut current);
        assert_eq!(current, [128, 255, 75, 255]);
    }

    #[test]
    fn linked_screens_sit_side_by_side() {
        let red = ([255, 0, 0, 255].repeat(2 * 2), [2, 2]);
        let blue = ([0, 0, 255, 255].repeat(3 * 4), [3, 4]);
        let (rgba, [w, h]) = side_by_side(red, blue);
        assert_eq!([w, h], [2 + LINK_GAP + 3, 4]);
        let px = |x: usize, y: usize| &rgba[(y * w + x) * 4..][..3];
        assert_eq!(px(0, 1), [255, 0, 0], "left screen centred vertically");
        assert_eq!(px(0, 0), [0x20; 3], "background above it");
        assert_eq!(px(2 + LINK_GAP, 0), [0, 0, 255]);
        assert_eq!(px(w - 1, 3), [0, 0, 255]);
    }

    #[test]
    fn colour_correction_keeps_black_and_white() {
        assert_eq!(cgb_color(0, true), [0, 0, 0]);
        assert_eq!(cgb_color(0x7FFF, true), [240, 240, 240]);
        assert_eq!(cgb_color(0x7FFF, false), [255, 255, 255]);
        let [r, g, b] = cgb_color(0x001F, true); // pure red is muted
        assert!(r > g && r > b && g < 64);
    }

    #[test]
    fn recent_roms_are_deduplicated_and_capped() {
        let mut s = Settings::default();
        for i in 0..12 {
            s.add_recent(Path::new(&format!("/roms/{i}.gb")));
        }
        s.add_recent(Path::new("/roms/5.gb"));
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0], Path::new("/roms/5.gb"));
        assert_eq!(s.recent.iter().filter(|p| p.ends_with("5.gb")).count(), 1);
    }

    #[test]
    fn settings_from_older_versions_fill_in_defaults() {
        let s: Settings = ron_like_roundtrip("(use_boot_rom:true)");
        assert!(s.use_boot_rom);
        assert_eq!((s.scale, s.fast_forward_speed), (4, 4));
    }

    fn ron_like_roundtrip(text: &str) -> Settings {
        // eframe stores settings as RON; parse the same way it does.
        ron::from_str(text).unwrap()
    }
}
