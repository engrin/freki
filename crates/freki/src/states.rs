//! Save state slots on disk, next to the ROM: `<rom>.ss0` … `<rom>.ss9`.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use freki_core::GameBoy;

pub const SLOTS: u8 = 10;

pub struct StateSlots {
    rom_path: PathBuf,
    slot: u8,
}

impl StateSlots {
    pub fn new(rom_path: &Path) -> Self {
        Self {
            rom_path: rom_path.to_path_buf(),
            slot: 0,
        }
    }

    pub fn slot(&self) -> u8 {
        self.slot
    }

    pub fn next(&mut self) {
        self.slot = (self.slot + 1) % SLOTS;
    }

    pub fn previous(&mut self) {
        self.slot = (self.slot + SLOTS - 1) % SLOTS;
    }

    fn path(&self) -> PathBuf {
        self.rom_path.with_extension(format!("ss{}", self.slot))
    }

    pub fn save(&self, gb: &GameBoy) -> Result<(), Box<dyn Error>> {
        write_atomic(&self.path(), &gb.save_state())
    }

    pub fn load(&self, gb: &mut GameBoy) -> Result<(), Box<dyn Error>> {
        let path = self.path();
        let data = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        gb.load_state(&data)?;
        Ok(())
    }
}

/// Writes to a temporary file and renames it, so a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<(), Box<dyn Error>> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, data)
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]); // JP $0150
        rom[0x150..0x152].copy_from_slice(&[0x18, 0xFE]); // JR -2
        rom
    }

    #[test]
    fn slots_wrap_and_round_trip_through_files() {
        let dir = std::env::temp_dir().join(format!("freki-slots-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let rom_path = dir.join("game.gb");
        let mut slots = StateSlots::new(&rom_path);
        slots.previous();
        assert_eq!(slots.slot(), 9);
        slots.next();
        slots.next();
        assert_eq!(slots.slot(), 1);

        let mut gb = GameBoy::new(rom()).unwrap();
        gb.run_frame().unwrap();
        slots.save(&gb).unwrap();
        assert!(dir.join("game.ss1").exists());
        let saved = gb.save_state();
        gb.run_frame().unwrap();
        slots.load(&mut gb).unwrap();
        assert!(gb.save_state() == saved);

        slots.next();
        assert!(slots.load(&mut gb).is_err(), "empty slot 2 must fail");
        fs::remove_dir_all(&dir).unwrap();
    }
}
