//! freki desktop frontend: window, keyboard and gamepad input, sound, frame pacing, save
//! files, save states and rewind.
//!
//! Usage: `freki <rom.gb>`
//!
//! Battery-backed games are saved next to the ROM as `<rom>.sav`; save state slots as
//! `<rom>.ss0` … `<rom>.ss9`.

mod audio;
mod gamepad;
mod states;

use std::error::Error;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use std::{env, fs, process};

use freki_core::{Button, FRAMES_PER_SECOND, GameBoy, Rewind, SCREEN_HEIGHT, SCREEN_WIDTH};
use minifb::{Key, KeyRepeat, Scale, ScaleMode, Window, WindowOptions};

use audio::Audio;
use gamepad::Gamepads;
use states::{StateSlots, write_atomic};

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

const KEY_SAVE_STATE: Key = Key::F5;
const KEY_PREV_SLOT: Key = Key::F6;
const KEY_NEXT_SLOT: Key = Key::F7;
const KEY_LOAD_STATE: Key = Key::F8;
const KEY_REWIND: Key = Key::R;

/// How often to write the save file while the game keeps changing it.
const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Audio queued ahead of playback. Emulation waits while more than this is buffered,
/// which paces the game by the sound card's clock. Higher = fewer crackles, more lag.
const AUDIO_LATENCY: Duration = Duration::from_millis(60);
/// Longest a single frame waits for the audio queue to drain.
const MAX_AUDIO_WAIT: Duration = Duration::from_millis(100);

/// A rewind snapshot is taken every this many frames…
const REWIND_INTERVAL: u32 = 2;
/// …and this much history is kept (each snapshot is ~40 KB).
const REWIND_SECONDS: f64 = 20.0;

/// How long a status message stays in the window title.
const STATUS_DURATION: Duration = Duration::from_secs(2);

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

    let result = run_window(&mut gb, Path::new(&path), &save_path);
    // Save on the way out, even if emulation stopped with an error.
    let saved = write_save(&mut gb, &save_path);
    result.and(saved)
}

/// Window title with an optional, temporary status message.
struct Title {
    base: String,
    status: Option<(String, Instant)>,
    shown: String,
}

impl Title {
    fn new(game: &str) -> Self {
        let base = if game.is_empty() {
            "freki".to_string()
        } else {
            format!("freki — {game}")
        };
        Self {
            shown: base.clone(),
            base,
            status: None,
        }
    }

    /// Shows a status message and logs it to the terminal.
    fn set_status(&mut self, message: impl Into<String>) {
        let message = message.into();
        eprintln!("freki: {message}");
        self.show(message);
    }

    /// Shows a status message without logging it (for messages updated every frame).
    fn show(&mut self, message: String) {
        self.status = Some((message, Instant::now()));
    }

    /// Updates the window title if it changed.
    fn apply(&mut self, window: &mut Window) {
        if self
            .status
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > STATUS_DURATION)
        {
            self.status = None;
        }
        let wanted = match &self.status {
            Some((message, _)) => format!("{} — {message}", self.base),
            None => self.base.clone(),
        };
        if wanted != self.shown {
            window.set_title(&wanted);
            self.shown = wanted;
        }
    }
}

fn run_window(gb: &mut GameBoy, rom_path: &Path, save_path: &Path) -> Result<(), Box<dyn Error>> {
    let mut title = Title::new(&gb.cartridge().title());
    let mut window = Window::new(
        &title.base,
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
    let mut slots = StateSlots::new(rom_path);
    let rewind_capacity = (REWIND_SECONDS * FRAMES_PER_SECOND / REWIND_INTERVAL as f64) as usize;
    let mut rewind = Rewind::new(rewind_capacity);
    let frame_time = Duration::from_secs_f64(1.0 / FRAMES_PER_SECOND);

    let mut pixels = vec![0u32; SCREEN_WIDTH * SCREEN_HEIGHT];
    let mut last_save = Instant::now();
    let mut last_frame = Instant::now();
    let mut frame: u32 = 0;
    let mut was_rewinding = false;
    while window.is_open() && !window.is_key_down(Key::Escape) {
        // ---- hotkeys ----
        if window.is_key_pressed(KEY_SAVE_STATE, KeyRepeat::No) {
            match slots.save(gb) {
                Ok(()) => title.set_status(format!("saved state {}", slots.slot())),
                Err(e) => title.set_status(format!("save failed: {e}")),
            }
        }
        if window.is_key_pressed(KEY_LOAD_STATE, KeyRepeat::No) {
            match slots.load(gb) {
                Ok(()) => {
                    rewind.clear();
                    title.set_status(format!("loaded state {}", slots.slot()));
                }
                Err(e) => title.set_status(format!("load failed: {e}")),
            }
        }
        if window.is_key_pressed(KEY_NEXT_SLOT, KeyRepeat::Yes) {
            slots.next();
            title.set_status(format!("slot {}", slots.slot()));
        }
        if window.is_key_pressed(KEY_PREV_SLOT, KeyRepeat::Yes) {
            slots.previous();
            title.set_status(format!("slot {}", slots.slot()));
        }

        let rewinding = window.is_key_down(KEY_REWIND);
        if rewinding {
            // Step back one snapshot per displayed frame (2× speed); sound is muted.
            if let Some(state) = rewind.pop() {
                gb.load_state(&state)?;
            }
            title.show(format!(
                "rewinding ({:.1} s left)",
                rewind.len() as f64 * REWIND_INTERVAL as f64 / FRAMES_PER_SECOND
            ));
            gb.drain_audio(&mut samples);
            samples.clear();
            // Without audio to wait on, pace rewinding at the normal frame rate.
            let elapsed = last_frame.elapsed();
            if audio.is_some() && elapsed < frame_time {
                thread::sleep(frame_time - elapsed);
            }
        } else {
            if was_rewinding {
                title.set_status("resumed");
            }
            gamepads.update();
            for (key, button) in KEYMAP {
                gb.set_button(
                    button,
                    window.is_key_down(key) || gamepads.is_pressed(button),
                );
            }

            if frame.is_multiple_of(REWIND_INTERVAL) {
                rewind.push(gb.save_state());
            }
            frame = frame.wrapping_add(1);
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
        }
        was_rewinding = rewinding;
        last_frame = Instant::now();

        for (dst, &shade) in pixels.iter_mut().zip(gb.framebuffer()) {
            *dst = PALETTE[(shade & 3) as usize];
        }
        title.apply(&mut window);
        window.update_with_buffer(&pixels, SCREEN_WIDTH, SCREEN_HEIGHT)?;

        if last_save.elapsed() >= SAVE_INTERVAL {
            write_save(gb, save_path)?;
            last_save = Instant::now();
        }
    }
    Ok(())
}

/// Writes the save file if the cartridge has a battery and its RAM changed.
fn write_save(gb: &mut GameBoy, save_path: &Path) -> Result<(), Box<dyn Error>> {
    if !gb.take_save_dirty() {
        return Ok(());
    }
    let Some(data) = gb.save_data() else {
        return Ok(());
    };
    write_atomic(save_path, data)
}
