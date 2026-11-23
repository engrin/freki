//! Renders `dmg-acid2` and compares the screen pixel by pixel with its reference image.
//!
//! Fetch the ROM and reference with `scripts/fetch-test-roms.sh`; if they are missing,
//! the test prints a note and passes. On a mismatch, freki's screen is written to
//! `target/dmg-acid2-actual.png` for comparison.

mod common;

use freki_core::GameBoy;

/// dmg-acid2 finishes drawing within a few frames, then loops forever.
const FRAMES: usize = 30;

#[test]
fn dmg_acid2_matches_reference() {
    let dir = common::test_dir();
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
    let expected = common::load_reference(&dir.join("dmg-acid2-reference.png"));
    if let Err(msg) = common::compare("dmg-acid2", gb.framebuffer(), &expected) {
        panic!("{msg}");
    }
}
