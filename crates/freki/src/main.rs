//! freki desktop frontend (egui): menu bar, screen, keyboard and gamepad input, sound,
//! save files, save states, rewind, cheats and a memory search.
//!
//! Usage: `freki [rom.gb|rom.zip] [--link rom2.gb]` — or open a ROM from the File menu or by
//! dropping it on the window; `--link` connects a second game by link cable. `freki
//! --headless rom.gb [options]` runs without a window (see [`headless`]).
//!
//! Files next to the ROM: `<rom>.sav` (battery save), `<rom>.ss0` … `<rom>.ss9` (save
//! states) and `<rom>.cht` (cheats). Settings such as the boot ROM are kept in the
//! platform's app data directory. The terminal freki was started from accepts cheat and
//! memory commands while the game runs (`help` lists them).

// Windows release builds are GUI programs (no console window of their own); see
// `attach_parent_console`.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod audio;
mod camera_picture;
mod console;
mod controls;
mod cpu_view;
mod debug;
mod gamepad;
mod headless;
mod input;
mod printer_view;
mod printout;
mod record;
mod rom_file;
mod session;
mod settings;
mod sound_view;
mod state_previews;
mod states;
mod webcam;
mod window;

use std::path::PathBuf;
use std::process;

use eframe::egui;
use freki_core::{SCREEN_HEIGHT, SCREEN_WIDTH};

const USAGE: &str = "usage: freki [ROM] [--link ROM2]   (--link: a second game on a link cable)
       freki --headless ROM [options]   (--headless --help for more)";

fn main() {
    #[cfg(all(windows, not(debug_assertions)))]
    attach_parent_console();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--headless") {
        process::exit(headless::main(&args[1..]));
    }
    if args.first().is_some_and(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return;
    }
    let mut paths = std::env::args_os().skip(1);
    let mut rom = None;
    let mut link = None;
    while let Some(arg) = paths.next() {
        if arg == "--link" {
            link = paths.next().map(PathBuf::from);
            if link.is_none() {
                eprintln!("freki: --link needs a ROM\n{USAGE}");
                process::exit(1);
            }
        } else if rom.is_none() {
            rom = Some(PathBuf::from(arg));
        } else {
            eprintln!(
                "freki: unexpected argument {}\n{USAGE}",
                arg.to_string_lossy()
            );
            process::exit(1);
        }
    }
    if link.is_some() && rom.is_none() {
        eprintln!("freki: --link needs a first ROM\n{USAGE}");
        process::exit(1);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("freki")
            .with_inner_size([SCREEN_WIDTH as f32 * 4.0, SCREEN_HEIGHT as f32 * 4.0 + 56.0])
            .with_min_inner_size([SCREEN_WIDTH as f32, SCREEN_HEIGHT as f32 + 56.0])
            .with_drag_and_drop(true)
            .with_icon(window_icon()),
        ..Default::default()
    };
    let result = eframe::run_native(
        "freki",
        options,
        Box::new(move |cc| Ok(Box::new(app::FrekiApp::new(cc, rom, link)))),
    );
    if let Err(err) = result {
        eprintln!("freki: {err}");
        process::exit(1);
    }
}

/// Windows release builds: started from a terminal, freki writes to it (headless mode,
/// `--help`, messages) and reads its commands from it; double-clicked, it has none.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    // SAFETY: a plain Win32 call without pointers; failing (no parent console) is fine.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// The wolf icon for the window, the taskbar and (when not started from freki.app) the Dock.
fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/freki-256.png"))
        .expect("assets/freki-256.png is a valid PNG")
}

#[cfg(test)]
mod tests {
    #[test]
    fn window_icon_decodes() {
        let icon = super::window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }
}
