//! Helpers shared by the screenshot-comparison tests.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use freki_core::{SCREEN_HEIGHT, SCREEN_WIDTH};

/// Where `scripts/fetch-test-roms.sh` puts the test ROMs.
pub fn test_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../roms/test")
}

/// Loads a reference screenshot as freki shades (0 = lightest … 3 = darkest).
/// Accepts any greyscale or palette PNG whose levels are white, light, dark, black.
pub fn load_reference(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).expect("read reference PNG");
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().expect("PNG header");
    let mut buf = vec![0; reader.output_buffer_size().expect("buffer size")];
    let info = reader.next_frame(&mut buf).expect("PNG data");
    assert_eq!(
        (info.width, info.height),
        (SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32),
        "{}",
        path.display()
    );
    let channels = info.color_type.samples();
    buf[..info.buffer_size()]
        .chunks_exact(channels)
        .map(|px| 3 - ((px[0] as u16 + 42) / 85) as u8)
        .collect()
}

/// Writes freki's screen as a greyscale PNG to `target/<name>-actual.png`.
pub fn save_actual(name: &str, shades: &[u8]) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("{name}-actual.png"));
    let file = std::fs::File::create(&path).expect("create PNG");
    let mut encoder = png::Encoder::new(file, SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let grey: Vec<u8> = shades.iter().map(|&s| 255 - s * 85).collect();
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(&grey))
        .expect("write PNG");
    path
}

/// Compares freki's screen with a reference; on mismatch saves the screen and explains.
pub fn compare(name: &str, actual: &[u8], expected: &[u8]) -> Result<(), String> {
    let wrong: Vec<usize> = (0..actual.len())
        .filter(|&i| actual[i] != expected[i])
        .collect();
    if wrong.is_empty() {
        return Ok(());
    }
    let path = save_actual(name, actual);
    let mut rows: Vec<usize> = wrong.iter().map(|&i| i / SCREEN_WIDTH).collect();
    rows.dedup();
    Err(format!(
        "{name}: {} of {} pixels differ (rows {:?}). Screen saved to {}",
        wrong.len(),
        actual.len(),
        rows,
        path.display()
    ))
}
