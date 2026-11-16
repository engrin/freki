//! Renders `dmg-acid2` and compares the screen pixel by pixel with its reference image.
//!
//! Fetch the ROM and reference with `scripts/fetch-test-roms.sh`; if they are missing,
//! the test prints a note and passes. On a mismatch, freki's screen is written to
//! `target/dmg-acid2-actual.png` for comparison.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use freki_core::{GameBoy, SCREEN_HEIGHT, SCREEN_WIDTH};

/// dmg-acid2 finishes drawing within a few frames, then loops forever.
const FRAMES: usize = 30;

fn test_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../roms/test")
}

/// Loads the reference PNG (2-bit greyscale, 0 = black) as freki shades (0 = lightest).
fn load_reference(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).expect("read reference PNG");
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().expect("PNG header");
    let mut buf = vec![0; reader.output_buffer_size().expect("buffer size")];
    let info = reader.next_frame(&mut buf).expect("PNG data");
    assert_eq!(
        (info.width, info.height),
        (SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32)
    );
    let channels = info.color_type.samples();
    buf[..info.buffer_size()]
        .chunks_exact(channels)
        .map(|px| 3 - px[0] / 85)
        .collect()
}

fn save_actual(shades: &[u8]) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/dmg-acid2-actual.png");
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

#[test]
fn dmg_acid2_matches_reference() {
    let dir = test_dir();
    let (Ok(rom), true) = (
        std::fs::read(dir.join("dmg-acid2.gb")),
        dir.join("dmg-acid2-reference.png").exists(),
    ) else {
        eprintln!("skipping dmg-acid2: not found (run scripts/fetch-test-roms.sh)");
        return;
    };

    let mut gb = GameBoy::new(rom).expect("ROM loads");
    for _ in 0..FRAMES {
        gb.run_frame().expect("runs");
    }
    let actual = gb.framebuffer().to_vec();
    let expected = load_reference(&dir.join("dmg-acid2-reference.png"));

    let wrong: Vec<(usize, usize)> = (0..actual.len())
        .filter(|&i| actual[i] != expected[i])
        .map(|i| (i % SCREEN_WIDTH, i / SCREEN_WIDTH))
        .collect();
    if !wrong.is_empty() {
        let path = save_actual(&actual);
        let mut rows: Vec<usize> = wrong.iter().map(|&(_, y)| y).collect();
        rows.dedup();
        panic!(
            "{} of {} pixels differ (rows {:?}). Screen saved to {}",
            wrong.len(),
            actual.len(),
            rows,
            path.display()
        );
    }
}
