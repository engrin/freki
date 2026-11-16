//! A loaded game: the emulated machine plus the files that belong to its ROM.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use freki_core::{BootRom, CartridgeError, FRAMES_PER_SECOND, GameBoy, Model, Rewind, Stop};

use crate::states::{StateSlots, write_atomic};

/// How often to write the save file while the game keeps changing it.
const SAVE_INTERVAL: Duration = Duration::from_secs(1);
/// A rewind snapshot is taken every this many frames…
pub const REWIND_INTERVAL: u32 = 2;
/// …and this much history is kept (each snapshot is ~40 KB).
const REWIND_SECONDS: f64 = 20.0;

/// How games start: on which console, and with which boot ROMs.
#[derive(Debug, Clone, Default)]
pub struct Start {
    /// `None`: a Game Boy Color for colour games, a Game Boy for the rest.
    pub model: Option<Model>,
    pub dmg_boot_rom: Option<BootRom>,
    pub cgb_boot_rom: Option<BootRom>,
}

/// A machine with `rom` inserted: started by the console's boot ROM if there is one,
/// otherwise in the state the boot ROM leaves behind.
fn power_on(rom: Vec<u8>, start: &Start) -> Result<GameBoy, CartridgeError> {
    let colour_game = rom.get(0x143).is_some_and(|flag| flag & 0x80 != 0);
    let model = start
        .model
        .unwrap_or(if colour_game { Model::Cgb } else { Model::Dmg });
    let boot_rom = match model {
        Model::Dmg => &start.dmg_boot_rom,
        Model::Cgb => &start.cgb_boot_rom,
    };
    match boot_rom {
        Some(boot) => GameBoy::with_boot_rom(rom, boot.clone()),
        None => GameBoy::with_model(rom, model),
    }
}

pub struct Session {
    pub gb: GameBoy,
    /// Console and boot ROMs used on every (re)start.
    start: Start,
    pub rom_path: PathBuf,
    pub title: String,
    save_path: PathBuf,
    pub slots: StateSlots,
    pub rewind: Rewind,
    frame: u32,
    last_save: Instant,
}

impl Session {
    /// Loads a ROM and its battery save, if any, starting it as `start` says. Returns the
    /// session and a status message.
    pub fn open(rom_path: &Path, start: Start) -> Result<(Self, String), Box<dyn Error>> {
        let rom =
            fs::read(rom_path).map_err(|e| format!("cannot read {}: {e}", rom_path.display()))?;
        let mut gb = power_on(rom, &start)?;
        let save_path = rom_path.with_extension("sav");
        let mut message = format!("loaded {}", file_name(rom_path));
        if gb.save_data().is_some()
            && let Ok(data) = fs::read(&save_path)
        {
            gb.load_save_data(&data);
            message.push_str(" with save file");
        }
        let title = gb.cartridge().title();
        let capacity = (REWIND_SECONDS * FRAMES_PER_SECOND / REWIND_INTERVAL as f64) as usize;
        Ok((
            Self {
                gb,
                start,
                rom_path: rom_path.to_path_buf(),
                title,
                save_path,
                slots: StateSlots::new(rom_path),
                rewind: Rewind::new(capacity),
                frame: 0,
                last_save: Instant::now(),
            },
            message,
        ))
    }

    /// Starts the game as `start` says from the next reset on.
    pub fn set_start(&mut self, start: Start) {
        self.start = start;
    }

    pub fn cheat_file(&self) -> PathBuf {
        self.rom_path.with_extension("cht")
    }

    /// Runs one frame, keeping a rewind snapshot every few frames. Returns early with the
    /// reason if a breakpoint or watchpoint is hit.
    pub fn run_frame(&mut self) -> Result<Option<Stop>, Box<dyn Error>> {
        if self.frame.is_multiple_of(REWIND_INTERVAL) {
            self.rewind.push(self.gb.save_state());
        }
        self.frame = self.frame.wrapping_add(1);
        Ok(self.gb.run_frame_checked()?)
    }

    /// Steps back one rewind snapshot. Returns `false` when the history is exhausted.
    pub fn rewind_step(&mut self) -> Result<bool, Box<dyn Error>> {
        match self.rewind.pop() {
            Some(state) => {
                self.gb.load_state(&state)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Seconds of rewind history available.
    pub fn rewind_seconds(&self) -> f64 {
        self.rewind.len() as f64 * REWIND_INTERVAL as f64 / FRAMES_PER_SECOND
    }

    /// Resets the machine (like power cycling), keeping battery RAM and cheats.
    pub fn reset(&mut self) -> Result<(), Box<dyn Error>> {
        self.write_save()?;
        let rom = fs::read(&self.rom_path)?;
        let mut gb = power_on(rom, &self.start)?;
        if let Some(data) = self.gb.save_data() {
            gb.load_save_data(data);
        }
        *gb.cheats_mut() = self.gb.cheats().clone();
        self.gb = gb;
        self.rewind.clear();
        Ok(())
    }

    pub fn save_state(&self) -> Result<String, Box<dyn Error>> {
        self.slots.save(&self.gb)?;
        Ok(format!("saved state {}", self.slots.slot()))
    }

    pub fn load_state(&mut self) -> Result<String, Box<dyn Error>> {
        self.slots.load(&mut self.gb)?;
        self.rewind.clear();
        Ok(format!("loaded state {}", self.slots.slot()))
    }

    /// Writes the battery save if it changed and the autosave interval has passed.
    pub fn autosave(&mut self) -> Result<(), Box<dyn Error>> {
        if self.last_save.elapsed() >= SAVE_INTERVAL {
            self.last_save = Instant::now();
            self.write_save()?;
        }
        Ok(())
    }

    /// Writes the battery save if the cartridge has a battery and its RAM changed.
    pub fn write_save(&mut self) -> Result<(), Box<dyn Error>> {
        if !self.gb.take_save_dirty() {
            return Ok(());
        }
        let Some(data) = self.gb.save_data() else {
            return Ok(());
        };
        write_atomic(&self.save_path, data)
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}
