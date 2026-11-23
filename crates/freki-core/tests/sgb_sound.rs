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

/// Donkey Kong (1994) on a Super Game Boy, once with and once without the SGB's own sound:
/// the difference between the two is what the SNES sound chip adds. DK uploads its own
/// music score and instrument (`SOU_TRN`) and starts the score with `SOUND` in the game.
#[test]
fn donkey_kong_uses_the_sgb_sound() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (Some(sgb), Ok(dk)) = (
        sgb_rom(),
        std::fs::read(root.join("roms/Donkey Kong (World) (Rev 1) (SGB Enhanced).gb")),
    ) else {
        eprintln!("no SGB ROM or Donkey Kong: skipped");
        return;
    };
    let start = |with_sound: bool| {
        let mut gb = freki_core::GameBoy::with_model(dk.clone(), freki_core::Model::Sgb).unwrap();
        gb.set_audio_sample_rate(48_000);
        if with_sound {
            gb.set_sgb_sound_rom(&sgb).unwrap();
        }
        gb
    };
    let (mut plain, mut with) = (start(false), start(true));
    let mut extra = 0.0f32;
    let mut frame = 0;
    // Both play the same, frame by frame, until the SNES adds something (or 5000 frames).
    while frame < 5000 && extra <= 0.02 {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for (gb, audio) in [(&mut plain, &mut a), (&mut with, &mut b)] {
            // Start through the title and menus, then A now and then (jumps) and right.
            gb.set_button(freki_core::Button::Start, frame % 300 <= 5 && frame < 1500);
            gb.set_button(freki_core::Button::A, frame > 1500 && frame % 90 < 8);
            gb.set_button(freki_core::Button::Right, frame > 1500 && frame % 400 < 200);
            gb.run_frame().unwrap();
            gb.drain_audio(audio);
        }
        // With its own sound, the SGB lowers the Game Boy's to make room.
        extra = a
            .iter()
            .zip(&b)
            .map(|(x, y)| (x * freki_core::sgb_sound::GB_LEVEL - y).abs())
            .fold(extra, f32::max);
        frame += 1;
    }
    eprintln!("frame {frame}: largest SNES contribution {extra}");
    assert!(extra > 0.02, "Donkey Kong's SGB sounds are heard: {extra}");
}
