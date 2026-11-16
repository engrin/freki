//! freki desktop frontend: window, keyboard and gamepad input, sound, frame pacing and
//! save files.
//!
//! Usage: `freki <rom.gb>`
//!
//! Battery-backed games are saved next to the ROM as `<rom>.sav`.

mod audio;
mod gamepad;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use std::{env, fs, process};

use freki_core::{Button, FRAMES_PER_SECOND, GameBoy, SCREEN_HEIGHT, SCREEN_WIDTH};
use minifb::{Key, Scale, ScaleMode, Window, WindowOptions};

use audio::Audio;
use gamepad::Gamepads;

/// Classic DMG green palette as 0RGB, indexed by shade (0 = lightest).
const PALETTE: [u32; 4] = [0x00E0_F8D0, 0x0088_C070, 0x0034_6856, 0x0008_1820];

const KEYMAP: [(Key, Button); 8] = [
    (Key::Right, Button::Right),
    (Key::Left, Button::Left),
    (Key::Up, Button::Up),
    (Key::Down, Button::Down),
    (Key::X, Button::A),
    (Key::Z, Button::B),
    (Key::Backspace, Button::Select),
    (Key::Enter, Button::Start),
];

/// How often to write the save file while the game keeps changing it.
const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Audio queued ahead of playback. Emulation waits while more than this is buffered,
/// which paces the game by the sound card's clock. Higher = fewer crackles, more lag.
const AUDIO_LATENCY: Duration = Duration::from_millis(60);
/// Longest a single frame waits for the audio queue to drain.
const MAX_AUDIO_WAIT: Duration = Duration::from_millis(100);

fn main() {
    if let Err(err) = run() {
        eprintln!("freki: {err}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let path = env::args().nth(1).ok_or("usage: freki <rom.gb>")?;
    let rom = fs::read(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let mut gb = GameBoy::new(rom)?;

    let save_path = Path::new(&path).with_extension("sav");
    if gb.save_data().is_some()
        && let Ok(data) = fs::read(&save_path)
    {
        gb.load_save_data(&data);
        eprintln!("freki: loaded {}", save_path.display());
    }

    let result = run_window(&mut gb, &save_path);
    // Save on the way out, even if emulation stopped with an error.
    let saved = write_save(&mut gb, &save_path);
    result.and(saved)
}

fn run_window(gb: &mut GameBoy, save_path: &Path) -> Result<(), Box<dyn Error>> {
    let title = match gb.cartridge().title() {
        t if t.is_empty() => "freki".to_string(),
        t => format!("freki — {t}"),
    };
    let mut window = Window::new(
        &title,
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
        WindowOptions {
            resize: true,
            scale: Scale::X4,
            scale_mode: ScaleMode::AspectRatioStretch,
            ..WindowOptions::default()
        },
    )?;
    let audio = match Audio::open() {
        Ok(audio) => {
            gb.set_audio_sample_rate(audio.sample_rate());
            // The audio queue paces emulation; don't let the window throttle as well.
            window.set_target_fps(0);
            Some(audio)
        }
        Err(err) => {
            eprintln!("freki: no sound ({err})");
            window.set_target_fps(FRAMES_PER_SECOND.round() as usize);
            None
        }
    };
    let max_queued = audio.as_ref().map_or(0, |a| {
        (a.sample_rate() as f64 * AUDIO_LATENCY.as_secs_f64()) as usize
    });
    let mut samples = Vec::new();

    let mut gamepads = Gamepads::open();
    let mut pixels = vec![0u32; SCREEN_WIDTH * SCREEN_HEIGHT];
    let mut last_save = Instant::now();
    while window.is_open() && !window.is_key_down(Key::Escape) {
        gamepads.update();
        for (key, button) in KEYMAP {
            gb.set_button(
                button,
                window.is_key_down(key) || gamepads.is_pressed(button),
            );
        }

        gb.run_frame()?;

        if let Some(audio) = &audio {
            gb.drain_audio(&mut samples);
            audio.push(&samples);
            samples.clear();
            // Wait for playback to catch up, but never hang if the device stops pulling.
            let waited = Instant::now();
            while audio.queued_frames() > max_queued && waited.elapsed() < MAX_AUDIO_WAIT {
                thread::sleep(Duration::from_millis(1));
            }
        }

        for (dst, &shade) in pixels.iter_mut().zip(gb.framebuffer()) {
            *dst = PALETTE[(shade & 3) as usize];
        }
        window.update_with_buffer(&pixels, SCREEN_WIDTH, SCREEN_HEIGHT)?;

        if last_save.elapsed() >= SAVE_INTERVAL {
            write_save(gb, save_path)?;
            last_save = Instant::now();
        }
    }
    Ok(())
}

/// Writes the save file if the cartridge has a battery and its RAM changed.
/// Writes to a temporary file first so a crash never leaves a half-written save.
fn write_save(gb: &mut GameBoy, save_path: &Path) -> Result<(), Box<dyn Error>> {
    if !gb.take_save_dirty() {
        return Ok(());
    }
    let Some(data) = gb.save_data() else {
        return Ok(());
    };
    let tmp: PathBuf = save_path.with_extension("sav.tmp");
    fs::write(&tmp, data)
        .and_then(|()| fs::rename(&tmp, save_path))
        .map_err(|e| format!("cannot write {}: {e}", save_path.display()))?;
    Ok(())
}
