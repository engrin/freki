//! A loaded game: the emulated machine plus the files that belong to its ROM.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use freki_core::{BootRom, CartridgeError, FRAMES_PER_SECOND, GameBoy, Model, Rewind, Stop};

use crate::rom_file;
use crate::states::{StateSlots, write_atomic};

/// How often to write the save file while the game keeps changing it.
const SAVE_INTERVAL: Duration = Duration::from_secs(1);
/// A rewind snapshot is taken every this many frames…
pub const REWIND_INTERVAL: u32 = 2;
/// …and this much history is kept (stored as differences: a few MB in all).
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
pub fn power_on(rom: Vec<u8>, start: &Start) -> Result<GameBoy, CartridgeError> {
    let model = start.model.unwrap_or_else(|| Model::for_rom(&rom));
    let boot_rom = match model {
        Model::Dmg => &start.dmg_boot_rom,
        Model::Cgb => &start.cgb_boot_rom,
        // The SGB's own boot ROM is not supported: start in its post-boot state.
        Model::Sgb => &None,
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
    /// Write only this many bytes of the battery save: a clock game's `.srm` that holds
    /// RAM only (RetroArch keeps the clock elsewhere) stays that way.
    save_len: Option<usize>,
    pub slots: StateSlots,
    pub rewind: Rewind,
    frame: u32,
    last_save: Instant,
}

impl Session {
    /// Loads a ROM and its battery save, if any, starting it as `start` says. Returns the
    /// session and a status message.
    pub fn open(rom_path: &Path, start: Start) -> Result<(Self, String), Box<dyn Error>> {
        Self::open_saving(rom_path, start, battery_save_path(rom_path))
    }

    /// [`Session::open`] with the battery save in `save_path`.
    pub fn open_saving(
        rom_path: &Path,
        start: Start,
        save_path: PathBuf,
    ) -> Result<(Self, String), Box<dyn Error>> {
        let rom = rom_file::load(rom_path, None)?;
        let mut message = format!("loaded {}{}", file_name(rom_path), rom.describe());
        let mut gb = power_on(rom.data, &start)?;
        let mut save_len = None;
        if gb.save_data(0).is_some()
            && let Ok(data) = fs::read(&save_path)
        {
            if save_path.extension().is_some_and(|e| e == "srm")
                && gb.has_clock()
                && gb.save_time(&data).is_none()
            {
                save_len = Some(data.len());
            }
            gb.load_save_data(&data, Some(unix_now()));
            message.push_str(&format!(" with {}", file_name(&save_path)));
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
                save_len,
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

    /// Writes `rgba` (the screen, `width` × `height`) as `<rom>-NNN.png` next to the ROM,
    /// using the first free number. Returns the path.
    pub fn save_screenshot(
        &self,
        rgba: &[u8],
        [width, height]: [usize; 2],
    ) -> Result<PathBuf, Box<dyn Error>> {
        let stem = self.rom_path.with_extension("");
        let stem = stem.to_string_lossy();
        let path = (1..10_000)
            .map(|n| PathBuf::from(format!("{stem}-{n:03}.png")))
            .find(|p| !p.exists())
            .ok_or("too many screenshots")?;
        let file = fs::File::create(&path)?;
        let mut encoder =
            png::Encoder::new(std::io::BufWriter::new(file), width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(rgba)?;
        Ok(path)
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
        self.write_save(true)?;
        let rom = rom_file::load(&self.rom_path, None)?;
        let mut gb = power_on(rom.data, &self.start)?;
        if let Some(data) = self.gb.save_data(unix_now()) {
            gb.load_save_data(&data, None);
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
            self.write_save(false)?;
        }
        Ok(())
    }

    /// Writes the battery save if the cartridge has a battery and its RAM changed. With
    /// `closing`, a cartridge with a clock is saved even if RAM did not change, so the
    /// clock's time and timestamp are current for the next start.
    pub fn write_save(&mut self, closing: bool) -> Result<(), Box<dyn Error>> {
        let dirty = self.gb.take_save_dirty();
        if !dirty && !(closing && self.gb.has_clock()) {
            return Ok(());
        }
        let Some(data) = self.gb.save_data(unix_now()) else {
            return Ok(());
        };
        let len = self.save_len.map_or(data.len(), |n| n.min(data.len()));
        write_atomic(&self.save_path, &data[..len])
    }
}

/// The battery save of `rom_path`: `game.sav`, or RetroArch's `game.srm` if only that
/// exists (it has the same contents and is then saved to as well).
pub fn battery_save_path(rom_path: &Path) -> PathBuf {
    let sav = rom_path.with_extension("sav");
    let srm = rom_path.with_extension("srm");
    if !sav.exists() && srm.exists() {
        srm
    } else {
        sav
    }
}

/// The battery save of the second Game Boy on a link cable, playing `rom_path` while the
/// first one plays `first_rom`: the game's own save, or `game.2.sav` when both play the
/// same ROM file (so each keeps its own save, as two cartridges would).
pub fn linked_save_path(rom_path: &Path, first_rom: &Path) -> PathBuf {
    let same = match (fs::canonicalize(rom_path), fs::canonicalize(first_rom)) {
        (Ok(a), Ok(b)) => a == b,
        _ => rom_path == first_rom,
    };
    if same {
        rom_path.with_extension("2.sav")
    } else {
        battery_save_path(rom_path)
    }
}

/// Seconds since the Unix epoch, for the cartridge clock's timestamp in save files.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_a_retroarch_srm_when_there_is_no_sav() {
        let dir = std::env::temp_dir().join(format!("freki-srm-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let rom = dir.join("game.gb");
        assert_eq!(
            battery_save_path(&rom),
            dir.join("game.sav"),
            "neither exists"
        );
        fs::write(dir.join("game.srm"), [0]).unwrap();
        assert_eq!(battery_save_path(&rom), dir.join("game.srm"));
        fs::write(dir.join("game.sav"), [0]).unwrap();
        assert_eq!(battery_save_path(&rom), dir.join("game.sav"), "a .sav wins");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// An MBC3 game with a clock and 8 KiB of RAM: its RAM-only `.srm` is written back
    /// without freki's clock block.
    #[test]
    fn a_ram_only_srm_stays_ram_only() {
        let dir = std::env::temp_dir().join(format!("freki-srm-rtc-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x102].copy_from_slice(&[0x18, 0xFE]);
        rom[0x147] = 0x10;
        rom[0x149] = 0x02;
        let rom_path = dir.join("clock.gb");
        fs::write(&rom_path, rom).unwrap();
        let srm = dir.join("clock.srm");
        fs::write(&srm, vec![0x42; 8192]).unwrap();
        let (mut session, message) = Session::open(&rom_path, Start::default()).unwrap();
        assert!(message.ends_with("with clock.srm"), "{message}");
        session.write_save(true).unwrap();
        let written = fs::read(&srm).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(written, vec![0x42; 8192]);
    }

    #[test]
    fn the_second_copy_of_a_linked_game_keeps_its_own_save() {
        let (red, blue) = (Path::new("/g/red.gb"), Path::new("/g/blue.gb"));
        assert_eq!(linked_save_path(red, red), Path::new("/g/red.2.sav"));
        assert_eq!(linked_save_path(blue, red), Path::new("/g/blue.sav"));
    }

    #[test]
    fn screenshots_are_numbered_pngs_next_to_the_rom() {
        let dir = std::env::temp_dir().join(format!("freki-shots-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let rom_path = dir.join("game.gb");
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
        rom[0x150..0x152].copy_from_slice(&[0x18, 0xFE]);
        fs::write(&rom_path, rom).unwrap();
        let (session, _) = Session::open(&rom_path, Start::default()).unwrap();
        let rgba = vec![0x80; freki_core::SCREEN_WIDTH * freki_core::SCREEN_HEIGHT * 4];
        let size = [freki_core::SCREEN_WIDTH, freki_core::SCREEN_HEIGHT];
        let first = session.save_screenshot(&rgba, size).unwrap();
        let second = session.save_screenshot(&rgba, size).unwrap();
        assert_eq!(first, dir.join("game-001.png"));
        assert_eq!(second, dir.join("game-002.png"));
        let decoder = png::Decoder::new(std::io::BufReader::new(fs::File::open(&first).unwrap()));
        let info = decoder.read_info().unwrap();
        assert_eq!((info.info().width, info.info().height), (160, 144));
        fs::remove_dir_all(&dir).unwrap();
    }
}
