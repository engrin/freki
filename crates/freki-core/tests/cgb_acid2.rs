//! Renders `cgb-acid2` and compares the screen pixel by pixel with its reference image
//! (colours converted from RGB555 with `(c << 3) | (c >> 2)`, as its README specifies).
//!
//! Fetch the ROM and reference with `scripts/fetch-test-roms.sh`; if they are missing,
//! the test prints a note and passes. On a mismatch, freki's screen is written to
//! `target/cgb-acid2-actual.png` for comparison.

mod common;

use freki_core::{GameBoy, Model, Screen, rgb555_to_rgb888};

/// cgb-acid2 finishes drawing within a few frames, then loops forever.
const FRAMES: usize = 30;

#[test]
fn cgb_acid2_matches_reference() {
    let dir = common::test_dir();
    let (Ok(rom), true) = (
        std::fs::read(dir.join("cgb-acid2.gbc")),
        dir.join("cgb-acid2-reference.png").exists(),
    ) else {
        eprintln!("skipping cgb-acid2: not found (run scripts/fetch-test-roms.sh)");
        return;
    };

    let mut gb = GameBoy::new(rom).expect("ROM loads");
    assert_eq!(gb.model(), Model::Cgb);
    for _ in 0..FRAMES {
        gb.run_frame().expect("runs");
    }
    let Screen::Rgb555(screen) = gb.screen() else {
        panic!("expected a colour screen");
    };
    let actual: Vec<[u8; 3]> = screen.iter().map(|&c| rgb555_to_rgb888(c)).collect();
    let expected = common::load_rgb_reference(&dir.join("cgb-acid2-reference.png"));
    if let Err(msg) = common::compare_rgb("cgb-acid2", &actual, &expected) {
        panic!("{msg}");
    }
}
