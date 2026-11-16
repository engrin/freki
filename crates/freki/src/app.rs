//! The egui application: menu bar, screen, status bar and tool windows.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Color32, ColorImage, Key, KeyboardShortcut, Modifiers, Rect, TextureHandle,
    TextureOptions, pos2, vec2,
};
use freki_core::{
    Button, Code, FRAMES_PER_SECOND, Filter, MemorySearch, Model, SCREEN_HEIGHT, SCREEN_WIDTH,
    Screen,
};

use crate::audio::Audio;
use crate::console::{self, Console};
use crate::cpu_view::{self, CpuView};
use crate::debug::{self, DebugViews};
use crate::gamepad::Gamepads;
use crate::input;
use crate::session::Start;
use crate::session::{Session, file_name};
use crate::settings::{self, ConsoleModel, FAST_FORWARD_SPEEDS, Palette, Settings};
use crate::states::SLOTS;

const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);
const RESET: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::R);

/// Audio kept queued ahead of playback. Video runs at the Game Boy's own frame rate;
/// audio stays in sync by nudging the output sample rate towards this buffer level.
const AUDIO_LATENCY: Duration = Duration::from_millis(60);
/// Largest audio rate correction (0.5 %: inaudible).
const MAX_RATE_ADJUST: f64 = 0.005;
/// Never emulate more than this many frames in one UI update (e.g. after a stall).
const MAX_FRAMES_PER_UPDATE: u32 = 4;
/// How long a status message stays in the status bar.
const STATUS_DURATION: Duration = Duration::from_secs(3);

/// What the controls window is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capture {
    Key,
    Pad,
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
    value: String,
    cheat_value: String,
    cheat_name: String,
    selected: Option<usize>,
}

pub struct FrekiApp {
    session: Option<Session>,
    settings: Settings,
    audio: Option<Audio>,
    gamepads: Gamepads,
    console: Console,
    texture: Option<TextureHandle>,
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
    capture: Option<(Button, Capture)>,
    controls_message: Option<String>,
    show_about: bool,
    new_code: String,
    new_name: String,
    cheat_error: Option<String>,
    search_ui: SearchUi,
}

impl FrekiApp {
    pub fn new(cc: &eframe::CreationContext<'_>, rom: Option<PathBuf>) -> Self {
        let audio = match Audio::open() {
            Ok(audio) => Some(audio),
            Err(err) => {
                eprintln!("freki: no sound ({err})");
                None
            }
        };
        let mut app = Self {
            session: None,
            settings: Settings::load(cc.storage),
            audio,
            gamepads: Gamepads::open(),
            console: Console::start(),
            texture: None,
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
            controls_message: None,
            show_about: false,
            new_code: String::new(),
            new_name: String::new(),
            cheat_error: None,
            search_ui: SearchUi::default(),
        };
        println!("freki: type `help` here for cheat and memory commands");
        if let Some(path) = rom {
            app.open_rom(&cc.egui_ctx, &path);
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
        let start = self.start();
        match Session::open(path, start) {
            Ok((mut session, message)) => {
                if let Some(audio) = &self.audio {
                    session.gb.set_audio_sample_rate(audio.sample_rate());
                }
                let cheats = console::load_cheat_file(&session.cheat_file(), &mut session.gb);
                self.console.set_cheat_file(session.cheat_file());
                let model = match session.gb.model() {
                    Model::Cgb => " (Game Boy Color)",
                    Model::Dmg => "",
                };
                let title = if session.title.is_empty() {
                    format!("freki{model}")
                } else {
                    format!("freki — {}{model}", session.title)
                };
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
                self.session = Some(session);
                self.settings.add_recent(path);
                self.search_ui = SearchUi::default();
                self.paused = false;
                self.set_status(message);
                if let Some(msg) = cheats {
                    self.set_status(msg);
                }
            }
            Err(e) => {
                if !path.exists() {
                    self.settings.remove_recent(path);
                }
                self.set_status(format!("cannot open {}: {e}", file_name(path)));
            }
        }
    }

    fn close_rom(&mut self) {
        if let Some(mut session) = self.session.take()
            && let Err(e) = session.write_save()
        {
            self.set_status(format!("save failed: {e}"));
        }
    }

    fn pick_rom(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Open Game Boy ROM")
            .add_filter("Game Boy ROM", &["gb", "gbc", "bin"]);
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
        let size = vec2(
            (SCREEN_WIDTH as u32 * scale) as f32,
            (SCREEN_HEIGHT as u32 * scale) as f32 + 56.0,
        );
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }

    // ---- input and emulation ---------------------------------------------------------

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_shortcut(&OPEN)) {
            self.pick_rom(ctx);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&QUIT)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&RESET)) {
            self.with_session(|s| s.reset().map(|()| "reset".to_string()));
        }
        // Single-key hotkeys don't fire while typing into a text field.
        if ctx.text_edit_focused() {
            return;
        }
        let pressed = |key| ctx.input(|i| i.key_pressed(key));
        if pressed(Key::F5) {
            self.with_session(|s| s.save_state());
        }
        if pressed(Key::F8) {
            self.with_session(|s| s.load_state());
        }
        if pressed(Key::F6) || pressed(Key::F7) {
            let next = pressed(Key::F7);
            self.with_session(|s| {
                if next {
                    s.slots.next();
                } else {
                    s.slots.previous();
                }
                Ok(format!("slot {}", s.slots.slot()))
            });
        }
        if pressed(Key::F9) {
            self.toggle_cheats();
        }
        if pressed(Key::F12) {
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
        self.rewinding = ctx.input(|i| i.key_down(Key::R)) && self.session.is_some();
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
            Err(e) => self.set_status(format!("emulation stopped: {e}")),
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
        self.frame_debt = (self.frame_debt + elapsed * FRAMES_PER_SECOND * speed as f64)
            .min((MAX_FRAMES_PER_UPDATE * speed) as f64);
        let frames = self.frame_debt.floor() as u32;
        self.frame_debt -= frames as f64;

        let typing = ctx.text_edit_focused();
        let Some(session) = self.session.as_mut() else {
            return;
        };

        if self.rewinding {
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
            session
                .gb
                .adjust_audio_sample_rate((rate * (1.0 + adjust)).round() as u32);
            self.timing.audio_ms = queued / rate * 1000.0;
            self.timing.rate_adjust = adjust;
        }

        self.gamepads.update();
        let bindings = &self.settings.bindings;
        for _ in 0..frames {
            for button in input::BUTTONS {
                let down = !typing && ctx.input(|i| i.key_down(bindings.key(button)));
                let pad = self.gamepads.is_pressed(button, bindings.pad(button));
                session.gb.set_button(button, down || pad);
            }
            match session.run_frame() {
                Ok(None) => {}
                Ok(Some(stop)) => {
                    // Breakpoint or watchpoint: pause and show the debugger.
                    self.paused = true;
                    let msg = console::describe_stop(&stop);
                    eprintln!("freki: {msg}");
                    self.status = Some((msg, Instant::now()));
                    self.cpu_view.last_stop = Some(stop);
                    self.cpu_view.open = true;
                    break;
                }
                Err(e) => {
                    self.paused = true;
                    let msg = format!("emulation stopped: {e}");
                    eprintln!("freki: {msg}");
                    self.status = Some((msg, Instant::now()));
                    break;
                }
            }
            session.gb.drain_audio(&mut self.samples);
            if let Some(audio) = &self.audio
                && speed == 1
            {
                audio.push(&self.samples);
            }
            self.samples.clear();
        }
        if let Err(e) = session.autosave() {
            self.status = Some((format!("save failed: {e}"), Instant::now()));
        }
    }

    fn update_texture(&mut self, ctx: &egui::Context) {
        let rgba = self.screen_rgba();
        let image = ColorImage::from_rgba_unmultiplied([SCREEN_WIDTH, SCREEN_HEIGHT], &rgba);
        match &mut self.texture {
            Some(texture) => texture.set(image, TextureOptions::NEAREST),
            None => self.texture = Some(ctx.load_texture("screen", image, TextureOptions::NEAREST)),
        }
    }

    /// The screen as shown (palette or colour correction applied), as RGBA.
    fn screen_rgba(&self) -> Vec<u8> {
        let colors = self.settings.palette.colors();
        let correct = self.settings.color_correction;
        let mut rgba = Vec::with_capacity(SCREEN_WIDTH * SCREEN_HEIGHT * 4);
        match self.session.as_ref().map(|s| s.gb.screen()) {
            Some(Screen::Shades(shades)) => {
                for &shade in shades {
                    let [r, g, b] = colors[(shade & 3) as usize];
                    rgba.extend([r, g, b, 0xFF]);
                }
            }
            Some(Screen::Rgb555(pixels)) => {
                for &color in pixels {
                    let [r, g, b] = settings::cgb_color(color, correct);
                    rgba.extend([r, g, b, 0xFF]);
                }
            }
            None => rgba.resize(SCREEN_WIDTH * SCREEN_HEIGHT * 4, 0x20),
        }
        rgba
    }

    /// Saves the screen as shown to a PNG next to the ROM.
    fn screenshot(&mut self) {
        let rgba = self.screen_rgba();
        self.with_session(|s| {
            let path = s.save_screenshot(&rgba)?;
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
                    self.with_session(|s| s.reset().map(|()| "reset".to_string()));
                }
                ui.separator();
                ui.menu_button("Console", |ui| self.console_menu(ui));
                ui.menu_button("Boot ROM", |ui| self.boot_rom_menu(ui));
                ui.separator();
                if ui.button("Controls…").clicked() {
                    ui.close();
                    self.show_controls = true;
                }
            });

            ui.menu_button("States", |ui| {
                if ui
                    .add_enabled(loaded, egui::Button::new("Save state").shortcut_text("F5"))
                    .clicked()
                {
                    ui.close();
                    self.with_session(|s| s.save_state());
                }
                if ui
                    .add_enabled(loaded, egui::Button::new("Load state").shortcut_text("F8"))
                    .clicked()
                {
                    ui.close();
                    self.with_session(|s| s.load_state());
                }
                ui.separator();
                ui.label("Slot (F6 / F7)");
                if let Some(session) = self.session.as_mut() {
                    let mut slot = session.slots.slot();
                    ui.horizontal_wrapped(|ui| {
                        for n in 0..SLOTS {
                            ui.radio_value(&mut slot, n, n.to_string());
                        }
                    });
                    while session.slots.slot() != slot {
                        session.slots.next();
                    }
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
                ui.label("Scale");
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
                if self.paused {
                    ui.label("paused");
                    ui.separator();
                } else if self.speed() > 1 {
                    ui.label(format!("fast-forward {}×", self.speed()));
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
        // Largest integer scale that fits keeps pixels sharp; fall back to fitting.
        let fit = (available.width() / SCREEN_WIDTH as f32)
            .min(available.height() / SCREEN_HEIGHT as f32);
        let scale = if fit >= 1.0 { fit.floor() } else { fit };
        let size = vec2(SCREEN_WIDTH as f32 * scale, SCREEN_HEIGHT as f32 * scale);
        let rect = Rect::from_center_size(available.center(), size);
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
    }

    fn cheat_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_cheats;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut status = None;
        egui::Window::new("Cheats")
            .open(&mut open)
            .resizable(true)
            .show(ctx, |ui| {
                let gb = &mut session.gb;
                let mut remove = None;
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        egui::Grid::new("cheat_list").striped(true).show(ui, |ui| {
                            let indices = 0..gb.cheats().list().len();
                            for i in indices {
                                let mut enabled = gb.cheats().list()[i].enabled;
                                if ui.checkbox(&mut enabled, "").changed() {
                                    gb.cheats_mut().set_enabled(i, enabled);
                                }
                                let cheat = &gb.cheats().list()[i];
                                ui.monospace(&cheat.text);
                                ui.label(&cheat.name);
                                if ui.small_button("✖").on_hover_text("Remove").clicked() {
                                    remove = Some(i);
                                }
                                ui.end_row();
                            }
                        });
                    });
                if let Some(i) = remove {
                    gb.cheats_mut().remove(i);
                }
                if gb.cheats().list().is_empty() {
                    ui.label("No cheats yet.");
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Code");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_code)
                            .hint_text("01VVLLHH, 9xVVLLHH or ABC-DEF-GHI")
                            .desired_width(130.0),
                    );
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut self.new_name).desired_width(140.0));
                    if ui.button("Add").clicked() {
                        match gb.cheats_mut().add(&self.new_code, &self.new_name) {
                            Ok(_) => {
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
                    if ui.button("Save .cht").clicked() {
                        status = Some(console::save_cheat_file(&path, gb).unwrap_or_else(|e| e));
                    }
                    if ui.button("Reload .cht").clicked() {
                        status = Some(
                            console::load_cheat_file(&path, gb)
                                .unwrap_or_else(|| format!("{} not found", file_name(&path))),
                        );
                    }
                });
            });
        self.show_cheats = open;
        if let Some(message) = status {
            self.set_status(message);
        }
    }

    fn search_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_search;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let ui_state = &mut self.search_ui;
        let mut status = None;
        egui::Window::new("Memory search").open(&mut open).resizable(true).show(ctx, |ui| {
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
                ui.horizontal(|ui| {
                    ui.label("Value");
                    ui.add(egui::TextEdit::singleline(&mut ui_state.value).desired_width(50.0));
                    let value = parse_byte(&ui_state.value);
                    let mut apply = |filter| {
                        search.filter(filter, |l| gb.peek_location(l));
                    };
                    if ui.add_enabled(value.is_some(), egui::Button::new("=")).clicked() {
                        apply(Filter::Equal(value.unwrap_or(0)));
                    }
                    if ui.add_enabled(value.is_some(), egui::Button::new("≠")).clicked() {
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
                egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                    for (i, &(location, _)) in search.candidates().iter().take(200).enumerate() {
                        let v = gb.peek_location(location);
                        let text = format!("{location:>7} = {v:>3} (${v:02X})");
                        if ui.selectable_label(ui_state.selected == Some(i), egui::RichText::new(text).monospace()).clicked() {
                            ui_state.selected = Some(i);
                            ui_state.cheat_value = v.to_string();
                        }
                    }
                    if search.len() > 200 {
                        ui.label(format!("… {} more", search.len() - 200));
                    }
                });
                if let Some(&(location, _)) = ui_state.selected.and_then(|i| search.candidates().get(i)) {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(format!("{location}: hold at"));
                        ui.add(egui::TextEdit::singleline(&mut ui_state.cheat_value).desired_width(40.0));
                        ui.label("name");
                        ui.add(egui::TextEdit::singleline(&mut ui_state.cheat_name).desired_width(120.0));
                        let value = parse_byte(&ui_state.cheat_value);
                        if ui.add_enabled(value.is_some(), egui::Button::new("Add cheat")).clicked() {
                            let value = value.unwrap_or(0);
                            let code = Code::gameshark(location, value);
                            let name = if ui_state.cheat_name.is_empty() {
                                format!("{location} = {value}")
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
        });
        self.show_search = open;
        if let Some(message) = status {
            self.set_status(message);
        }
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
            .show(ctx, |ui| {
                ui.label(
                    "Click a binding, then press the key or gamepad button to use (Esc cancels).",
                );
                egui::Grid::new("bindings").striped(true).show(ui, |ui| {
                    ui.strong("Game Boy");
                    ui.strong("Keyboard");
                    ui.strong("Gamepad");
                    ui.end_row();
                    for button in input::BUTTONS {
                        ui.label(input::button_name(button));
                        for kind in [Capture::Key, Capture::Pad] {
                            let waiting = self.capture == Some((button, kind));
                            let text = match (waiting, kind) {
                                (true, Capture::Key) => "press a key…".to_string(),
                                (true, Capture::Pad) => "press a button…".to_string(),
                                (false, Capture::Key) => {
                                    self.settings.bindings.key(button).name().to_string()
                                }
                                (false, Capture::Pad) => {
                                    input::pad_name(self.settings.bindings.pad(button)).to_string()
                                }
                            };
                            if ui.add(egui::Button::new(text).selected(waiting)).clicked() {
                                self.capture = if waiting { None } else { Some((button, kind)) };
                                self.controls_message = None;
                                // Forget presses made before the capture started.
                                self.gamepads.update();
                                self.gamepads.take_pressed();
                            }
                        }
                        ui.end_row();
                    }
                });
                if let Some(message) = &self.controls_message {
                    ui.colored_label(egui::Color32::LIGHT_RED, message);
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !self.settings.bindings.is_default(),
                            egui::Button::new("Reset to defaults"),
                        )
                        .clicked()
                    {
                        self.settings.bindings = Default::default();
                        self.capture = None;
                    }
                    ui.weak("The left stick also steers.");
                });
                ui.separator();
                egui::Grid::new("controls").striped(true).show(ui, |ui| {
                    for (what, keys) in [
                        ("Save / load state", "F5 / F8"),
                        ("Previous / next slot", "F6 / F7"),
                        ("Rewind", "hold R"),
                        ("Fast-forward", "hold Tab"),
                        ("Pause", "P"),
                        ("Cheats on/off", "F9"),
                        ("Screenshot", "F12"),
                        ("Open ROM / reset / quit", "Ctrl+O / Ctrl+R / Ctrl+Q"),
                    ] {
                        ui.label(what);
                        ui.label(keys);
                        ui.end_row();
                    }
                });
                ui.label("The terminal freki was started from also accepts commands (type help).");
            });
        if !open {
            self.capture = None;
        }
        self.show_controls = open;
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
        self.update_texture(&ctx);

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
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
        if let Some(session) = &self.session {
            let palettes = debug::Palettes::new(
                &session.gb,
                self.settings.palette.colors(),
                self.settings.color_correction,
            );
            self.debug.show(&ctx, &session.gb, &palettes);
        }

        ctx.request_repaint();
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
        self.settings.save(storage);
    }

    fn on_exit(&mut self) {
        self.close_rom();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bytes() {
        assert_eq!(parse_byte("42"), Some(42));
        assert_eq!(parse_byte("$2A"), Some(42));
        assert_eq!(parse_byte("0x2a"), Some(42));
        assert_eq!(parse_byte("256"), None);
        assert_eq!(parse_byte("x"), None);
    }
}
