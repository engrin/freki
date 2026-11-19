//! Save state slots on disk, next to the ROM: `<rom>.ss0` … `<rom>.ss9`, each with a
//! picture of the screen at the time as `<rom>.ssN.png`.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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

    pub fn select(&mut self, slot: u8) {
        self.slot = slot % SLOTS;
    }

    pub fn rom_path(&self) -> &Path {
        &self.rom_path
    }

    fn path(&self) -> PathBuf {
        self.path_of(self.slot)
    }

    fn path_of(&self, slot: u8) -> PathBuf {
        self.rom_path.with_extension(format!("ss{slot}"))
    }

    fn preview_path(&self, slot: u8) -> PathBuf {
        self.rom_path.with_extension(format!("ss{slot}.png"))
    }

    /// Saves the state in the current slot, with `picture` (RGBA and its size) as preview.
    pub fn save(
        &self,
        gb: &GameBoy,
        (rgba, size): (&[u8], [usize; 2]),
    ) -> Result<(), Box<dyn Error>> {
        write_atomic(&self.path(), &gb.save_state())?;
        write_atomic(&self.preview_path(self.slot), &encode_png(rgba, size)?)
    }

    /// When the state in `slot` was saved, or `None` for an empty slot.
    pub fn saved_at(&self, slot: u8) -> Option<SystemTime> {
        fs::metadata(self.path_of(slot)).ok()?.modified().ok()
    }

    /// The preview of `slot` as RGBA with its size, if there is one that belongs to the
    /// state (a state saved without a picture, e.g. headless, leaves an older one behind).
    pub fn preview(&self, slot: u8) -> Option<(Vec<u8>, [usize; 2])> {
        let saved = self.saved_at(slot)?;
        let path = self.preview_path(slot);
        if fs::metadata(&path).ok()?.modified().ok()? < saved {
            return None;
        }
        decode_png(&fs::read(path).ok()?)
    }

    /// "slot 3, saved 5 min ago" or "slot 3, empty".
    pub fn describe(&self, slot: u8) -> String {
        match self.saved_at(slot) {
            Some(time) => format!("slot {slot}, saved {}", age(time)),
            None => format!("slot {slot}, empty"),
        }
    }

    pub fn load(&self, gb: &mut GameBoy) -> Result<(), Box<dyn Error>> {
        let path = self.path();
        let data = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        gb.load_state(&data)?;
        Ok(())
    }
}

/// How long ago `time` was, in words: "just now", "5 min ago", "3 h ago", "2 days ago".
pub fn age(time: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(time)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    match secs {
        0..60 => "just now".to_string(),
        60..3_600 => format!("{} min ago", secs / 60),
        3_600..86_400 => format!("{} h ago", secs / 3_600),
        86_400..172_800 => "yesterday".to_string(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// `rgba` (`width` × `height`) as a PNG file.
pub fn encode_png(rgba: &[u8], [width, height]: [usize; 2]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(out)
}

/// An 8-bit RGBA PNG (as [`encode_png`] writes them) back to pixels and size.
fn decode_png(data: &[u8]) -> Option<(Vec<u8>, [usize; 2])> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(data))
        .read_info()
        .ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    rgba.truncate(info.buffer_size());
    Some((rgba, [info.width as usize, info.height as usize]))
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
        assert!(slots.saved_at(1).is_none());
        let picture = vec![7u8; 4 * 4 * 2];
        slots.save(&gb, (&picture, [4, 2])).unwrap();
        assert!(dir.join("game.ss1").exists());
        assert!(slots.saved_at(1).is_some());
        assert_eq!(slots.preview(1), Some((picture, [4, 2])));
        assert_eq!(slots.describe(1), "slot 1, saved just now");
        assert_eq!(slots.describe(2), "slot 2, empty");
        let saved = gb.save_state();
        gb.run_frame().unwrap();
        slots.load(&mut gb).unwrap();
        assert!(gb.save_state() == saved);

        slots.next();
        assert!(slots.load(&mut gb).is_err(), "empty slot 2 must fail");
        fs::remove_dir_all(&dir).unwrap();
    }
}
