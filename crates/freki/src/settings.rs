//! Settings kept between runs (in eframe's storage: the platform's app data directory).

use std::fs;
use std::path::{Path, PathBuf};

use freki_core::BootRom;
use serde::{Deserialize, Serialize};

const KEY: &str = "freki-settings";

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    /// DMG boot ROM file chosen by the user.
    pub boot_rom_path: Option<PathBuf>,
    /// Whether games start with the boot ROM.
    pub use_boot_rom: bool,
}

impl Settings {
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
