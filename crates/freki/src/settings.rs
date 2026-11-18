//! Settings kept between runs (in eframe's storage: the platform's app data directory).

use std::fs;
use std::path::{Path, PathBuf};

use freki_core::{BootRom, Model};

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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            console: ConsoleModel::Auto,
            boot_rom_path: None,
            cgb_boot_rom_path: None,
            use_boot_rom: false,
            palette: Palette::default(),
            scale: 4,
            recent: Vec::new(),
            fast_forward_speed: 4,
            color_correction: true,
            bindings: Bindings::default(),
            rumble: true,
            integer_scaling: true,
        }
    }
}

impl Settings {
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
        let start = Start {
            model: self.console.model(),
            dmg_boot_rom: load(&self.boot_rom_path),
            cgb_boot_rom: load(&self.cgb_boot_rom_path),
        };
        (start, problems)
    }
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
}

impl ConsoleModel {
    pub const ALL: [ConsoleModel; 3] = [
        ConsoleModel::Auto,
        ConsoleModel::GameBoy,
        ConsoleModel::GameBoyColor,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ConsoleModel::Auto => "Auto (GBC for colour games)",
            ConsoleModel::GameBoy => "Game Boy",
            ConsoleModel::GameBoyColor => "Game Boy Color",
        }
    }

    /// The model to use, or `None` to choose per game.
    fn model(self) -> Option<Model> {
        match self {
            ConsoleModel::Auto => None,
            ConsoleModel::GameBoy => Some(Model::Dmg),
            ConsoleModel::GameBoyColor => Some(Model::Cgb),
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
