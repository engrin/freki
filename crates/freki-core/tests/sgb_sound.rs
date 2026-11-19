//! The Super Game Boy's sound with the real N-SPC engine, if the SGB's SNES ROM is around
//! (Nintendo's, so not part of the repository): `FREKI_SGB_ROM` or the usual places.

use std::path::PathBuf;

use freki_core::sgb_sound::SgbSound;

fn sgb_rom() -> Option<Vec<u8>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let candidates = [
        std::env::var_os("FREKI_SGB_ROM").map(PathBuf::from),
        Some(root.join("bios/sgb.sfc")),
        Some(root.join("roms/super-game-boy-sound/Super Game Boy (World) (Rev 2).sfc")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find_map(|p| std::fs::read(p).ok())
}

/// Peak level of the next `seconds` of sound.
fn peak(sound: &mut SgbSound, seconds: f32) -> f32 {
    let frames = (48_000.0 * seconds) as usize;
    let mut peak = 0.0f32;
    for _ in 0..frames / 800 {
        sound.tick(4_194_304 / 60);
        let mut out = vec![0.0; 1600];
        sound.mix_into(&mut out);
        peak = out.iter().fold(peak, |p, s| p.max(s.abs()));
    }
    peak
}

#[test]
fn sound_effects_play_on_the_n_spc_engine() {
    let Some(rom) = sgb_rom() else {
        eprintln!("no SGB SNES ROM: skipped");
        return;
    };
    let mut sound = SgbSound::new(&rom).expect("engine found");
    sound.set_rate(Some(48_000));
    let quiet = peak(&mut sound, 0.5);
    assert!(quiet < 0.01, "silent at start: {quiet}");
    assert!(!sound.smp().halted(), "the engine runs");
    // Effect A $0E (large explosion), pitch 1, volume high.
    sound.command(&[0x41, 0x0E, 0x00, 0x01, 0x00]);
    let loud = peak(&mut sound, 1.0);
    assert!(loud > 0.05, "the explosion is heard: {loud}");
    // Effect B $03 (applause, large group).
    sound.command(&[0x41, 0x00, 0x03, 0x20, 0x00]);
    let applause = peak(&mut sound, 1.0);
    assert!(applause > 0.05, "the applause is heard: {applause}");
}
