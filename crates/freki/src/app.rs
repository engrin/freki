//! The egui application: menu bar, screen, status bar and tool windows.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, ColorImage, Key, KeyboardShortcut, Modifiers, Pos2, Rect, Sense, Stroke,
    StrokeKind, TextureHandle, TextureOptions, pos2, vec2,
};
use freki_core::{
    Button, Code, FRAMES_PER_SECOND, Filter, Link, MemorySearch, Model, SCREEN_HEIGHT, SCREEN_WIDTH,
};

use crate::audio::Audio;
use crate::camera_picture;
use crate::console::{self, Console};
use crate::controls;
use crate::cpu_view::{self, CpuView};
use crate::debug::{self, DebugViews};
use crate::gamepad::Gamepads;
use crate::input;
use crate::printer_view::PrinterView;
use crate::printout;
use crate::record::{self, Recorder};
use crate::rom_file;
use crate::session::Start;
use crate::session::{self, Session, file_name};
use crate::settings::{self, ConsoleModel, FAST_FORWARD_SPEEDS, GameSettings, Palette, Settings};
use crate::sound_view::SoundView;
use crate::state_previews::{Pick, StatePreviews};
use crate::webcam::{self, Webcam};
use crate::window::{self, take_forwarded_hotkeys, tool_window};

const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
/// How long no freki window must have focus before the game pauses in the background.
const FOCUS_GRACE: Duration = Duration::from_millis(200);
const QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);
const RESET: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::R);
/// How much of the way to the D-pad's or stick's tilt MBC7 games get each frame.
const TILT_EASE: f32 = 0.2;
const FULLSCREEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::ALT, Key::Enter);

/// Audio kept queued ahead of playback. Video runs at the Game Boy's own frame rate;
/// audio stays in sync by nudging the output sample rate towards this buffer level.
const AUDIO_LATENCY: Duration = Duration::from_millis(60);
/// Largest audio rate correction (0.5 %: inaudible).
const MAX_RATE_ADJUST: f64 = 0.005;
/// Never emulate more than this many frames in one UI update (e.g. after a stall).
const MAX_FRAMES_PER_UPDATE: u32 = 4;
/// How long a status message stays in the status bar.
const STATUS_DURATION: Duration = Duration::from_secs(3);
/// Tooltip of the gamepad switches.
const GAMEPADS_HINT: &str = if cfg!(target_os = "macos") {
    "Read game controllers. macOS asks once for Input Monitoring permission (it covers \
     every HID device); freki only reads the controllers' buttons and sticks"
} else {
    "Read game controllers"
};

/// What the controls window is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capture {
    Key,
    Pad,
}

/// Which bindings the Controls window shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ControlsTab {
    Keyboard,
    Gamepad,
}

/// A key's name as shown on its keycap: arrows get an arrow.
fn key_label(key: Key) -> String {
    match key {
        Key::ArrowUp => "↑ Up arrow".into(),
        Key::ArrowDown => "↓ Down arrow".into(),
        Key::ArrowLeft => "← Left arrow".into(),
        Key::ArrowRight => "→ Right arrow".into(),
        other => other.name().into(),
    }
}

/// Frame pacing statistics, shown in the status bar by View → Show frame timing.
struct Timing {
    since: Instant,
    frames: u32,
    updates: u32,
    catch_ups: u32,
    /// Results of the last full second.
    fps: f64,
    refresh: f64,
    catch_ups_per_s: u32,
    audio_ms: f64,
    rate_adjust: f64,
}

impl Timing {
    fn new() -> Self {
        Self {
            since: Instant::now(),
            frames: 0,
            updates: 0,
            catch_ups: 0,
            fps: 0.0,
            refresh: 0.0,
            catch_ups_per_s: 0,
            audio_ms: 0.0,
            rate_adjust: 0.0,
        }
    }

    /// Records one UI update that emulated `frames` frames.
    fn record(&mut self, frames: u32) {
        self.updates += 1;
        self.frames += frames;
        if frames > 1 {
            self.catch_ups += 1;
        }
        let elapsed = self.since.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            self.fps = self.frames as f64 / elapsed;
            self.refresh = self.updates as f64 / elapsed;
            self.catch_ups_per_s = self.catch_ups;
            *self = Self {
                since: Instant::now(),
                frames: 0,
                updates: 0,
                catch_ups: 0,
                ..*self
            };
        }
    }

    fn summary(&self) -> String {
        format!(
            "{:.1} fps · display {:.0} Hz · {} catch-up(s)/s · audio {:.0} ms ({:+.2} %)",
            self.fps,
            self.refresh,
            self.catch_ups_per_s,
            self.audio_ms,
            self.rate_adjust * 100.0
        )
    }
}

/// Input state of the memory search window.
#[derive(Default)]
struct SearchUi {
    search: Option<MemorySearch>,
    /// Values as the game shows them, stored as BCD (each hex digit one decimal digit:
    /// 99 lives = $99), as many games keep lives, coins and timers.
    bcd: bool,
    value: String,
    cheat_value: String,
    cheat_name: String,
    selected: Option<usize>,
}

/// The second Game Boy, connected to the first by a link cable.
struct LinkedGame {
    session: Session,
    cable: Link,
}

pub struct FrekiApp {
    session: Option<Session>,
    /// A second game on a link cable, shown to the right of the first.
    link: Option<LinkedGame>,
    /// While linked: the keyboard plays the second Game Boy (F4 switches).
    keyboard_second: bool,
    settings: Settings,
    audio: Option<Audio>,
    gamepads: Gamepads,
    console: Console,
    texture: Option<TextureHandle>,
    /// Picture size the window was last fitted to (Super Game Boy pictures are larger).
    fitted_size: [usize; 2],
    /// The loaded game's key for its own settings (see `settings::game_key`).
    game_key: Option<String>,
    /// While the loaded game uses its own settings: your general settings, put back when
    /// it closes.
    general_settings: Option<GameSettings>,
    /// The video being recorded (File → Record video, Shift+F12).
    recorder: Option<Recorder>,
    /// Recordings whose encoder is still finishing the file.
    finishing: Vec<std::thread::JoinHandle<Result<record::Summary, String>>>,
    /// With frame blending: the picture of the emulated frame before the current one.
    previous_frame: Option<(Vec<u8>, [usize; 2])>,
    samples: Vec<f32>,

    paused: bool,
    rewinding: bool,
    /// Fast-forward toggled on in the menu (holding Tab also fast-forwards).
    fast_forward: bool,
    fast_forward_held: bool,
    status: Option<(String, Instant)>,
    /// Frames of emulation owed to real time.
    frame_debt: f64,
    last_tick: Instant,
    show_timing: bool,
    timing: Timing,
    debug: DebugViews,
    cpu_view: CpuView,

    show_cheats: bool,
    show_search: bool,
    show_controls: bool,
    /// The Game Boy button whose key or gamepad button is being chosen.
    capture: Option<(input::Control, Capture)>,
    /// Frames each turbo button (A, B) has been held, per Game Boy (left, right).
    turbo: [[u32; 2]; 2],
    /// Paused because no freki window has focus (see [`Settings::pause_in_background`]).
    background_paused: bool,
    /// When the last freki window lost focus.
    unfocused_since: Option<Instant>,
    controls_tab: ControlsTab,
    /// Tilt passed to MBC7 cartridges, eased towards the input (see `emulate`).
    tilt: (f32, f32),
    controls_message: Option<String>,
    show_about: bool,
    new_code: String,
    /// Values typed into the cheat manager's template rows.
    template_values: Vec<String>,
    new_name: String,
    cheat_error: Option<String>,
    /// The cheat being edited in the cheat manager: index, code and name as typed.
    cheat_edit: Option<(usize, String, String)>,
    search_ui: SearchUi,
    /// A Game Boy Printer is plugged into the link cable port (Link cable menu).
    printer: bool,
    printer_view: PrinterView,
    sound_view: SoundView,
    /// The camera picture, decoded (`settings.camera_picture`), for camera cartridges.
    camera_view: Option<Vec<u8>>,
    /// Whether the remembered camera picture was loaded (or tried) since the start.
    camera_loaded: bool,
    /// The webcam the camera sees (`settings.camera_webcam`), while a camera cartridge is in.
    webcam: Option<Webcam>,
    /// The webcam stopped with an error; not started again until chosen again.
    webcam_failed: bool,
    /// The webcams found (Camera: webcam menu), or why none could be looked for.
    webcams: Option<Result<Vec<String>, String>>,
    /// The search for webcams, running in the background.
    webcam_search: Option<Receiver<Result<Vec<String>, String>>>,
    state_previews: StatePreviews,
}

impl FrekiApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        rom: Option<PathBuf>,
        link: Option<PathBuf>,
    ) -> Self {
        let audio = match Audio::open() {
            Ok(audio) => Some(audio),
            Err(err) => {
                eprintln!("freki: no sound ({err})");
                None
            }
        };
        let mut app = Self {
            session: None,
            link: None,
            keyboard_second: false,
            settings: Settings::load(cc.storage),
            audio,
            // Started below, once the settings say whether gamepads are on.
            gamepads: Gamepads::new(false),
            console: Console::start(),
            texture: None,
            fitted_size: [SCREEN_WIDTH, SCREEN_HEIGHT],
            previous_frame: None,
            recorder: None,
            finishing: Vec::new(),
            game_key: None,
            general_settings: None,
            samples: Vec::new(),
            paused: false,
            rewinding: false,
            fast_forward: false,
            fast_forward_held: false,
            status: None,
            frame_debt: 0.0,
            last_tick: Instant::now(),
            show_timing: false,
            timing: Timing::new(),
            debug: DebugViews::new(),
            cpu_view: CpuView::new(),
            show_cheats: false,
            show_search: false,
            show_controls: false,
            capture: None,
            turbo: [[0; 2]; 2],
            background_paused: false,
            unfocused_since: None,
            controls_tab: ControlsTab::Keyboard,
            tilt: (0.0, 0.0),
            controls_message: None,
            show_about: false,
            new_code: String::new(),
            template_values: Vec::new(),
            new_name: String::new(),
            cheat_error: None,
            cheat_edit: None,
            search_ui: SearchUi::default(),
            printer: false,
            printer_view: PrinterView::default(),
            sound_view: SoundView::default(),
            camera_view: None,
            camera_loaded: false,
            webcam: None,
            webcam_failed: false,
            webcams: None,
            webcam_search: None,
            state_previews: StatePreviews::default(),
        };
        app.gamepads.set_on(app.settings.gamepads);
        println!("freki: type `help` here for cheat and memory commands");
        if let Some(path) = rom {
            app.open_rom(&cc.egui_ctx, &path);
        }
        if let Some(path) = link {
            app.connect_link(&cc.egui_ctx, &path);
        }
        app
    }

    fn set_status(&mut self, message: impl Into<String>) {
        let message = message.into();
        eprintln!("freki: {message}");
        self.status = Some((message, Instant::now()));
    }

    /// How games start (console, boot ROMs); problems with boot ROM files are reported.
    fn start(&mut self) -> Start {
        let (start, problems) = self.settings.start();
        for problem in problems {
            self.set_status(problem);
        }
        start
    }

    /// Lets the user pick a boot ROM; its size tells whether it is the GB or GBC one.
    fn pick_boot_rom(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choose Game Boy or Game Boy Color boot ROM")
            .add_filter("Boot ROM", &["bin", "gb", "gbc", "rom"])
            .pick_file()
        else {
            return;
        };
        match settings::read_boot_rom(&path) {
            Ok(boot) => {
                let name = file_name(&path);
                if boot.is_cgb() {
                    self.settings.cgb_boot_rom_path = Some(path);
                } else {
                    self.settings.boot_rom_path = Some(path);
                }
                self.settings.use_boot_rom = true;
                let kind = if boot.is_cgb() {
                    "Game Boy Color"
                } else {
                    "Game Boy"
                };
                self.start_changed(&format!("{kind} boot ROM: {name}"));
            }
            Err(e) => self.set_status(e),
        }
    }

    /// Lets the user pick the Super Game Boy's SNES ROM, for the SGB's own sound.
    fn pick_sgb_rom(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choose the Super Game Boy's SNES ROM")
            .add_filter("SNES ROM", &["sfc", "smc", "bin"])
            .pick_file()
        else {
            return;
        };
        match settings::read_sgb_rom(&path) {
            Ok(_) => {
                let name = file_name(&path);
                self.settings.sgb_rom_path = Some(path);
                self.start_changed(&format!("Super Game Boy sound: {name}"));
            }
            Err(e) => self.set_status(e),
        }
    }

    /// Applies the console and boot ROM settings to the running game from its next reset.
    fn start_changed(&mut self, what: &str) {
        let start = self.start();
        if let Some(session) = &mut self.session {
            session.set_start(start);
            self.set_status(format!("{what} (from the next reset, Ctrl+R, or ROM load)"));
        } else {
            self.set_status(what);
        }
    }

    fn open_rom(&mut self, ctx: &egui::Context, path: &Path) {
        self.close_rom();
        // A game with its own settings starts with them (its console included).
        self.game_key = rom_file::load(path, None)
            .ok()
            .and_then(|rom| settings::game_key(&rom.data));
        let own = self
            .game_key
            .as_ref()
            .and_then(|key| self.settings.games.get(key))
            .copied();
        if let Some(own) = own {
            self.general_settings = Some(GameSettings::of(&self.settings));
            own.apply(&mut self.settings);
        }
        let start = self.start();
        // Fit the window to this game's picture (a Super Game Boy game's is larger).
        self.fitted_size = [0, 0];
        match Session::open(path, start) {
            Ok((mut session, message)) => {
                if let Some(audio) = &self.audio {
                    session.gb.set_audio_sample_rate(audio.sample_rate());
                }
                let cheats = console::load_cheat_file(&session.cheat_file(), &mut session.gb);
                self.console.set_cheat_file(session.cheat_file());
                self.session = Some(session);
                self.update_title(ctx);
                self.settings.add_recent(path);
                self.search_ui = SearchUi::default();
                self.paused = false;
                self.set_status(message);
                if let Some(own) = own {
                    self.set_status(format!(
                        "using this game's own settings: {}, {} palette",
                        own.console.name(),
                        own.palette.name()
                    ));
                }
                if let Some(msg) = cheats {
                    self.set_status(msg);
                }
            }
            Err(e) => {
                self.restore_general_settings();
                self.game_key = None;
                if !path.exists() {
                    self.settings.remove_recent(path);
                }
                self.set_status(format!("cannot open {}: {e}", file_name(path)));
            }
        }
    }

    /// The window title: the game's title and console (both games while linked).
    fn update_title(&self, ctx: &egui::Context) {
        let describe = |session: &Session| {
            let model = match session.gb.model() {
                Model::Cgb => " (Game Boy Color)",
                Model::Sgb => " (Super Game Boy)",
                Model::Dmg => "",
            };
            format!("{}{model}", session.title)
        };
        let title = match (&self.session, &self.link) {
            (Some(one), Some(two)) => {
                format!("freki — {} ⇄ {}", describe(one), describe(&two.session))
            }
            (Some(one), None) if !one.title.is_empty() => format!("freki — {}", describe(one)),
            (Some(one), None) => format!("freki{}", describe(one)),
            (None, _) => "freki".to_string(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    /// Connects a second game by link cable, shown to the right of the first.
    fn connect_link(&mut self, ctx: &egui::Context, path: &Path) {
        self.disconnect_link();
        let Some(first) = self.session.as_ref() else {
            self.set_status("open a game first");
            return;
        };
        let save = session::linked_save_path(path, &first.rom_path);
        let start = self.start();
        match Session::open_saving(path, start, save) {
            Ok((mut session, message)) => {
                if let Some(audio) = &self.audio {
                    session.gb.set_audio_sample_rate(audio.sample_rate());
                }
                self.link = Some(LinkedGame {
                    session,
                    cable: Link::new(),
                });
                if self.printer {
                    self.printer = false;
                    if let Some(first) = self.session.as_mut() {
                        first.gb.set_printer(false);
                    }
                }
                self.keyboard_second = false;
                self.update_title(ctx);
                self.set_status(format!(
                    "linked: {message}. Keyboard and first gamepad play the left Game Boy, \
                     the second gamepad the right one (F4 moves the keyboard)"
                ));
            }
            Err(e) => self.set_status(format!("cannot open {}: {e}", file_name(path))),
        }
    }

    fn pick_link_rom(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Game for the second Game Boy")
            .add_filter("Game Boy ROM", &["gb", "gbc", "sgb", "bin", "zip"]);
        if let Some(dir) = self.session.as_ref().and_then(|s| s.rom_path.parent()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.connect_link(ctx, &path);
        }
    }

    /// Unplugs the link cable, closing the second game (its save is written).
    fn disconnect_link(&mut self) {
        if let Some(mut linked) = self.link.take()
            && let Err(e) = linked.session.write_save(true)
        {
            self.set_status(format!("save failed: {e}"));
        }
        self.keyboard_second = false;
        // Nothing faces the infrared port any more.
        if let Some(session) = self.session.as_mut() {
            session.gb.set_ir_input(false);
        }
    }

    /// Plugs the Game Boy Printer in or out (it shares the port with the link cable).
    fn set_printer(&mut self, ctx: &egui::Context, on: bool) {
        if on && self.link.is_some() {
            self.disconnect_link();
            self.update_title(ctx);
        }
        self.printer = on;
        if on {
            self.printer_view.open = true;
            self.set_status(
                "Game Boy Printer connected: print from the game; each page is saved as \
                 a PNG next to the ROM",
            );
        } else {
            if let Some(session) = self.session.as_mut() {
                session.gb.set_printer(false);
            }
            self.set_status("Game Boy Printer disconnected");
        }
    }

    /// Saves the pages the printer fed out, next to the ROM; opens the Printer window when
    /// the game starts printing.
    fn collect_printouts(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(printer) = session.gb.printer_mut() else {
            return;
        };
        if printer.take_changed() {
            self.printer_view.open = true;
        }
        let pages = printer.take_pages();
        for page in pages {
            let saved = session
                .next_print_file()
                .ok_or("too many printouts".into())
                .and_then(|path| printout::save_png(&path, &page.shades).map(|()| path));
            let message = match &saved {
                Ok(path) => format!("printed a page: saved {}", file_name(path)),
                Err(e) => format!("printed a page, but it could not be saved: {e}"),
            };
            self.printer_view.page_done(page.shades, saved.ok());
            self.status = Some((message, Instant::now()));
        }
    }

    /// Save states and rewind would move one Game Boy away from the other: they are off
    /// while linked. Returns whether they are (and says so).
    /// Saves the state in the current slot, with the screen as shown as its preview.
    fn save_state(&mut self) {
        if self.link_blocks_states() {
            return;
        }
        let (palette, correct) = (self.settings.palette, self.settings.color_correction);
        self.with_session(|s| {
            let (rgba, size) = settings::screen_rgba(s.gb.screen(), palette, correct);
            s.save_state((&rgba, size))
        });
    }

    fn load_state(&mut self) {
        if !self.link_blocks_states() {
            self.with_session(|s| s.load_state());
        }
    }

    fn link_blocks_states(&mut self) -> bool {
        if self.link.is_some() {
            self.set_status("save states and rewind are off while two Game Boys are linked");
        }
        self.link.is_some()
    }

    /// Writes `game.cht` when cheats were added, removed or switched (from the cheat
    /// manager, the memory search or the terminal console).
    fn write_changed_cheats(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if session.gb.cheats_mut().take_changed()
            && let Err(e) = console::save_cheat_file(&session.cheat_file(), &session.gb)
        {
            self.set_status(e);
        }
    }

    /// Resets the game, on the console now chosen (the title says which).
    fn reset(&mut self, ctx: &egui::Context) {
        self.with_session(|s| s.reset().map(|()| "reset".to_string()));
        self.update_title(ctx);
    }

    /// Puts your general settings back after a game with its own settings.
    fn restore_general_settings(&mut self) {
        if let Some(general) = self.general_settings.take() {
            general.apply(&mut self.settings);
        }
    }

    /// Emulation → Settings for this game: keeps the console, palette, colour correction
    /// and frame blending for the loaded game only (`on`), or drops them.
    fn set_game_settings(&mut self, on: bool) {
        let Some(key) = self.game_key.clone() else {
            return;
        };
        if on {
            self.settings
                .games
                .insert(key, GameSettings::of(&self.settings));
            self.general_settings = Some(GameSettings::of(&self.settings));
            self.set_status(
                "this game keeps its own console, palette, colour correction and frame blending",
            );
        } else {
            self.settings.games.remove(&key);
            let console = self.settings.console;
            self.restore_general_settings();
            if self.settings.console == console {
                self.set_status("this game uses your general settings again");
            } else {
                self.start_changed("this game uses your general settings again");
            }
        }
    }

    /// Starts or stops recording a video of the screen as shown, with sound.
    fn toggle_recording(&mut self) {
        if let Some(recorder) = self.recorder.take() {
            self.set_status(format!(
                "finishing {} ({:.0} s)…",
                file_name(&recorder.path),
                recorder.seconds()
            ));
            self.finishing.push(recorder.stop());
            return;
        }
        let Some(session) = self.session.as_mut() else {
            self.set_status("no game loaded");
            return;
        };
        let Some(path) = session.next_file("webm") else {
            self.set_status("too many videos");
            return;
        };
        // Without a sound device the game makes no sound; record it anyway.
        if self.audio.is_none() {
            session.gb.set_audio_sample_rate(48_000);
            if let Some(linked) = self.link.as_mut() {
                linked.session.gb.set_audio_sample_rate(48_000);
            }
        }
        let size = self.screen_size();
        let clock = self
            .session
            .as_ref()
            .map_or(freki_core::CLOCK_HZ, |s| s.gb.clock_hz());
        match Recorder::start(&path, size, clock) {
            Ok(recorder) => {
                self.recorder = Some(recorder);
                self.set_status(format!("recording {} (Shift+F12 stops)", file_name(&path)));
            }
            Err(e) => self.set_status(format!("cannot record: {e}")),
        }
    }

    /// Reports recordings whose file is finished. With `wait`, waits for all of them.
    fn poll_recordings(&mut self, wait: bool) {
        let (done, running): (Vec<_>, Vec<_>) = std::mem::take(&mut self.finishing)
            .into_iter()
            .partition(|h| wait || h.is_finished());
        self.finishing = running;
        for handle in done {
            let message = match handle.join() {
                Ok(Ok(summary)) => format!(
                    "saved {} ({:.0} s, {:.1} MB)",
                    file_name(&summary.path),
                    summary.frames as f64 / FRAMES_PER_SECOND,
                    summary.bytes as f64 / 1e6
                ),
                Ok(Err(e)) => format!("video failed: {e}"),
                Err(_) => "video failed: the encoder crashed".to_string(),
            };
            self.set_status(message);
        }
    }

    fn close_rom(&mut self) {
        if self.recorder.is_some() {
            self.toggle_recording();
        }
        self.disconnect_link();
        self.restore_general_settings();
        self.game_key = None;
        if let Some(mut session) = self.session.take()
            && let Err(e) = session.write_save(true)
        {
            self.set_status(format!("save failed: {e}"));
        }
    }

    /// Lets the player choose the picture the Game Boy Camera sees.
    fn pick_camera_picture(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Picture for the Game Boy Camera")
            .add_filter("Picture", &["png", "jpg", "jpeg"]);
        if let Some(dir) = self
            .settings
            .camera_picture
            .as_ref()
            .and_then(|p| p.parent())
        {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.pick_file() else {
            return;
        };
        match camera_picture::load(&path) {
            Ok(view) => {
                self.set_status(format!("the camera sees {}", file_name(&path)));
                self.settings.camera_picture = Some(path);
                self.settings.camera_webcam = None;
                self.set_camera_view(Some(view));
            }
            Err(e) => self.set_status(e.to_string()),
        }
    }

    /// The Camera: webcam menu: the webcams found, to choose the one the camera sees.
    fn webcam_menu(&mut self, ui: &mut egui::Ui) {
        if let Some(search) = &self.webcam_search
            && let Ok(found) = search.try_recv()
        {
            self.webcams = Some(found);
            self.webcam_search = None;
        }
        if self.webcams.is_none() && self.webcam_search.is_none() {
            // In the background: the system may ask the player for camera access first.
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let _ = tx.send(webcam::list());
                ctx.request_repaint();
            });
            self.webcam_search = Some(rx);
        }
        let mut chosen = None;
        match &self.webcams {
            None => {
                ui.label("Looking for webcams…");
            }
            Some(Err(e)) => {
                ui.label(e);
            }
            Some(Ok(names)) if names.is_empty() => {
                ui.label("No webcam found");
            }
            Some(Ok(names)) => {
                for name in names {
                    let on = self.settings.camera_webcam.as_ref() == Some(name);
                    if ui.radio(on, name).clicked() {
                        chosen = Some(name.clone());
                    }
                }
            }
        }
        if let Some(name) = chosen {
            ui.close();
            self.settings.camera_webcam = Some(name);
            // Started by the next frame.
            self.webcam = None;
            self.webcam_failed = false;
        }
        if self.webcam_search.is_none() && ui.button("Look again").clicked() {
            self.webcams = None;
        }
    }

    /// Uses `view` (`None`: the test pattern) as the camera's picture from now on.
    fn set_camera_view(&mut self, view: Option<Vec<u8>>) {
        self.camera_view = view;
        if let Some(session) = self.session.as_mut() {
            session.gb.set_camera_image(self.camera_view.as_deref());
        }
    }

    fn pick_rom(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Open Game Boy ROM")
            .add_filter("Game Boy ROM", &["gb", "gbc", "sgb", "bin", "zip"]);
        if let Some(dir) = self.session.as_ref().and_then(|s| s.rom_path.parent()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.open_rom(ctx, &path);
        }
    }

    fn with_session(
        &mut self,
        f: impl FnOnce(&mut Session) -> Result<String, Box<dyn std::error::Error>>,
    ) {
        let Some(session) = self.session.as_mut() else {
            self.set_status("no game loaded");
            return;
        };
        let message = f(session).unwrap_or_else(|e| format!("error: {e}"));
        self.set_status(message);
    }

    fn set_scale(&mut self, ctx: &egui::Context, scale: u32) {
        self.settings.scale = scale;
        // Room for the menu bar and status bar around the screen.
        let [width, height] = self.screen_size();
        let size = vec2(
            (width as u32 * scale) as f32,
            (height as u32 * scale) as f32 + 56.0,
        );
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }

    // ---- input and emulation ---------------------------------------------------------

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        // Hotkeys pressed while a tool window had focus count as pressed here.
        let forwarded = take_forwarded_hotkeys(ctx);
        let tool_command = |key| forwarded.iter().any(|&(m, k)| k == key && m.command);
        let tool_key = |key| forwarded.iter().any(|&(m, k)| k == key && !m.command);
        if ctx.input_mut(|i| i.consume_shortcut(&OPEN)) || tool_command(OPEN.logical_key) {
            self.pick_rom(ctx);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&QUIT)) || tool_command(QUIT.logical_key) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&RESET)) || tool_command(RESET.logical_key) {
            self.reset(ctx);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&FULLSCREEN)) {
            set_fullscreen(ctx, !is_fullscreen(ctx));
        }
        if is_fullscreen(ctx) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            set_fullscreen(ctx, false);
        }
        // Single-key hotkeys don't fire while typing into a text field.
        if ctx.text_edit_focused() {
            return;
        }
        let pressed = |key| ctx.input(|i| i.key_pressed(key)) || tool_key(key);
        if pressed(Key::F5) {
            self.save_state();
        }
        if pressed(Key::F8) {
            self.load_state();
        }
        if pressed(Key::F4) && self.link.is_some() {
            self.keyboard_second = !self.keyboard_second;
            let side = if self.keyboard_second {
                "right"
            } else {
                "left"
            };
            self.set_status(format!("the keyboard plays the {side} Game Boy"));
        }
        if pressed(Key::F6) || pressed(Key::F7) {
            let next = pressed(Key::F7);
            self.with_session(|s| {
                if next {
                    s.slots.next();
                } else {
                    s.slots.previous();
                }
                Ok(s.slots.describe(s.slots.slot()))
            });
        }
        if pressed(Key::F9) {
            self.toggle_cheats();
        }
        if pressed(Key::M) {
            self.settings.muted = !self.settings.muted;
            let msg = if self.settings.muted {
                "muted"
            } else {
                "sound on"
            };
            self.set_status(msg);
        }
        // Shift+F12 records video; F12 alone takes a screenshot.
        let tool_shift_f12 = forwarded
            .iter()
            .any(|&(m, k)| k == Key::F12 && m.shift && !m.command);
        if ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::F12)) || tool_shift_f12 {
            self.toggle_recording();
        } else if pressed(Key::F12) {
            self.screenshot();
        }
        if pressed(Key::P) {
            self.set_paused(!self.paused);
        }
        if pressed(Key::F10) && self.paused {
            self.debugger_action(cpu_view::Action::Continue);
        }
        if pressed(Key::F11) && self.paused {
            self.debugger_action(cpu_view::Action::StepInstruction);
        }
        let rewind = ctx.input(|i| i.key_down(Key::R)) && self.session.is_some();
        if rewind && !self.rewinding && self.link_blocks_states() {
            return;
        }
        self.rewinding = rewind;
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if !paused {
            self.cpu_view.last_stop = None;
        }
        self.set_status(if paused { "paused" } else { "resumed" });
    }

    /// Executes a debugger request (continue, pause or one of the step sizes).
    fn debugger_action(&mut self, action: cpu_view::Action) {
        use cpu_view::Action;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let gb = &mut session.gb;
        let result: Result<Option<freki_core::Stop>, freki_core::CpuError> = match action {
            Action::Continue => {
                self.set_paused(false);
                return;
            }
            Action::Pause => {
                self.set_paused(true);
                return;
            }
            // A step always executes, passing over a breakpoint at the current PC.
            Action::StepInstruction => match gb.step_checked() {
                Ok(Some(freki_core::Stop::Breakpoint { .. })) => gb.step_checked(),
                other => other,
            },
            Action::StepScanline => {
                let ly = gb.peek(0xFF44);
                let mut result = Ok(None);
                // A scanline is 456 dots; bound the loop in case the LCD is off.
                for _ in 0..200 {
                    result = gb.step_checked();
                    if !matches!(result, Ok(None)) || gb.peek(0xFF44) != ly {
                        break;
                    }
                }
                result
            }
            Action::StepFrame => gb.run_frame_checked(),
        };
        gb.drain_audio(&mut self.samples);
        self.samples.clear();
        match result {
            Ok(stop) => self.cpu_view.last_stop = stop,
            Err(e) => {
                let cheats = gb.cheats().any_active();
                self.set_status(stopped_message(&e, cheats));
            }
        }
    }

    fn toggle_cheats(&mut self) {
        self.with_session(|s| {
            let cheats = s.gb.cheats_mut();
            cheats.enabled = !cheats.enabled;
            let state = if cheats.enabled { "on" } else { "off" };
            Ok(format!("cheats {state} ({})", cheats.list().len()))
        });
    }

    /// Emulation speed multiplier: 1, or the fast-forward speed.
    fn speed(&self) -> u32 {
        if (self.fast_forward || self.fast_forward_held) && !self.rewinding {
            self.settings.fast_forward_speed
        } else {
            1
        }
    }

    /// Runs as many frames as real time (and the audio queue) calls for.
    fn emulate(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        let speed = self.speed();
        let fps = self
            .session
            .as_ref()
            .map_or(FRAMES_PER_SECOND, |s| s.gb.frames_per_second());
        self.frame_debt = (self.frame_debt + elapsed * fps * speed as f64)
            .min((MAX_FRAMES_PER_UPDATE * speed) as f64);
        let frames = self.frame_debt.floor() as u32;
        self.frame_debt -= frames as f64;

        let typing = ctx.text_edit_focused();
        // In the background (no freki window focused), the game waits for the player.
        // Focus moving between freki's own windows leaves none focused for a moment.
        let unfocused = self.settings.pause_in_background
            && self.recorder.is_none()
            && !window::any_window_focused(ctx);
        if !unfocused {
            self.unfocused_since = None;
        }
        let since = *self.unfocused_since.get_or_insert(now);
        let background = unfocused && now.duration_since(since) >= FOCUS_GRACE;
        if background != self.background_paused {
            self.background_paused = background;
            if !self.paused && self.session.is_some() {
                self.set_status(if background {
                    "paused in the background"
                } else {
                    "resumed"
                });
            }
        }
        if background {
            return;
        }

        let Some(session) = self.session.as_mut() else {
            return;
        };
        // Also after a reset or a new game (a new Game Boy).
        session.gb.set_printer(self.printer && self.link.is_none());
        session.gb.set_muted_channels(self.sound_view.muted);
        // The camera picture chosen last time, decoded once.
        if session.gb.has_camera() && !self.camera_loaded {
            self.camera_loaded = true;
            if let Some(path) = &self.settings.camera_picture {
                match camera_picture::load(path) {
                    Ok(view) => self.camera_view = Some(view),
                    Err(e) => self.status = Some((e.to_string(), Instant::now())),
                }
            }
        }
        // The chosen webcam runs while a camera cartridge is in.
        if session.gb.has_camera() && self.settings.camera_webcam.is_some() {
            if self.webcam.is_none() && !self.webcam_failed {
                self.webcam = Some(Webcam::start(self.settings.camera_webcam.clone()));
            }
        } else {
            self.webcam = None;
        }
        while let Some(event) = self.webcam.as_ref().and_then(Webcam::poll) {
            match event {
                webcam::Event::Opened(name) => {
                    self.status = Some((format!("the camera sees {name}"), Instant::now()));
                }
                webcam::Event::View(view) => {
                    session.gb.set_camera_image(Some(&view));
                    self.camera_view = Some(view);
                }
                webcam::Event::Failed(e) => {
                    self.status = Some((e, Instant::now()));
                    self.webcam = None;
                    self.webcam_failed = true;
                }
            }
        }
        // Also after a reset or a loaded state (a new Game Boy).
        if session.gb.has_camera()
            && !session.gb.camera_has_image()
            && let Some(view) = &self.camera_view
        {
            session.gb.set_camera_image(Some(view));
        }
        // Without a sound device the game makes no sound; the sound window needs it.
        if self.sound_view.open && !session.gb.produces_audio() {
            session.gb.set_audio_sample_rate(48_000);
        }
        if let Some(linked) = self.link.as_mut() {
            linked.session.gb.set_muted_channels(self.sound_view.muted);
        }

        if self.rewinding {
            self.previous_frame = None;
            for _ in 0..frames {
                match session.rewind_step() {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => {
                        eprintln!("freki: rewind failed: {e}");
                        break;
                    }
                }
            }
            let left = session.rewind_seconds();
            session.gb.drain_audio(&mut self.samples);
            self.samples.clear();
            self.status = Some((format!("rewinding ({left:.1} s left)"), Instant::now()));
            return;
        }
        if self.paused {
            return;
        }
        self.timing.record(frames);

        // Keep audio in sync without touching video: steer the sample rate so the buffer
        // stays near its target, and refill it with silence if it ran dry.
        // While fast-forwarding, sound is muted (the queue is kept topped up with silence).
        if let Some(audio) = &self.audio {
            let rate = audio.sample_rate() as f64;
            let target = rate * AUDIO_LATENCY.as_secs_f64();
            let mut queued = audio.queued_frames() as f64;
            if queued < target / 4.0 {
                audio.push_silence((target - queued) as usize);
                queued = target;
            }
            let adjust = ((target - queued) / target).clamp(-1.0, 1.0) * MAX_RATE_ADJUST;
            let adjusted = (rate * (1.0 + adjust)).round() as u32;
            session.gb.adjust_audio_sample_rate(adjusted);
            if let Some(linked) = self.link.as_mut() {
                linked.session.gb.adjust_audio_sample_rate(adjusted);
            }
            self.timing.audio_ms = queued / rate * 1000.0;
            self.timing.rate_adjust = adjust;
        }

        self.gamepads.update();
        let bindings = &self.settings.bindings;
        // Linked: the first gamepad plays the left Game Boy, the second the right one, and
        // the keyboard either.
        let (first_pad, keyboard_second) = match self.link {
            Some(_) => (Some(0), self.keyboard_second),
            None => (None, false),
        };
        // Alt+Enter toggles fullscreen; it must not press the button on Enter.
        let key_down = |control: input::Control| {
            !typing && ctx.input(|i| !i.modifiers.alt && i.key_down(bindings.key(control)))
        };
        // Whether `control` is held on the left (0) or right (1) Game Boy.
        let gamepads = &self.gamepads;
        let down = |side: usize, control: input::Control| {
            let pad = if side == 0 { first_pad } else { Some(1) };
            (key_down(control) && (side == 1) == keyboard_second)
                || gamepads.is_pressed_on(pad, control.stick(), bindings.pad(control))
        };
        for _ in 0..frames {
            // Turbo A and B: pressed and released in turn while held.
            let mut turbo = [[false; 2]; 2];
            for (side, counters) in self.turbo.iter_mut().enumerate() {
                for (n, control) in input::TURBOS.into_iter().enumerate() {
                    if down(side, control) {
                        turbo[side][n] = input::turbo_pressed(counters[n]);
                        counters[n] += 1;
                    } else {
                        counters[n] = 0;
                    }
                }
            }
            let turbo_for = |side: usize, button: Button| match button {
                Button::A => turbo[side][0],
                Button::B => turbo[side][1],
                _ => false,
            };
            let mut held = [false; input::BUTTONS.len()];
            for (i, button) in input::BUTTONS.into_iter().enumerate() {
                held[i] = down(0, button.into()) || turbo_for(0, button);
                session.gb.set_button(button, held[i]);
                if let Some(linked) = self.link.as_mut() {
                    let pressed = down(1, button.into()) || turbo_for(1, button);
                    linked.session.gb.set_button(button, pressed);
                }
            }
            for control in input::SHOULDERS {
                if let input::Control::Shoulder(shoulder) = control {
                    session.gb.set_sgb_shoulder(shoulder, down(0, control));
                }
            }
            if session.gb.has_tilt_sensor() {
                // MBC7 tilt: the left stick tilts as far as it is pushed; the D-pad (and
                // arrow keys) tilt all the way, eased in and out.
                let pressed =
                    |b: Button| held[input::BUTTONS.iter().position(|&x| x == b).unwrap()];
                let axis = |neg: Button, pos: Button| {
                    f32::from(u8::from(pressed(pos))) - f32::from(u8::from(pressed(neg)))
                };
                let (sx, sy) = self.gamepads.left_stick();
                let target = (
                    (axis(Button::Left, Button::Right) + sx).clamp(-1.0, 1.0),
                    // Kirby rolls the way the D-pad or stick points.
                    (axis(Button::Up, Button::Down) - sy).clamp(-1.0, 1.0),
                );
                self.tilt.0 += (target.0 - self.tilt.0) * TILT_EASE;
                self.tilt.1 += (target.1 - self.tilt.1) * TILT_EASE;
                session.gb.set_tilt(self.tilt.0, self.tilt.1);
            }
            if self.settings.frame_blend {
                self.previous_frame = Some(picture(
                    session,
                    self.link.as_ref(),
                    self.settings.palette,
                    self.settings.color_correction,
                ));
            }
            let result = match self.link.as_mut() {
                Some(linked) => linked
                    .cable
                    .run_frame(&mut session.gb, &mut linked.session.gb)
                    .map(|()| None)
                    .map_err(Into::into),
                None => session.run_frame(),
            };
            match result {
                Ok(None) => {}
                Ok(Some(stop)) => {
                    // Breakpoint or watchpoint: pause and show the debugger.
                    self.paused = true;
                    let msg = console::describe_stop(&stop, &session.gb);
                    eprintln!("freki: {msg}");
                    self.status = Some((msg, Instant::now()));
                    self.cpu_view.last_stop = Some(stop);
                    self.cpu_view.open = true;
                    break;
                }
                Err(e) => {
                    self.paused = true;
                    let cheats = session.gb.cheats().any_active()
                        || self
                            .link
                            .as_ref()
                            .is_some_and(|l| l.session.gb.cheats().any_active());
                    let msg = stopped_message(&e, cheats);
                    eprintln!("freki: {msg}");
                    self.status = Some((msg, Instant::now()));
                    break;
                }
            }
            session.gb.drain_audio(&mut self.samples);
            // Linked: the sound of the Game Boy the keyboard plays.
            if let Some(linked) = self.link.as_mut() {
                let first = self.samples.len();
                linked.session.gb.drain_audio(&mut self.samples);
                if keyboard_second {
                    self.samples.drain(..first);
                } else {
                    self.samples.truncate(first);
                }
            }
            if let Some(recorder) = self.recorder.as_mut() {
                let (mut rgba, size) = picture(
                    session,
                    self.link.as_ref(),
                    self.settings.palette,
                    self.settings.color_correction,
                );
                if let Some((previous, previous_size)) = &self.previous_frame
                    && *previous_size == size
                {
                    settings::blend(previous, &mut rgba);
                }
                if let Err(e) = recorder.frame(&rgba, size, &self.samples) {
                    let recorder = self.recorder.take().expect("recording");
                    self.finishing.push(recorder.stop());
                    self.status = Some((format!("recording stopped: {e}"), Instant::now()));
                }
            }
            if let Some(audio) = &self.audio
                && speed == 1
            {
                // Volume applies to what is heard only; recordings keep the full level.
                let gain = self.settings.gain();
                if gain == 1.0 {
                    audio.push(&self.samples);
                } else {
                    self.samples.iter_mut().for_each(|s| *s *= gain);
                    audio.push(&self.samples);
                }
            }
            self.samples.clear();
        }
        let mut saved = session.autosave();
        if let Some(linked) = self.link.as_mut() {
            saved = saved.and(linked.session.autosave());
        }
        if let Err(e) = saved {
            self.status = Some((format!("save failed: {e}"), Instant::now()));
        }
    }

    /// Passes a rumble cartridge's motor on to the gamepads (stopped while not running).
    fn update_rumble(&mut self) {
        let strength = match self.session.as_mut() {
            Some(s)
                if self.settings.rumble
                    && !self.paused
                    && !self.background_paused
                    && !self.rewinding =>
            {
                s.gb.take_rumble()
            }
            _ => Some(0.0),
        };
        // `None`: no frame ran since the last update; keep the current strength.
        if let Some(strength) = strength {
            self.gamepads.set_rumble(strength);
        }
    }

    fn update_texture(&mut self, ctx: &egui::Context) {
        let (mut rgba, size) = self.screen_rgba();
        if !self.settings.frame_blend {
            self.previous_frame = None;
        }
        if let Some((previous, previous_size)) = &self.previous_frame
            && *previous_size == size
        {
            settings::blend(previous, &mut rgba);
        }
        let image = ColorImage::from_rgba_unmultiplied(size, &rgba);
        // A game with the other picture size (Super Game Boy or not): fit the window to it.
        if size != self.fitted_size && !is_fullscreen(ctx) {
            self.fitted_size = size;
            self.set_scale(ctx, self.settings.scale);
        }
        match &mut self.texture {
            Some(texture) => texture.set(image, TextureOptions::NEAREST),
            None => self.texture = Some(ctx.load_texture("screen", image, TextureOptions::NEAREST)),
        }
    }

    /// The screen as shown (palette or colour correction applied), as RGBA, with its size.
    fn screen_rgba(&self) -> (Vec<u8>, [usize; 2]) {
        match self.session.as_ref() {
            Some(s) => picture(
                s,
                self.link.as_ref(),
                self.settings.palette,
                self.settings.color_correction,
            ),
            None => (
                vec![0x20; SCREEN_WIDTH * SCREEN_HEIGHT * 4],
                [SCREEN_WIDTH, SCREEN_HEIGHT],
            ),
        }
    }

    /// Size of the picture: 160×144, or 256×224 on a Super Game Boy; both side by side
    /// while linked.
    fn screen_size(&self) -> [usize; 2] {
        let size = |s: &Session| s.gb.screen().size();
        match (self.session.as_ref(), self.link.as_ref()) {
            (Some(one), Some(two)) => {
                let ((w1, h1), (w2, h2)) = (size(one), size(&two.session));
                [w1 + settings::LINK_GAP + w2, h1.max(h2)]
            }
            (Some(one), None) => size(one).into(),
            (None, _) => [SCREEN_WIDTH, SCREEN_HEIGHT],
        }
    }

    /// Saves the screen as shown to a PNG next to the ROM.
    fn screenshot(&mut self) {
        let (rgba, size) = self.screen_rgba();
        self.with_session(|s| {
            let path = s.save_screenshot(&rgba, size)?;
            Ok(format!("saved {}", file_name(&path)))
        });
    }

    // ---- UI ----------------------------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let loaded = self.session.is_some();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui
                    .add(egui::Button::new("Open ROM…").shortcut_text(ctx.format_shortcut(&OPEN)))
                    .clicked()
                {
                    ui.close();
                    self.pick_rom(&ctx);
                }
                ui.add_enabled_ui(!self.settings.recent.is_empty(), |ui| {
                    ui.menu_button("Recent", |ui| {
                        let mut open = None;
                        for path in &self.settings.recent {
                            if ui
                                .button(file_name(path))
                                .on_hover_text(path.display().to_string())
                                .clicked()
                            {
                                open = Some(path.clone());
                            }
                        }
                        ui.separator();
                        if ui.button("Clear list").clicked() {
                            self.settings.recent.clear();
                            ui.close();
                        }
                        if let Some(path) = open {
                            ui.close();
                            self.open_rom(&ctx, &path);
                        }
                    });
                });
                if ui
                    .add_enabled(loaded, egui::Button::new("Close ROM"))
                    .clicked()
                {
                    ui.close();
                    self.close_rom();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title("freki".into()));
                }
                if ui
                    .add_enabled(
                        loaded,
                        egui::Button::new("Save screenshot").shortcut_text("F12"),
                    )
                    .clicked()
                {
                    ui.close();
                    self.screenshot();
                }
                let recording = self.recorder.is_some();
                let label = if recording {
                    "Stop recording"
                } else {
                    "Record video"
                };
                if ui
                    .add_enabled(
                        loaded || recording,
                        egui::Button::new(label).shortcut_text("Shift+F12"),
                    )
                    .on_hover_text("WebM video with sound, saved next to the ROM")
                    .clicked()
                {
                    ui.close();
                    self.toggle_recording();
                }
                ui.separator();
                if ui
                    .add(egui::Button::new("Quit").shortcut_text(ctx.format_shortcut(&QUIT)))
                    .clicked()
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            ui.menu_button("Emulation", |ui| {
                if ui
                    .add_enabled(loaded, egui::Checkbox::new(&mut self.paused, "Paused"))
                    .changed()
                {
                    let msg = if self.paused { "paused" } else { "resumed" };
                    self.set_status(msg);
                }
                ui.checkbox(
                    &mut self.settings.pause_in_background,
                    "Pause in the background",
                )
                .on_hover_text("Pause while no freki window has focus; resumes when you return");
                ui.add_enabled(false, egui::Button::new("Rewind").shortcut_text("hold R"));
                ui.horizontal(|ui| {
                    ui.add_enabled(
                        loaded,
                        egui::Checkbox::new(&mut self.fast_forward, "Fast-forward"),
                    );
                    ui.weak("or hold Tab");
                });
                ui.menu_button("Fast-forward speed", |ui| {
                    for speed in FAST_FORWARD_SPEEDS {
                        ui.radio_value(
                            &mut self.settings.fast_forward_speed,
                            speed,
                            format!("{speed}×"),
                        );
                    }
                });
                if ui
                    .add_enabled(
                        loaded,
                        egui::Button::new("Reset").shortcut_text(ctx.format_shortcut(&RESET)),
                    )
                    .clicked()
                {
                    ui.close();
                    self.reset(&ctx);
                }
                ui.separator();
                ui.menu_button("Console", |ui| self.console_menu(ui));
                let mut own = self.general_settings.is_some();
                if ui
                    .add_enabled(
                        self.game_key.is_some(),
                        egui::Checkbox::new(&mut own, "Settings for this game"),
                    )
                    .on_hover_text(
                        "Keep the console, palette, colour correction and frame blending \
                         chosen now for this game only; your general settings come back \
                         for other games",
                    )
                    .changed()
                {
                    self.set_game_settings(own);
                }
                ui.menu_button("Boot ROM", |ui| self.boot_rom_menu(ui));
                ui.separator();
                if ui.button("Controls…").clicked() {
                    ui.close();
                    self.show_controls = true;
                }
                let camera = self.session.as_ref().is_some_and(|s| s.gb.has_camera());
                if camera {
                    let current =
                        match (&self.settings.camera_webcam, &self.settings.camera_picture) {
                            (Some(name), _) => name.clone(),
                            (None, Some(path)) => file_name(path),
                            (None, None) => "its test pattern".to_string(),
                        };
                    if ui
                        .button("Camera picture…")
                        .on_hover_text(format!(
                            "What the Game Boy Camera sees (now: {current}); a PNG or JPEG"
                        ))
                        .clicked()
                    {
                        ui.close();
                        self.pick_camera_picture();
                    }
                    ui.menu_button("Camera: webcam", |ui| self.webcam_menu(ui))
                        .response
                        .on_hover_text("The Game Boy Camera sees a webcam");
                    if (self.settings.camera_picture.is_some()
                        || self.settings.camera_webcam.is_some())
                        && ui.button("Camera: test pattern").clicked()
                    {
                        ui.close();
                        self.settings.camera_picture = None;
                        self.settings.camera_webcam = None;
                        self.set_camera_view(None);
                        self.set_status("the camera sees its test pattern");
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Volume");
                    ui.add(egui::Slider::new(&mut self.settings.volume, 0..=100).suffix(" %"));
                });
                if ui
                    .add(egui::Checkbox::new(&mut self.settings.muted, "Mute"))
                    .on_hover_text("M")
                    .changed()
                {
                    let msg = if self.settings.muted {
                        "muted"
                    } else {
                        "sound on"
                    };
                    self.set_status(msg);
                }
                if ui
                    .checkbox(&mut self.settings.gamepads, "Gamepads")
                    .on_hover_text(GAMEPADS_HINT)
                    .changed()
                {
                    self.gamepads.set_on(self.settings.gamepads);
                }
                ui.add_enabled(
                    self.settings.gamepads,
                    egui::Checkbox::new(&mut self.settings.rumble, "Gamepad rumble"),
                )
                .on_hover_text("Rumble cartridges (MBC5 with motor) shake the controller");
                ui.separator();
                ui.add_enabled_ui(loaded, |ui| {
                    ui.menu_button("Link cable", |ui| self.link_menu(ui));
                });
            });

            // Clicks on the slot grid keep the menu open, so a slot can be double-clicked.
            let states_menu = egui::containers::menu::MenuConfig::new()
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
            egui::containers::menu::MenuButton::new("States")
                .config(states_menu)
                .ui(ui, |ui| {
                    if ui
                        .add_enabled(loaded, egui::Button::new("Save state").shortcut_text("F5"))
                        .clicked()
                    {
                        ui.close();
                        self.save_state();
                    }
                    if ui
                        .add_enabled(loaded, egui::Button::new("Load state").shortcut_text("F8"))
                        .clicked()
                    {
                        ui.close();
                        self.load_state();
                    }
                    ui.separator();
                    ui.label("Slot (F6 / F7)");
                    let pick = match self.session.as_ref() {
                        Some(session) => self.state_previews.grid(ui, &session.slots),
                        None => None,
                    };
                    match pick {
                        Some(Pick::Select(slot)) => {
                            if let Some(session) = self.session.as_mut() {
                                session.slots.select(slot);
                            }
                        }
                        Some(Pick::Load(slot)) => {
                            if let Some(session) = self.session.as_mut() {
                                session.slots.select(slot);
                            }
                            ui.close();
                            self.load_state();
                        }
                        None => {}
                    }
                });

            ui.menu_button("Cheats", |ui| {
                if let Some(session) = self.session.as_mut() {
                    let cheats = session.gb.cheats_mut();
                    let count = cheats.list().len();
                    ui.add(egui::Checkbox::new(
                        &mut cheats.enabled,
                        format!("Cheats enabled ({count})"),
                    ))
                    .on_hover_text("F9");
                } else {
                    ui.add_enabled(false, egui::Checkbox::new(&mut false, "Cheats enabled"));
                }
                if ui
                    .add_enabled(loaded, egui::Button::new("Cheat manager…"))
                    .clicked()
                {
                    ui.close();
                    self.show_cheats = true;
                }
                if ui
                    .add_enabled(loaded, egui::Button::new("Memory search…"))
                    .clicked()
                {
                    ui.close();
                    self.show_search = true;
                }
            });

            ui.menu_button("View", |ui| {
                let fullscreen = is_fullscreen(&ctx);
                if ui
                    .add(
                        egui::Button::selectable(fullscreen, "Fullscreen")
                            .shortcut_text(ctx.format_shortcut(&FULLSCREEN)),
                    )
                    .clicked()
                {
                    ui.close();
                    set_fullscreen(&ctx, !fullscreen);
                }
                ui.checkbox(&mut self.settings.integer_scaling, "Integer scaling")
                    .on_hover_text("Whole-number scale only: sharp, even pixels");
                ui.checkbox(&mut self.settings.frame_blend, "Frame blending")
                    .on_hover_text(
                        "Mix each frame with the one before, like the slow LCD of the real \
                     console: sprites that games flicker every other frame look steady",
                    );
                ui.separator();
                ui.label("Window scale");
                for scale in 2..=6 {
                    if ui
                        .radio(self.settings.scale == scale, format!("{scale}×"))
                        .clicked()
                    {
                        ui.close();
                        self.set_scale(&ctx, scale);
                    }
                }
                ui.separator();
                ui.checkbox(&mut self.show_timing, "Show frame timing");
                ui.add_enabled(
                    loaded,
                    egui::Checkbox::new(&mut self.cpu_view.open, "Debugger"),
                );
                ui.add_enabled(
                    loaded,
                    egui::Checkbox::new(&mut self.sound_view.open, "Sound channels"),
                );
                ui.add_enabled(
                    loaded,
                    egui::Checkbox::new(&mut self.debug.show_tiles, "Tiles"),
                );
                ui.add_enabled(
                    loaded,
                    egui::Checkbox::new(&mut self.debug.show_map, "Tile map"),
                );
                ui.add_enabled(
                    loaded,
                    egui::Checkbox::new(&mut self.debug.show_sprites, "Sprites"),
                );
                ui.add_enabled(
                    loaded,
                    egui::Checkbox::new(&mut self.debug.show_palettes, "Palettes"),
                );
                ui.separator();
                ui.label("Palette (Game Boy)");
                for palette in Palette::ALL {
                    ui.radio_value(&mut self.settings.palette, palette, palette.name());
                }
                ui.separator();
                ui.checkbox(&mut self.settings.color_correction, "GBC colour correction")
                    .on_hover_text("Colours as on the Game Boy Color's screen (less saturated)");
            });

            ui.menu_button("Help", |ui| {
                if ui.button("Controls…").clicked() {
                    ui.close();
                    self.show_controls = true;
                }
                if ui.button("About freki…").clicked() {
                    ui.close();
                    self.show_about = true;
                }
            });
        });
    }

    fn link_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let linked = self.link.is_some();
        let connect = if linked {
            "Change second game…"
        } else {
            "Connect a second game…"
        };
        if ui
            .button(connect)
            .on_hover_text(
                "Runs a second Game Boy next to this one, connected by a link cable and \
                 facing it with the infrared port: for trading, two-player games and \
                 Mystery Gift",
            )
            .clicked()
        {
            ui.close();
            self.pick_link_rom(&ctx);
        }
        let mut keyboard_second = self.keyboard_second;
        if ui
            .add_enabled(
                linked,
                egui::Checkbox::new(&mut keyboard_second, "Keyboard plays the right Game Boy"),
            )
            .on_hover_text("F4. Gamepads: the first plays the left one, the second the right one")
            .changed()
        {
            self.keyboard_second = keyboard_second;
        }
        if ui
            .add_enabled(linked, egui::Button::new("Disconnect"))
            .clicked()
        {
            ui.close();
            self.disconnect_link();
            self.update_title(&ctx);
            self.set_status("link cable disconnected");
        }
        ui.separator();
        let mut printer = self.printer;
        if ui
            .checkbox(&mut printer, "Game Boy Printer")
            .on_hover_text(
                "Plugs a Game Boy Printer into the link cable port (in place of a second \
                 game). Printed pages are saved as ROM-print-001.png, … next to the ROM",
            )
            .changed()
        {
            ui.close();
            self.set_printer(&ctx, printer);
        }
        if ui.button("Show printout").clicked() {
            ui.close();
            self.printer_view.open = true;
        }
    }

    fn console_menu(&mut self, ui: &mut egui::Ui) {
        for console in ConsoleModel::ALL {
            if ui
                .radio_value(&mut self.settings.console, console, console.name())
                .clicked()
            {
                self.start_changed(&format!("console: {}", console.name()));
            }
        }
    }

    fn boot_rom_menu(&mut self, ui: &mut egui::Ui) {
        let chosen =
            self.settings.boot_rom_path.is_some() || self.settings.cgb_boot_rom_path.is_some();
        if ui
            .add_enabled(
                chosen,
                egui::Checkbox::new(&mut self.settings.use_boot_rom, "Run boot ROM at start"),
            )
            .changed()
        {
            let state = if self.settings.use_boot_rom {
                "on"
            } else {
                "off"
            };
            self.start_changed(&format!("boot ROM {state}"));
        }
        if ui.button("Choose file…").clicked() {
            ui.close();
            self.pick_boot_rom();
        }
        for (console, path) in [
            ("Game Boy", &self.settings.boot_rom_path),
            ("Game Boy Color", &self.settings.cgb_boot_rom_path),
        ] {
            let file = path.as_deref().map_or("no file chosen".into(), file_name);
            ui.weak(format!("{console}: {file}"));
        }
        ui.weak("Dumps of the consoles' boot ROMs (not included).");
        ui.separator();
        if ui
            .button("Super Game Boy sound ROM…")
            .on_hover_text(
                "The Super Game Boy cartridge's SNES ROM (e.g. Super Game Boy (World) \
                 (Rev 2).sfc): its sound engine plays the SGB sound effects and music games \
                 ask for",
            )
            .clicked()
        {
            ui.close();
            self.pick_sgb_rom();
        }
        if self.settings.sgb_rom_path.is_some() && ui.button("No Super Game Boy sound").clicked() {
            ui.close();
            self.settings.sgb_rom_path = None;
            self.start_changed("Super Game Boy sound off");
        }
        let file = self
            .settings
            .sgb_rom_path
            .as_deref()
            .map_or("no file chosen".into(), file_name);
        ui.weak(format!("Super Game Boy sound: {file}"));
        let mut hardware = self.settings.sgb_hardware;
        if ui
            .add_enabled(
                self.settings.sgb_rom_path.is_some(),
                egui::Checkbox::new(&mut hardware, "Run the SGB's SNES program"),
            )
            .on_hover_text(
                "Emulates the SNES side of the Super Game Boy and runs the SGB ROM's own \
                 program, so games that upload SNES code work (Space Invaders' arcade \
                 mode)",
            )
            .changed()
        {
            self.settings.sgb_hardware = hardware;
            self.start_changed(if hardware {
                "Super Game Boy: runs its SNES program"
            } else {
                "Super Game Boy: built-in model"
            });
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        if self
            .status
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > STATUS_DURATION)
        {
            self.status = None;
        }
        ui.horizontal(|ui| {
            if let Some(recorder) = &self.recorder {
                let seconds = recorder.seconds() as u64;
                ui.colored_label(
                    Color32::from_rgb(0xE0, 0x30, 0x30),
                    format!("● REC {}:{:02}", seconds / 60, seconds % 60),
                );
                ui.separator();
            }
            if let Some(session) = &self.session {
                let cheats = session.gb.cheats();
                ui.label(format!("slot {}", session.slots.slot()));
                ui.separator();
                let n = cheats.list().iter().filter(|c| c.enabled).count();
                ui.label(if cheats.enabled && n > 0 {
                    format!("{n} cheat(s) on")
                } else {
                    "cheats off".to_string()
                });
                ui.separator();
                if self.paused || self.background_paused {
                    ui.label("paused");
                    ui.separator();
                } else if self.speed() > 1 {
                    ui.label(format!("fast-forward {}×", self.speed()));
                    ui.separator();
                }
                if self.settings.muted {
                    ui.label("muted");
                    ui.separator();
                }
            }
            // Pause state is shown above; don't repeat a pause/resume message next to it.
            if self.show_timing && self.session.is_some() {
                ui.label(self.timing.summary());
                ui.separator();
            }
            if let Some((message, _)) = &self.status
                && message != "paused"
            {
                ui.label(message.as_str());
            }
        });
    }

    fn screen(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_rect_before_wrap();
        let Some(texture) = &self.texture else { return };
        if self.session.is_none() {
            ui.centered_and_justified(|ui| {
                ui.label(format!(
                    "File → Open ROM… ({})",
                    ui.ctx().format_shortcut(&OPEN)
                ));
            });
            return;
        }
        // The largest scale that fits; with integer scaling, the largest whole one (pixels
        // stay sharp and even), unless the window is smaller than the screen.
        let [width, height] = texture.size().map(|n| n as f32);
        let fit = (available.width() / width).min(available.height() / height);
        let scale = if self.settings.integer_scaling && fit >= 1.0 {
            fit.floor()
        } else {
            fit
        };
        let size = vec2(width * scale, height * scale);
        let rect = Rect::from_center_size(available.center(), size);
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
    }

    /// The cheat manager, in a window of its own.
    fn cheat_window(&mut self, ctx: &egui::Context) {
        if !self.show_cheats || self.session.is_none() {
            return;
        }
        let mut status = None;
        let open = tool_window(ctx, "cheats", "freki — Cheats", [640.0, 560.0], |ui| {
            status = self.cheat_contents(ui);
        });
        self.show_cheats = open;
        if let Some(message) = status {
            self.set_status(message);
        }
    }

    /// The cheat manager's contents. Returns a status message, if any.
    fn cheat_contents(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let session = self.session.as_mut()?;
        let mut status = None;
        let gb = &mut session.gb;
        let mut remove = None;
        let mut start_edit = None;
        let mut remove_template = None;
        let mut cancel_edit = false;
        let templates = gb.cheats().templates();
        // The list gets the height the rest leaves.
        let reserved = if templates.is_empty() { 95.0 } else { 255.0 };
        egui::ScrollArea::vertical()
            .id_salt("cheat_list")
            .auto_shrink([false, true])
            .max_height((ui.available_height() - reserved).max(120.0))
            .show(ui, |ui| {
                let gap = ui.spacing().item_spacing.x;
                let name_width = ui.available_width()
                    - (CHECK_WIDTH
                        + CODE_WIDTH
                        + TYPE_WIDTH
                        + WARNING_WIDTH
                        + BUTTONS_WIDTH
                        + 5.0 * gap);
                let mut save_edit = false;
                for i in 0..gb.cheats().list().len() {
                    // The cheat being edited: its code and name as fields.
                    if let Some((at, code, name)) = self.cheat_edit.as_mut()
                        && *at == i
                    {
                        ui.horizontal(|ui| {
                            fixed(ui, CHECK_WIDTH, |_| {});
                            let edit = ui.add(
                                egui::TextEdit::singleline(code)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(CODE_WIDTH + TYPE_WIDTH + WARNING_WIDTH),
                            );
                            let edit_name =
                                ui.add(egui::TextEdit::singleline(name).desired_width(name_width));
                            let enter = (edit.lost_focus() || edit_name.lost_focus())
                                && ui.input(|i| i.key_pressed(Key::Enter));
                            if ui.small_button("✔").on_hover_text("Save (Enter)").clicked() || enter
                            {
                                save_edit = true;
                            }
                            if ui.small_button("✖").on_hover_text("Cancel").clicked() {
                                cancel_edit = true;
                            }
                        });
                        continue;
                    }
                    ui.horizontal(|ui| {
                        fixed(ui, CHECK_WIDTH, |ui| {
                            let mut enabled = gb.cheats().list()[i].enabled;
                            if ui.checkbox(&mut enabled, "").changed() {
                                gb.cheats_mut().set_enabled(i, enabled);
                            }
                        });
                        let cheat = &gb.cheats().list()[i];
                        cell(ui, CODE_WIDTH, &cheat.text, true);
                        kind_cell(ui, cheat.kind(), false);
                        let fits = gb.cheat_fits(cheat);
                        fixed(ui, WARNING_WIDTH, |ui| {
                            if !fits {
                                ui.colored_label(ui.visuals().warn_fg_color, "⚠")
                                    .on_hover_text(
                                        "Made for another version of the game: this ROM \
                                         does not have the bytes its Game Genie code \
                                         expects, so it does nothing",
                                    );
                            }
                        });
                        cell(ui, name_width, &cheat.name, false);
                        fixed(ui, BUTTONS_WIDTH, |ui| {
                            if ui.small_button("✎").on_hover_text("Edit").clicked() {
                                start_edit = Some(i);
                            }
                            if ui.small_button("✖").on_hover_text("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                }
                if save_edit && let Some((i, code, name)) = self.cheat_edit.clone() {
                    match gb.cheats_mut().edit(i, &code, &name) {
                        Ok(()) => {
                            self.cheat_edit = None;
                            self.cheat_error = None;
                        }
                        Err(e) => self.cheat_error = Some(e.to_string()),
                    }
                }
                // Entries of the file freki cannot use: shown, greyed out, with the reason.
                for entry in gb.cheats().unsupported() {
                    ui.horizontal(|ui| {
                        fixed(ui, CHECK_WIDTH, |ui| {
                            ui.add_enabled(false, egui::Checkbox::without_text(&mut false));
                        });
                        ui.add_enabled_ui(false, |ui| cell(ui, CODE_WIDTH, &entry.code, true));
                        kind_cell(ui, entry.kind, true);
                        fixed(ui, WARNING_WIDTH, |ui| {
                            ui.colored_label(ui.visuals().warn_fg_color, "⚠")
                                .on_hover_text(format!(
                                    "Not supported, so it does nothing: {}. It stays in \
                                     the cheat file.",
                                    entry.reason
                                ));
                        });
                        ui.add_enabled_ui(false, |ui| cell(ui, name_width, &entry.name, false));
                    });
                }
            });
        if cancel_edit {
            self.cheat_edit = None;
            self.cheat_error = None;
        }
        if let Some(i) = start_edit {
            let cheat = &gb.cheats().list()[i];
            self.cheat_edit = Some((i, cheat.text.clone(), cheat.name.clone()));
            self.cheat_error = None;
        }
        if let Some(i) = remove {
            gb.cheats_mut().remove(i);
            self.cheat_edit = None;
        }
        if gb.cheats().list().is_empty() {
            ui.label("No cheats yet.");
        }
        if !templates.is_empty() {
            ui.separator();
            ui.label(
                "Templates: enter a value (decimal, or hex with $; $99 shows as 99 in games \
                 that count in BCD) to add a cheat",
            );
            self.template_values.resize(templates.len(), String::new());
            egui::ScrollArea::vertical()
                .id_salt("cheat_templates")
                .auto_shrink([false, true])
                .max_height(120.0)
                .show(ui, |ui| {
                    let gap = ui.spacing().item_spacing.x;
                    let name_width = ui.available_width()
                        - (CODE_WIDTH + VALUE_WIDTH + ADD_WIDTH + BUTTON_WIDTH + 4.0 * gap);
                    for (t, template) in templates.iter().enumerate() {
                        ui.horizontal(|ui| {
                            cell(ui, CODE_WIDTH, &template.code, true);
                            cell(ui, name_width, &template.name, false);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.template_values[t])
                                    .hint_text(format!("0–{}", template.max()))
                                    .desired_width(VALUE_WIDTH - 8.0),
                            );
                            if ui
                                .add_sized([ADD_WIDTH, 18.0], egui::Button::new("Add"))
                                .clicked()
                            {
                                let value = parse_byte(&self.template_values[t])
                                    .ok_or("not a number from 0 to 255".to_string())
                                    .and_then(|v| {
                                        gb.cheats_mut()
                                            .fill_template_as(t, v, &self.template_values[t])
                                            .map_err(|e| e.to_string())
                                    });
                                match value {
                                    Ok(_) => {
                                        self.template_values[t].clear();
                                        self.cheat_error = None;
                                    }
                                    Err(e) => self.cheat_error = Some(e),
                                }
                            }
                            if ui
                                .small_button("✖")
                                .on_hover_text("Remove template")
                                .clicked()
                            {
                                remove_template = Some(t);
                            }
                        });
                    }
                });
        }
        if let Some(t) = remove_template {
            gb.cheats_mut().remove_template(t);
            self.template_values.clear();
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Code");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_code)
                    .hint_text("01VVLLHH, ABC-DEF-GHI, $0DVVLLHH; ?? = value")
                    .desired_width(130.0),
            );
            ui.label("Name");
            let gap = ui.spacing().item_spacing.x;
            let width = (ui.available_width() - ADD_WIDTH - gap - 8.0).max(60.0);
            ui.add(egui::TextEdit::singleline(&mut self.new_name).desired_width(width));
            if ui
                .add_sized([ADD_WIDTH, 18.0], egui::Button::new("Add"))
                .clicked()
            {
                match gb
                    .cheats_mut()
                    .add_or_template(&self.new_code, &self.new_name)
                {
                    Ok(added) => {
                        if added == freki_core::Added::Template {
                            status =
                                Some("template added: enter a value for it under Templates".into());
                        }
                        self.new_code.clear();
                        self.new_name.clear();
                        self.cheat_error = None;
                    }
                    Err(e) => self.cheat_error = Some(e.to_string()),
                }
            }
        });
        if let Some(err) = &self.cheat_error {
            ui.colored_label(Color32::LIGHT_RED, err);
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.checkbox(&mut gb.cheats_mut().enabled, "All cheats enabled (F9)");
            let path = session_cheat_path(&session.rom_path);
            if ui
                .button("Reload .cht")
                .on_hover_text(format!(
                    "Changes are saved to {} automatically",
                    file_name(&path)
                ))
                .clicked()
            {
                status = Some(
                    console::load_cheat_file(&path, gb)
                        .unwrap_or_else(|| format!("{} not found", file_name(&path))),
                );
            }
        });
        status
    }

    /// The memory search, in a window of its own.
    fn search_window(&mut self, ctx: &egui::Context) {
        if !self.show_search || self.session.is_none() {
            return;
        }
        let mut status = None;
        let open = tool_window(
            ctx,
            "search",
            "freki — Memory search",
            [480.0, 480.0],
            |ui| {
                status = self.search_contents(ui);
            },
        );
        self.show_search = open;
        if let Some(message) = status {
            self.set_status(message);
        }
    }

    /// The memory search's contents. Returns a status message, if any.
    fn search_contents(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let session = self.session.as_mut()?;
        let ui_state = &mut self.search_ui;
        let mut status = None;
        {
            let gb = &mut session.gb;
            ui.label("Find the address of a value (lives, money, …): start, change the value in the game, then filter.");
            ui.horizontal(|ui| {
                if ui.button("Start new search").clicked() {
                    ui_state.search = Some(gb.start_memory_search());
                    ui_state.selected = None;
                }
                if let Some(s) = &ui_state.search {
                    ui.label(format!("{} candidate(s)", s.len()));
                }
            });
            if let Some(search) = ui_state.search.as_mut() {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Value");
                    ui.add(egui::TextEdit::singleline(&mut ui_state.value).desired_width(50.0));
                    ui.checkbox(&mut ui_state.bcd, "BCD").on_hover_text(
                        "For values the game keeps in BCD, one decimal digit per hex digit \
                         (99 lives = $99; then 100 shows as 64): search for and hold the \
                         number the game shows. $ still means a raw byte.",
                    );
                    let value = parse_value(&ui_state.value, ui_state.bcd);
                    let mut apply = |filter| {
                        search.filter(filter, |l| gb.peek_location(l));
                    };
                    if ui
                        .add_enabled(value.is_some(), egui::Button::new("="))
                        .clicked()
                    {
                        apply(Filter::Equal(value.unwrap_or(0)));
                    }
                    if ui
                        .add_enabled(value.is_some(), egui::Button::new("≠"))
                        .clicked()
                    {
                        apply(Filter::NotEqual(value.unwrap_or(0)));
                    }
                    ui.separator();
                    if ui.button("changed").clicked() {
                        apply(Filter::Changed);
                    }
                    if ui.button("unchanged").clicked() {
                        apply(Filter::Unchanged);
                    }
                    if ui.button("increased").clicked() {
                        apply(Filter::Increased);
                    }
                    if ui.button("decreased").clicked() {
                        apply(Filter::Decreased);
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .max_height((ui.available_height() - 60.0).max(120.0))
                    .show(ui, |ui| {
                        for (i, &(location, _)) in search.candidates().iter().take(200).enumerate()
                        {
                            let v = gb.peek_location(location);
                            let shown = if ui_state.bcd {
                                from_bcd(v).map_or("--".to_string(), |n| n.to_string())
                            } else {
                                v.to_string()
                            };
                            let text = format!("{location:>7} = {shown:>3} (${v:02X})");
                            if ui
                                .selectable_label(
                                    ui_state.selected == Some(i),
                                    egui::RichText::new(text).monospace(),
                                )
                                .clicked()
                            {
                                ui_state.selected = Some(i);
                                ui_state.cheat_value = match (ui_state.bcd, from_bcd(v)) {
                                    (true, Some(n)) => n.to_string(),
                                    (true, None) => format!("${v:02X}"),
                                    (false, _) => v.to_string(),
                                };
                            }
                        }
                        if search.len() > 200 {
                            ui.label(format!("… {} more", search.len() - 200));
                        }
                    });
                if let Some(&(location, _)) =
                    ui_state.selected.and_then(|i| search.candidates().get(i))
                {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(format!("{location}: hold at"));
                        ui.add(
                            egui::TextEdit::singleline(&mut ui_state.cheat_value)
                                .desired_width(40.0),
                        );
                        ui.label("name");
                        ui.add(
                            egui::TextEdit::singleline(&mut ui_state.cheat_name)
                                .desired_width(120.0),
                        );
                        let value = parse_value(&ui_state.cheat_value, ui_state.bcd);
                        if ui
                            .add_enabled(value.is_some(), egui::Button::new("Add cheat"))
                            .clicked()
                        {
                            let value = value.unwrap_or(0);
                            let code = Code::gameshark(location, value);
                            let name = if ui_state.cheat_name.is_empty() {
                                format!("{location} = {}", ui_state.cheat_value.trim())
                            } else {
                                ui_state.cheat_name.clone()
                            };
                            status = Some(match gb.cheats_mut().add(&code, &name) {
                                Ok(_) => format!("added cheat {code} {name}"),
                                Err(e) => e.to_string(),
                            });
                        }
                    });
                }
            }
        }
        status
    }

    /// While binding a gamepad button: takes the next button pressed.
    fn capture_pad(&mut self) {
        if let Some((button, Capture::Pad)) = self.capture {
            self.gamepads.update();
            if let Some(pad) = self.gamepads.take_pressed() {
                if input::bindable_pad(pad) {
                    self.settings.bindings.set_pad(button, pad);
                    self.controls_message = None;
                } else {
                    self.controls_message = Some("that button cannot be used".into());
                }
                self.capture = None;
            }
        }
    }

    fn controls_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_controls;
        egui::Window::new("Controls")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| self.controls_contents(ui));
        if !open {
            self.capture = None;
        }
        self.show_controls = open;
    }

    /// The Controls window: a console with each button's binding beside it, for the
    /// keyboard or the gamepad, and the emulator's own shortcuts.
    fn controls_contents(&mut self, ui: &mut egui::Ui) {
        let kind = match self.controls_tab {
            ControlsTab::Keyboard => Capture::Key,
            ControlsTab::Gamepad => Capture::Pad,
        };
        let connected = self.gamepads.connected();
        ui.horizontal(|ui| {
            for (tab, name) in [
                (ControlsTab::Keyboard, "Keyboard"),
                (ControlsTab::Gamepad, "Gamepad"),
            ] {
                if ui
                    .selectable_label(self.controls_tab == tab, name)
                    .clicked()
                    && self.controls_tab != tab
                {
                    self.controls_tab = tab;
                    self.capture = None;
                    self.controls_message = None;
                }
            }
            ui.separator();
            match (self.capture, &self.controls_message) {
                (Some((button, _)), _) => {
                    let what = if kind == Capture::Key {
                        "key"
                    } else {
                        "button"
                    };
                    ui.colored_label(
                        controls::AMBER,
                        format!(
                            "Press the {what} for {} … (Esc cancels)",
                            input::control_name(button)
                        ),
                    );
                }
                (None, Some(message)) => {
                    ui.colored_label(Color32::LIGHT_RED, message);
                }
                (None, None) => {
                    let what = if kind == Capture::Key {
                        "key"
                    } else {
                        "button"
                    };
                    ui.weak(format!(
                        "Click a binding, then press the new {what}. Esc cancels."
                    ));
                }
            }
        });
        if kind == Capture::Pad {
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut self.settings.gamepads, "Use gamepads")
                    .on_hover_text(GAMEPADS_HINT)
                    .changed()
                {
                    self.gamepads.set_on(self.settings.gamepads);
                }
                if cfg!(target_os = "macos") && !self.settings.gamepads {
                    ui.weak("macOS will ask for Input Monitoring permission.");
                }
            });
            match &connected {
                Some((name, _, rumble)) => {
                    let text = if *rumble {
                        format!("{name} · rumble")
                    } else {
                        name.clone()
                    };
                    ui.colored_label(Color32::from_rgb(0xB9, 0xD9, 0xB9), format!("● {text}"));
                }
                None if !self.settings.gamepads => {
                    ui.weak("Gamepads are off: buttons are named by position.");
                }
                None => {
                    ui.weak("No controller connected: buttons are named by position.");
                }
            }
        }
        ui.add_space(4.0);

        let pad_kind = connected.map_or(controls::ControllerKind::Generic, |(_, kind, _)| kind);
        let dmg = self
            .session
            .as_ref()
            .is_some_and(|s| s.gb.model() == freki_core::Model::Dmg);
        // The picture is laid out at 800×500 points and shrinks to fit the window.
        let [width, height] = controls::layout::SIZE;
        let k = (ui.ctx().content_rect().width() - 40.0).clamp(width * 0.5, width) / width;
        let (area, _) = ui.allocate_exact_size(vec2(width, height) * k, Sense::hover());
        let origin = area.min;
        let map = |p: Pos2| origin + p.to_vec2() * k;
        let painter = ui.painter_at(area);
        let line = Stroke::new(1.2, Color32::from_rgb(0x5C, 0x5C, 0x66));
        let waiting_control = self.capture.filter(|(_, k)| *k == kind).map(|(c, _)| c);
        let waiting_for = match waiting_control {
            Some(input::Control::Button(button)) => Some(button),
            _ => None,
        };
        for button in input::BUTTONS {
            let points: Vec<Pos2> = controls::layout::leader(button)
                .into_iter()
                .map(map)
                .collect();
            let waiting = waiting_for == Some(button);
            let stroke = if waiting {
                Stroke::new(2.0, controls::AMBER)
            } else {
                line
            };
            painter.add(egui::Shape::line(points.clone(), stroke));
            let dot = if waiting {
                controls::AMBER
            } else {
                Color32::from_rgb(0x7B, 0x9C, 0xC9)
            };
            painter.circle_filled(*points.last().unwrap(), 3.0 * k, dot);
        }
        controls::draw_console(&painter, origin, k, dmg, waiting_for);

        let font = egui::FontId::proportional(13.0 * k);
        for button in input::BUTTONS {
            let (chip, label_pos, label_align) = controls::layout::chip(button);
            let chip = Rect::from_min_max(map(chip.min), map(chip.max));
            let waiting = waiting_for == Some(button);
            let label_color = if waiting {
                controls::AMBER
            } else {
                Color32::from_gray(0xA8)
            };
            painter.text(
                map(label_pos),
                label_align,
                input::button_name(button),
                egui::FontId::proportional(12.0 * k),
                label_color,
            );
            let response = ui.interact(
                chip,
                ui.scope_id().with(("binding", button as u8)),
                Sense::click(),
            );
            let fill = if waiting {
                Color32::from_rgb(0x3A, 0x32, 0x20)
            } else if response.hovered() {
                Color32::from_rgb(0x38, 0x38, 0x3F)
            } else {
                Color32::from_rgb(0x2C, 0x2C, 0x31)
            };
            let border = if waiting {
                controls::AMBER
            } else {
                Color32::from_rgb(0x4A, 0x4A, 0x52)
            };
            let text_color = if waiting {
                controls::AMBER
            } else {
                Color32::from_gray(0xF0)
            };
            let radius = |v: f32| (v * k).round() as u8;
            if kind == Capture::Key {
                // A keycap: a thicker bottom edge.
                painter.rect_filled(chip, radius(5.0), border);
                painter.rect(
                    chip.with_max_y(chip.max.y - 3.0 * k),
                    radius(5.0),
                    fill,
                    Stroke::new(1.0, border),
                    StrokeKind::Inside,
                );
                let text = if waiting {
                    "press a key…".to_string()
                } else {
                    key_label(self.settings.bindings.key(button))
                };
                painter.text(
                    chip.center() - vec2(0.0, 1.5 * k),
                    Align2::CENTER_CENTER,
                    text,
                    font.clone(),
                    text_color,
                );
            } else {
                painter.rect(
                    chip,
                    radius(14.0),
                    fill,
                    Stroke::new(1.0, border),
                    StrokeKind::Inside,
                );
                if waiting {
                    painter.text(
                        chip.center(),
                        Align2::CENTER_CENTER,
                        "press a button…",
                        font.clone(),
                        text_color,
                    );
                } else {
                    let (glyph, name) =
                        controls::pad_label(pad_kind, self.settings.bindings.pad(button));
                    let size = 18.0 * k;
                    let glyph_w = controls::glyph_width(&painter, size, &glyph);
                    let text_w = painter
                        .layout_no_wrap(name.clone(), font.clone(), text_color)
                        .size()
                        .x;
                    let gap = if glyph_w > 0.0 { 7.0 * k } else { 0.0 };
                    let left = chip.center().x - (glyph_w + gap + text_w) / 2.0;
                    controls::draw_glyph(
                        &painter,
                        pos2(left + glyph_w / 2.0, chip.center().y),
                        size,
                        &glyph,
                    );
                    painter.text(
                        pos2(left + glyph_w + gap, chip.center().y),
                        Align2::LEFT_CENTER,
                        name,
                        font.clone(),
                        text_color,
                    );
                }
            }
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.capture = if waiting {
                    None
                } else {
                    Some((button.into(), kind))
                };
                self.controls_message = None;
                // Forget presses made before the capture started.
                self.gamepads.update();
                self.gamepads.take_pressed();
            }
        }

        for (row, note) in [
            (input::TURBOS, "held: presses the button 15 times a second"),
            (
                input::SHOULDERS,
                "the SNES joypad's L and R: both open the SGB's menu (SGB as hardware)",
            ),
        ] {
            ui.horizontal(|ui| {
                for control in row {
                    let waiting = waiting_control == Some(control);
                    ui.label(input::control_name(control));
                    let text = if waiting {
                        if kind == Capture::Key {
                            "press a key…".to_string()
                        } else {
                            "press a button…".to_string()
                        }
                    } else if kind == Capture::Key {
                        key_label(self.settings.bindings.key(control))
                    } else {
                        let (_, name) =
                            controls::pad_label(pad_kind, self.settings.bindings.pad(control));
                        name
                    };
                    let mut chip = egui::Button::new(text).min_size(vec2(90.0, 0.0));
                    if waiting {
                        chip = chip.stroke(Stroke::new(1.5, controls::AMBER));
                    }
                    if ui.add(chip).clicked() {
                        self.capture = if waiting { None } else { Some((control, kind)) };
                        self.controls_message = None;
                        self.gamepads.update();
                        self.gamepads.take_pressed();
                    }
                    ui.add_space(12.0);
                }
                ui.weak(note);
            });
        }
        ui.separator();
        ui.horizontal(|ui| {
            let (default, label) = match kind {
                Capture::Key => (
                    self.settings.bindings.keys_default(),
                    "Reset keyboard to defaults",
                ),
                Capture::Pad => (
                    self.settings.bindings.pads_default(),
                    "Reset gamepad to defaults",
                ),
            };
            if ui.add_enabled(!default, egui::Button::new(label)).clicked() {
                match kind {
                    Capture::Key => self.settings.bindings.reset_keys(),
                    Capture::Pad => self.settings.bindings.reset_pads(),
                }
                self.capture = None;
            }
            if kind == Capture::Pad {
                ui.weak("The left stick also steers.");
            }
        });
        egui::CollapsingHeader::new("Emulator shortcuts").show(ui, |ui| {
            egui::Grid::new("controls").striped(true).show(ui, |ui| {
                for (what, keys) in [
                    ("Save / load state", "F5 / F8"),
                    ("Previous / next slot", "F6 / F7"),
                    ("Rewind", "hold R"),
                    ("Fast-forward", "hold Tab"),
                    ("Pause", "P"),
                    ("Mute", "M"),
                    ("Cheats on/off", "F9"),
                    ("Screenshot", "F12"),
                    ("Fullscreen", "Alt+Enter (Esc leaves)"),
                    ("Open ROM / reset / quit", "Ctrl+O / Ctrl+R / Ctrl+Q"),
                ] {
                    ui.label(what);
                    ui.label(keys);
                    ui.end_row();
                }
            });
            ui.label(
                "The function keys, P and the Ctrl shortcuts also work while a tool window \
                 (debugger, viewers, cheats, printer) has focus.",
            );
            ui.label("The terminal freki was started from also accepts commands (type help).");
        });
    }

    fn info_windows(&mut self, ctx: &egui::Context) {
        self.capture_pad();
        self.controls_window(ctx);
        egui::Window::new("About freki")
            .open(&mut self.show_about)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading(format!("freki {}", env!("CARGO_PKG_VERSION")));
                ui.label("A Game Boy and Game Boy Color emulator written in Rust.");
                ui.label("Named after Freki, one of Odin's two wolves.");
                ui.hyperlink("https://github.com/engrin/freki");
                ui.label("MIT license");
            });
    }
}

fn session_cheat_path(rom_path: &Path) -> PathBuf {
    rom_path.with_extension("cht")
}

/// A value typed into the memory search: like [`parse_byte`], or with `bcd` a decimal
/// 0–99 as the game shows it, stored as BCD (99 → $99). `$`/`0x` always mean a raw byte.
fn parse_value(text: &str, bcd: bool) -> Option<u8> {
    let text = text.trim();
    if bcd && !text.starts_with('$') && !text.starts_with("0x") {
        return to_bcd(text.parse().ok()?);
    }
    parse_byte(text)
}

/// 0–99 as BCD: tens in the high nibble, ones in the low one.
fn to_bcd(n: u8) -> Option<u8> {
    (n <= 99).then_some((n / 10) << 4 | (n % 10))
}

/// A BCD byte as its number, if both nibbles are decimal digits.
fn from_bcd(b: u8) -> Option<u8> {
    let (tens, ones) = (b >> 4, b & 0x0F);
    (tens <= 9 && ones <= 9).then_some(tens * 10 + ones)
}

/// Parses a byte: decimal, or hex with `$` or `0x`.
fn parse_byte(s: &str) -> Option<u8> {
    let s = s.trim();
    match s.strip_prefix('$').or_else(|| s.strip_prefix("0x")) {
        Some(hex) => u8::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}

impl eframe::App for FrekiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // A ROM dropped onto the window is opened.
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.open_rom(&ctx, &path);
        }
        self.handle_hotkeys(&ctx);
        self.console.poll(self.session.as_mut().map(|s| &mut s.gb));
        if let Some(paused) = self.console.take_pause_request() {
            self.set_paused(paused);
        }
        self.emulate(&ctx);
        self.collect_printouts();
        self.poll_recordings(false);
        // Changes while a game uses its own settings are its own.
        if let (Some(key), Some(_)) = (&self.game_key, &self.general_settings) {
            let now = GameSettings::of(&self.settings);
            if self.settings.games.get(key) != Some(&now) {
                self.settings.games.insert(key.clone(), now);
            }
        }
        self.write_changed_cheats();
        self.update_rumble();
        self.update_texture(&ctx);

        // Fullscreen shows just the game (Esc or Alt+Enter leaves it).
        if !is_fullscreen(&ctx) {
            egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
            egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(ui, |ui| self.screen(ui));
        self.cheat_window(&ctx);
        self.search_window(&ctx);
        self.info_windows(&ctx);
        if self.cpu_view.open
            && let Some(session) = self.session.as_mut()
            && let Some(action) = self.cpu_view.show(&ctx, &mut session.gb, self.paused)
        {
            self.debugger_action(action);
        }
        if self.printer_view.open {
            let printer = self.session.as_ref().and_then(|s| s.gb.printer());
            if self.printer_view.show(&ctx, printer)
                && let Some(printer) = self.session.as_mut().and_then(|s| s.gb.printer_mut())
            {
                printer.tear_off();
                self.collect_printouts();
            }
        }
        if let Some(session) = &self.session {
            let palettes = debug::Palettes::new(
                &session.gb,
                self.settings.palette.colors(),
                self.settings.color_correction,
            );
            self.debug.show(&ctx, &session.gb, &palettes);
            if self.sound_view.open {
                self.sound_view.show(&ctx, &session.gb);
            }
        }

        if self.background_paused {
            // Nothing moves; focus coming back wakes the window at once anyway.
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            ctx.request_repaint();
        }
    }

    /// Takes Tab away from egui (which would move keyboard focus through the menus on
    /// every key repeat) and uses it as the fast-forward key, except while typing. While a
    /// key is being bound in the controls window, the next key press goes there.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some((button, Capture::Key)) = self.capture {
            let pressed = raw_input.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key, pressed: true, ..
                } => Some(*key),
                _ => None,
            });
            if let Some(key) = pressed {
                self.capture = None;
                if key != Key::Escape {
                    match input::reserved(key) {
                        Some(what) => {
                            self.controls_message =
                                Some(format!("{} is used for {what}", key.name()));
                        }
                        None => {
                            self.settings.bindings.set_key(button, key);
                            self.controls_message = None;
                        }
                    }
                }
                raw_input
                    .events
                    .retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_)));
            }
            return;
        }
        if ctx.text_edit_focused() {
            return;
        }
        raw_input.events.retain(|event| match event {
            egui::Event::Key {
                key: Key::Tab,
                pressed,
                ..
            } => {
                self.fast_forward_held = *pressed;
                false
            }
            egui::Event::WindowFocused(false) => {
                self.fast_forward_held = false;
                true
            }
            _ => true,
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // Saved with your general settings, not those of the game being played.
        let now = GameSettings::of(&self.settings);
        if let Some(general) = self.general_settings {
            general.apply(&mut self.settings);
        }
        self.settings.save(storage);
        now.apply(&mut self.settings);
    }

    fn on_exit(&mut self) {
        self.close_rom();
        // Let videos being written finish their files.
        self.poll_recordings(true);
    }
}

/// Width of the code column in the cheat manager.
const CODE_WIDTH: f32 = 190.0;
/// Width of the ⚠ column in the cheat manager.
const WARNING_WIDTH: f32 = 16.0;
/// Width of the cheat type column ("GameShark", "Game Genie", "Xploder", "GS + GG").
const TYPE_WIDTH: f32 = 72.0;
/// Widths of the checkbox and remove-button columns, the template value field and the
/// Add buttons in the cheat manager.
const CHECK_WIDTH: f32 = 20.0;
const BUTTON_WIDTH: f32 = 22.0;
/// Room for the edit and remove buttons of a cheat.
const BUTTONS_WIDTH: f32 = 2.0 * BUTTON_WIDTH + 6.0;
const VALUE_WIDTH: f32 = 64.0;
const ADD_WIDTH: f32 = 40.0;

/// A cell exactly `width` wide holding `contents`.
fn fixed(ui: &mut egui::Ui, width: f32, contents: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        vec2(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_width(width);
            contents(ui);
        },
    );
}

/// The type column of the cheat list; `unsupported` ones in the warning colour.
fn kind_cell(ui: &mut egui::Ui, kind: &str, unsupported: bool) {
    // Mixed kinds are abbreviated: "GS + GG", "GS + XP".
    let short = if kind.contains('+') {
        kind.replace("GameShark", "GS")
            .replace("Game Genie", "GG")
            .replace("Xploder", "XP")
    } else {
        kind.to_string()
    };
    fixed(ui, TYPE_WIDTH, |ui| {
        let text = egui::RichText::new(short).small();
        let text = if unsupported {
            text.color(ui.visuals().warn_fg_color)
        } else {
            text.weak()
        };
        ui.add(egui::Label::new(text).truncate())
            .on_hover_text(kind);
    });
}

/// A table cell `width` wide: `text` cut off with "…" when longer, shown whole on hover.
fn cell(ui: &mut egui::Ui, width: f32, text: &str, monospace: bool) {
    let text = if monospace {
        egui::RichText::new(text).monospace()
    } else {
        egui::RichText::new(text)
    };
    fixed(ui, width.max(20.0), |ui| {
        ui.add(egui::Label::new(text.clone()).truncate())
            .on_hover_text(text);
    });
}

/// The picture of `session` (and of the linked game to its right), as RGBA with its size.
fn picture(
    session: &Session,
    link: Option<&LinkedGame>,
    palette: Palette,
    correct: bool,
) -> (Vec<u8>, [usize; 2]) {
    let shot = |s: &Session| settings::screen_rgba(s.gb.screen(), palette, correct);
    match link {
        Some(two) => settings::side_by_side(shot(session), shot(&two.session)),
        None => shot(session),
    }
}

/// Why emulation stopped. A game that crashes while cheats are on was most likely
/// crashed by one of them (a code for another version of the game, or a broken one).
fn stopped_message(error: &dyn std::fmt::Display, cheats_on: bool) -> String {
    if cheats_on {
        format!(
            "emulation stopped: {error}. Cheats are on and may have caused it: switch them \
             off (F9) and reset (Ctrl+R), then switch them on one at a time"
        )
    } else {
        format!("emulation stopped: {error}")
    }
}

fn is_fullscreen(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
}

fn set_fullscreen(ctx: &egui::Context, on: bool) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bcd_values_as_games_show_them() {
        assert_eq!(to_bcd(99), Some(0x99));
        assert_eq!(to_bcd(100), None);
        assert_eq!(
            from_bcd(0x64),
            Some(64),
            "100 lives written plainly show as 64"
        );
        assert_eq!(from_bcd(0x6A), None);
        assert_eq!(parse_value("99", true), Some(0x99));
        assert_eq!(parse_value("$63", true), Some(0x63), "$ stays a raw byte");
        assert_eq!(parse_value("100", true), None);
        assert_eq!(parse_value("100", false), Some(100));
    }

    #[test]
    fn parses_bytes() {
        assert_eq!(parse_byte("42"), Some(42));
        assert_eq!(parse_byte("$2A"), Some(42));
        assert_eq!(parse_byte("0x2a"), Some(42));
        assert_eq!(parse_byte("256"), None);
        assert_eq!(parse_byte("x"), None);
    }
}
