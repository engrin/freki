//! The egui application: menu bar, screen, status bar and tool windows.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Color32, ColorImage, Key, KeyboardShortcut, Modifiers, Rect, TextureHandle,
    TextureOptions, pos2, vec2,
};
use freki_core::{
    Button, Code, FRAMES_PER_SECOND, Filter, MemorySearch, SCREEN_HEIGHT, SCREEN_WIDTH,
};

use crate::audio::Audio;
use crate::console::{self, Console};
use crate::gamepad::Gamepads;
use crate::session::{Session, file_name};
use crate::states::SLOTS;

const KEYMAP: [(Key, Button); 8] = [
    (Key::ArrowRight, Button::Right),
    (Key::ArrowLeft, Button::Left),
    (Key::ArrowUp, Button::Up),
    (Key::ArrowDown, Button::Down),
    (Key::X, Button::A),
    (Key::Z, Button::B),
    (Key::Backspace, Button::Select),
    (Key::Enter, Button::Start),
];

const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);
const RESET: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::R);

/// Audio queued ahead of playback. Emulation slows down when more than this is
/// buffered and speeds up when less than a quarter is, so audio stays in sync.
const AUDIO_LATENCY: Duration = Duration::from_millis(60);
/// Never emulate more than this many frames in one UI update (e.g. after a stall).
const MAX_FRAMES_PER_UPDATE: u32 = 4;
/// How long a status message stays in the status bar.
const STATUS_DURATION: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Palette {
    Green,
    Grey,
    Pocket,
}

impl Palette {
    const ALL: [Palette; 3] = [Palette::Green, Palette::Grey, Palette::Pocket];

    fn name(self) -> &'static str {
        match self {
            Palette::Green => "DMG green",
            Palette::Grey => "Greyscale",
            Palette::Pocket => "Pocket",
        }
    }

    /// RGB for shades 0 (lightest) to 3 (darkest).
    fn colors(self) -> [[u8; 3]; 4] {
        match self {
            Palette::Green => [
                [0xE0, 0xF8, 0xD0],
                [0x88, 0xC0, 0x70],
                [0x34, 0x68, 0x56],
                [0x08, 0x18, 0x20],
            ],
            Palette::Grey => [
                [0xFF, 0xFF, 0xFF],
                [0xAA, 0xAA, 0xAA],
                [0x55, 0x55, 0x55],
                [0x00, 0x00, 0x00],
            ],
            Palette::Pocket => [
                [0xC4, 0xCF, 0xA1],
                [0x8B, 0x95, 0x6D],
                [0x4D, 0x53, 0x3C],
                [0x1F, 0x1F, 0x1F],
            ],
        }
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
    audio: Option<Audio>,
    gamepads: Gamepads,
    console: Console,
    texture: Option<TextureHandle>,
    samples: Vec<f32>,

    paused: bool,
    rewinding: bool,
    palette: Palette,
    scale: u32,
    status: Option<(String, Instant)>,
    /// Frames of emulation owed to real time.
    frame_debt: f64,
    last_tick: Instant,

    show_cheats: bool,
    show_search: bool,
    show_controls: bool,
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
            audio,
            gamepads: Gamepads::open(),
            console: Console::start(),
            texture: None,
            samples: Vec::new(),
            paused: false,
            rewinding: false,
            palette: Palette::Green,
            scale: 4,
            status: None,
            frame_debt: 0.0,
            last_tick: Instant::now(),
            show_cheats: false,
            show_search: false,
            show_controls: false,
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

    fn open_rom(&mut self, ctx: &egui::Context, path: &Path) {
        self.close_rom();
        match Session::open(path) {
            Ok((mut session, message)) => {
                if let Some(audio) = &self.audio {
                    session.gb.set_audio_sample_rate(audio.sample_rate());
                }
                let cheats = console::load_cheat_file(&session.cheat_file(), &mut session.gb);
                self.console.set_cheat_file(session.cheat_file());
                let title = if session.title.is_empty() {
                    "freki".to_string()
                } else {
                    format!("freki — {}", session.title)
                };
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
                self.session = Some(session);
                self.search_ui = SearchUi::default();
                self.paused = false;
                self.set_status(message);
                if let Some(msg) = cheats {
                    self.set_status(msg);
                }
            }
            Err(e) => self.set_status(format!("cannot open {}: {e}", file_name(path))),
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
        self.scale = scale;
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
        if ctx.egui_wants_keyboard_input() {
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
        if pressed(Key::P) {
            self.paused = !self.paused;
            self.set_status(if self.paused { "paused" } else { "resumed" });
        }
        self.rewinding = ctx.input(|i| i.key_down(Key::R)) && self.session.is_some();
    }

    fn toggle_cheats(&mut self) {
        self.with_session(|s| {
            let cheats = s.gb.cheats_mut();
            cheats.enabled = !cheats.enabled;
            let state = if cheats.enabled { "on" } else { "off" };
            Ok(format!("cheats {state} ({})", cheats.list().len()))
        });
    }

    /// Runs as many frames as real time (and the audio queue) calls for.
    fn emulate(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        self.frame_debt =
            (self.frame_debt + elapsed * FRAMES_PER_SECOND).min(MAX_FRAMES_PER_UPDATE as f64);
        let mut frames = self.frame_debt.floor() as u32;
        self.frame_debt -= frames as f64;

        let typing = ctx.egui_wants_keyboard_input();
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

        // Keep audio in sync: skip a frame when too much is queued, add one when starved.
        if let Some(audio) = &self.audio {
            let target = (audio.sample_rate() as f64 * AUDIO_LATENCY.as_secs_f64()) as usize;
            let queued = audio.queued_frames();
            if queued > target {
                frames = frames.saturating_sub(1);
            } else if queued < target / 4 && frames < MAX_FRAMES_PER_UPDATE {
                frames += 1;
            }
        }

        self.gamepads.update();
        for _ in 0..frames {
            for (key, button) in KEYMAP {
                let down = !typing && ctx.input(|i| i.key_down(key));
                session
                    .gb
                    .set_button(button, down || self.gamepads.is_pressed(button));
            }
            if let Err(e) = session.run_frame() {
                self.paused = true;
                let msg = format!("emulation stopped: {e}");
                eprintln!("freki: {msg}");
                self.status = Some((msg, Instant::now()));
                break;
            }
            session.gb.drain_audio(&mut self.samples);
            if let Some(audio) = &self.audio {
                audio.push(&self.samples);
            }
            self.samples.clear();
        }
        if let Err(e) = session.autosave() {
            self.status = Some((format!("save failed: {e}"), Instant::now()));
        }
    }

    fn update_texture(&mut self, ctx: &egui::Context) {
        let colors = self.palette.colors();
        let mut rgba = Vec::with_capacity(SCREEN_WIDTH * SCREEN_HEIGHT * 4);
        match &self.session {
            Some(session) => {
                for &shade in session.gb.framebuffer() {
                    let [r, g, b] = colors[(shade & 3) as usize];
                    rgba.extend([r, g, b, 0xFF]);
                }
            }
            None => rgba.resize(SCREEN_WIDTH * SCREEN_HEIGHT * 4, 0x20),
        }
        let image = ColorImage::from_rgba_unmultiplied([SCREEN_WIDTH, SCREEN_HEIGHT], &rgba);
        match &mut self.texture {
            Some(texture) => texture.set(image, TextureOptions::NEAREST),
            None => self.texture = Some(ctx.load_texture("screen", image, TextureOptions::NEAREST)),
        }
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
                if ui
                    .add_enabled(loaded, egui::Button::new("Close ROM"))
                    .clicked()
                {
                    ui.close();
                    self.close_rom();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title("freki".into()));
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
                    if ui.radio(self.scale == scale, format!("{scale}×")).clicked() {
                        ui.close();
                        self.set_scale(&ctx, scale);
                    }
                }
                ui.separator();
                ui.label("Palette");
                for palette in Palette::ALL {
                    ui.radio_value(&mut self.palette, palette, palette.name());
                }
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
                }
            }
            // Pause state is shown above; don't repeat a pause/resume message next to it.
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
                            .hint_text("01VVLLHH or ABC-DEF-GHI")
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
                        search.filter(filter, |a| gb.peek(a));
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
                    for (i, &(addr, _)) in search.candidates().iter().take(200).enumerate() {
                        let v = gb.peek(addr);
                        let text = format!("${addr:04X} = {v:>3} (${v:02X})");
                        if ui.selectable_label(ui_state.selected == Some(i), egui::RichText::new(text).monospace()).clicked() {
                            ui_state.selected = Some(i);
                            ui_state.cheat_value = v.to_string();
                        }
                    }
                    if search.len() > 200 {
                        ui.label(format!("… {} more", search.len() - 200));
                    }
                });
                if let Some(&(addr, _)) = ui_state.selected.and_then(|i| search.candidates().get(i)) {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(format!("${addr:04X}: hold at"));
                        ui.add(egui::TextEdit::singleline(&mut ui_state.cheat_value).desired_width(40.0));
                        ui.label("name");
                        ui.add(egui::TextEdit::singleline(&mut ui_state.cheat_name).desired_width(120.0));
                        let value = parse_byte(&ui_state.cheat_value);
                        if ui.add_enabled(value.is_some(), egui::Button::new("Add cheat")).clicked() {
                            let value = value.unwrap_or(0);
                            let code = Code::gameshark(addr, value);
                            let name = if ui_state.cheat_name.is_empty() {
                                format!("${addr:04X} = {value}")
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

    fn info_windows(&mut self, ctx: &egui::Context) {
        egui::Window::new("Controls")
            .open(&mut self.show_controls)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("controls").striped(true).show(ui, |ui| {
                    for (what, keys) in [
                        ("D-pad", "Arrow keys / gamepad D-pad or left stick"),
                        ("A / B", "X / Z / gamepad right / bottom button"),
                        (
                            "Start / Select",
                            "Enter / Backspace / gamepad Start / Select",
                        ),
                        ("Save / load state", "F5 / F8"),
                        ("Previous / next slot", "F6 / F7"),
                        ("Rewind", "hold R"),
                        ("Pause", "P"),
                        ("Cheats on/off", "F9"),
                        ("Open ROM / reset / quit", "Ctrl+O / Ctrl+R / Ctrl+Q"),
                    ] {
                        ui.label(what);
                        ui.label(keys);
                        ui.end_row();
                    }
                });
                ui.label("The terminal freki was started from also accepts commands (type help).");
            });
        egui::Window::new("About freki")
            .open(&mut self.show_about)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading(format!("freki {}", env!("CARGO_PKG_VERSION")));
                ui.label("A Game Boy (DMG) emulator written in Rust.");
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

        ctx.request_repaint();
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
