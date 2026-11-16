//! Settings kept between runs (in eframe's storage: the platform's app data directory).

use std::fs;
use std::path::{Path, PathBuf};

use freki_core::BootRom;
use serde::{Deserialize, Serialize};

const KEY: &str = "freki-settings";
/// Entries kept in File → Recent.
const MAX_RECENT: usize = 10;
/// Fast-forward speeds offered in the Emulation menu.
pub const FAST_FORWARD_SPEEDS: [u32; 4] = [2, 3, 4, 8];

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// DMG boot ROM file chosen by the user.
    pub boot_rom_path: Option<PathBuf>,
    /// Whether games start with the boot ROM.
    pub use_boot_rom: bool,
    pub palette: Palette,
    /// Window scale (the window size itself is restored by eframe).
    pub scale: u32,
    /// Recently opened ROMs, most recent first.
    pub recent: Vec<PathBuf>,
    /// Emulation speed multiplier while fast-forwarding.
    pub fast_forward_speed: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            boot_rom_path: None,
            use_boot_rom: false,
            palette: Palette::default(),
            scale: 4,
            recent: Vec::new(),
            fast_forward_speed: 4,
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

    /// The boot ROM to start games with, if enabled. Errors describe why the chosen file
    /// cannot be used.
    pub fn boot_rom(&self) -> Result<Option<BootRom>, String> {
        match (&self.boot_rom_path, self.use_boot_rom) {
            (Some(path), true) => read_boot_rom(path).map(Some),
            _ => Ok(None),
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

#[cfg(test)]
mod tests {
    use super::*;

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
