//! freki desktop frontend: window, keyboard input and frame pacing.
//!
//! Usage: `freki <rom.gb>`

use std::error::Error;
use std::{env, fs, process};

use freki_core::{Button, FRAMES_PER_SECOND, GameBoy, SCREEN_HEIGHT, SCREEN_WIDTH};
use minifb::{Key, Scale, ScaleMode, Window, WindowOptions};

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
    window.set_target_fps(FRAMES_PER_SECOND.round() as usize);

    let mut pixels = vec![0u32; SCREEN_WIDTH * SCREEN_HEIGHT];
    while window.is_open() && !window.is_key_down(Key::Escape) {
        for (key, button) in KEYMAP {
            gb.set_button(button, window.is_key_down(key));
        }

        gb.run_frame()?;

        for (dst, &shade) in pixels.iter_mut().zip(gb.framebuffer()) {
            *dst = PALETTE[(shade & 3) as usize];
        }
        window.update_with_buffer(&pixels, SCREEN_WIDTH, SCREEN_HEIGHT)?;
    }
    Ok(())
}
