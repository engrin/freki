//! freki desktop frontend (egui): menu bar, screen, keyboard and gamepad input, sound,
//! save files, save states, rewind, cheats and a memory search.
//!
//! Usage: `freki [rom.gb]` — or open a ROM from the File menu or by dropping it on the
//! window.
//!
//! Files next to the ROM: `<rom>.sav` (battery save), `<rom>.ss0` … `<rom>.ss9` (save
//! states) and `<rom>.cht` (cheats). Settings such as the boot ROM are kept in the
//! platform's app data directory. The terminal freki was started from accepts cheat and
//! memory commands while the game runs (`help` lists them).

mod app;
mod audio;
mod console;
mod cpu_view;
mod debug;
mod gamepad;
mod session;
mod settings;
mod states;

use std::path::PathBuf;
use std::process;

use eframe::egui;
use freki_core::{SCREEN_HEIGHT, SCREEN_WIDTH};

fn main() {
    let rom = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("freki")
            .with_inner_size([SCREEN_WIDTH as f32 * 4.0, SCREEN_HEIGHT as f32 * 4.0 + 56.0])
            .with_min_inner_size([SCREEN_WIDTH as f32, SCREEN_HEIGHT as f32 + 56.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let result = eframe::run_native(
        "freki",
        options,
        Box::new(move |cc| Ok(Box::new(app::FrekiApp::new(cc, rom)))),
    );
    if let Err(err) = result {
        eprintln!("freki: {err}");
        process::exit(1);
    }
}
