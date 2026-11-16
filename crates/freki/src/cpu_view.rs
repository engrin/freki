//! The Debugger window: execution control, registers, disassembly, memory, stack and
//! breakpoint/watchpoint lists.

use eframe::egui::{self, Color32, RichText};
use freki_core::{GameBoy, Stop, Watchpoint};

use crate::console::{describe_stop, describe_watch};

/// What the debugger wants the app to do after drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Continue,
    Pause,
    StepInstruction,
    StepScanline,
    StepFrame,
}

pub struct CpuView {
    pub open: bool,
    /// Why execution last stopped (shown until the game continues).
    pub last_stop: Option<Stop>,
    /// Disassembly start; `None` follows PC.
    disasm_at: Option<u16>,
    disasm_input: String,
    memory_at: u16,
    memory_input: String,
    break_input: String,
    watch_start: String,
    watch_end: String,
    watch_read: bool,
    watch_write: bool,
    error: Option<String>,
}

impl CpuView {
    pub fn new() -> Self {
        Self {
            open: false,
            last_stop: None,
            disasm_at: None,
            disasm_input: String::new(),
            memory_at: 0xC000,
            memory_input: "C000".into(),
            break_input: String::new(),
            watch_start: String::new(),
            watch_end: String::new(),
            watch_read: false,
            watch_write: true,
            error: None,
        }
    }

    /// Draws the window. Returns the requested execution action, if any.
    pub fn show(&mut self, ctx: &egui::Context, gb: &mut GameBoy, paused: bool) -> Option<Action> {
        let mut action = None;
        let mut open = self.open;
        egui::Window::new("Debugger")
            .open(&mut open)
            .default_size([760.0, 560.0])
            .resizable(true)
            .show(ctx, |ui| {
                action = self.toolbar(ui, paused);
                ui.separator();
                ui.columns(2, |cols| {
                    self.registers(&mut cols[0], gb);
                    cols[0].separator();
                    self.disassembly(&mut cols[0], gb);
                    self.memory(&mut cols[1], gb);
                    cols[1].separator();
                    stack(&mut cols[1], gb);
                });
                ui.separator();
                self.breakpoints(ui, gb);
            });
        self.open = open;
        action
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, paused: bool) -> Option<Action> {
        let mut action = None;
        ui.horizontal(|ui| {
            if paused {
                if ui.button("Continue").on_hover_text("F10").clicked() {
                    action = Some(Action::Continue);
                }
            } else if ui.button("Pause").on_hover_text("P").clicked() {
                action = Some(Action::Pause);
            }
            ui.add_enabled_ui(paused, |ui| {
                if ui
                    .button("Step")
                    .on_hover_text("F11: one instruction")
                    .clicked()
                {
                    action = Some(Action::StepInstruction);
                }
                if ui
                    .button("Scanline")
                    .on_hover_text("run to the next scanline")
                    .clicked()
                {
                    action = Some(Action::StepScanline);
                }
                if ui
                    .button("Frame")
                    .on_hover_text("run to the next frame")
                    .clicked()
                {
                    action = Some(Action::StepFrame);
                }
            });
            ui.separator();
            match (&self.last_stop, paused) {
                (Some(stop), true) => {
                    ui.colored_label(Color32::from_rgb(0xFF, 0xB0, 0x40), describe_stop(stop));
                }
                (_, true) => {
                    ui.label("paused");
                }
                (_, false) => {
                    ui.label("running");
                }
            }
        });
        action
    }

    fn registers(&self, ui: &mut egui::Ui, gb: &GameBoy) {
        let r = gb.registers();
        let flag = |bit: u8, c: &str| {
            if r.f & bit != 0 {
                c.to_string()
            } else {
                "·".into()
            }
        };
        egui::Grid::new("regs")
            .num_columns(4)
            .spacing([16.0, 2.0])
            .show(ui, |ui| {
                for (name, value) in [
                    ("AF", r.af()),
                    ("BC", r.bc()),
                    ("DE", r.de()),
                    ("HL", r.hl()),
                ] {
                    ui.monospace(name);
                    ui.monospace(format!("${value:04X}"));
                    ui.end_row();
                }
                ui.monospace("SP");
                ui.monospace(format!("${:04X}", r.sp));
                ui.monospace("PC");
                ui.monospace(format!("${:04X}", r.pc));
                ui.end_row();
            });
        ui.monospace(format!(
            "flags {}{}{}{}  IME {}  {}  LY {:>3}  IE ${:02X}  IF ${:02X}",
            flag(0x80, "Z"),
            flag(0x40, "N"),
            flag(0x20, "H"),
            flag(0x10, "C"),
            u8::from(gb.ime()),
            if gb.halted() { "HALT" } else { "    " },
            gb.peek(0xFF44),
            gb.peek(0xFFFF),
            gb.peek(0xFF0F) & 0x1F,
        ));
    }

    fn disassembly(&mut self, ui: &mut egui::Ui, gb: &mut GameBoy) {
        ui.horizontal(|ui| {
            ui.strong("Disassembly");
            let follow = self.disasm_at.is_none();
            if ui.selectable_label(follow, "follow PC").clicked() {
                self.disasm_at = None;
            }
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.disasm_input)
                    .hint_text("addr")
                    .desired_width(48.0),
            );
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.disasm_at = parse_addr(&self.disasm_input);
            }
        });
        let mut addr = self.disasm_at.unwrap_or_else(|| gb.pc());
        let pc = gb.pc();
        let mut toggle = None;
        for _ in 0..18 {
            let ins = gb.disassemble(addr);
            ui.horizontal(|ui| {
                let has_bp = gb.breakpoints().pc.contains(&addr);
                let dot = if has_bp {
                    RichText::new("●").color(Color32::from_rgb(0xE0, 0x40, 0x40))
                } else {
                    RichText::new("○").color(Color32::DARK_GRAY)
                };
                if ui
                    .add(egui::Button::new(dot).frame(false))
                    .on_hover_text("toggle breakpoint")
                    .clicked()
                {
                    toggle = Some(addr);
                }
                let bytes: Vec<String> = ins.bytes.iter().map(|b| format!("{b:02X}")).collect();
                let line = format!("${addr:04X}  {:<9} {}", bytes.join(" "), ins.text);
                let text = RichText::new(line).monospace();
                if addr == pc {
                    ui.label(
                        text.background_color(Color32::from_rgb(0x30, 0x50, 0x80))
                            .color(Color32::WHITE),
                    );
                } else {
                    ui.label(text);
                }
            });
            addr = addr.wrapping_add(ins.len as u16);
        }
        if let Some(addr) = toggle {
            let set = &mut gb.breakpoints_mut().pc;
            if !set.remove(&addr) {
                set.insert(addr);
            }
        }
    }

    fn memory(&mut self, ui: &mut egui::Ui, gb: &GameBoy) {
        ui.horizontal(|ui| {
            ui.strong("Memory");
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.memory_input)
                    .hint_text("addr")
                    .desired_width(48.0),
            );
            if edit.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && let Some(addr) = parse_addr(&self.memory_input)
            {
                self.memory_at = addr & 0xFFF0;
            }
            for (label, addr) in [
                ("WRAM", 0xC000),
                ("HRAM", 0xFF80),
                ("I/O", 0xFF00),
                ("OAM", 0xFE00),
            ] {
                if ui.small_button(label).clicked() {
                    self.memory_at = addr;
                    self.memory_input = format!("{addr:04X}");
                }
            }
        });
        for row in 0..16u16 {
            let base = self.memory_at.wrapping_add(row * 8);
            let bytes: Vec<String> = (0..8)
                .map(|i| format!("{:02X}", gb.peek(base.wrapping_add(i))))
                .collect();
            ui.monospace(format!("${base:04X}  {}", bytes.join(" ")));
        }
    }

    fn breakpoints(&mut self, ui: &mut egui::Ui, gb: &mut GameBoy) {
        ui.horizontal(|ui| {
            ui.strong("Breakpoint");
            ui.add(
                egui::TextEdit::singleline(&mut self.break_input)
                    .hint_text("addr")
                    .desired_width(48.0),
            );
            if ui.button("Add").clicked() {
                match parse_addr(&self.break_input) {
                    Some(addr) => {
                        gb.breakpoints_mut().pc.insert(addr);
                        self.break_input.clear();
                        self.error = None;
                    }
                    None => self.error = Some(format!("{:?} is not an address", self.break_input)),
                }
            }
            ui.separator();
            ui.strong("Watch");
            ui.add(
                egui::TextEdit::singleline(&mut self.watch_start)
                    .hint_text("addr")
                    .desired_width(48.0),
            );
            ui.label("–");
            ui.add(
                egui::TextEdit::singleline(&mut self.watch_end)
                    .hint_text("end")
                    .desired_width(48.0),
            );
            ui.checkbox(&mut self.watch_read, "read");
            ui.checkbox(&mut self.watch_write, "write");
            if ui.button("Add").clicked() {
                let start = parse_addr(&self.watch_start);
                let end = if self.watch_end.trim().is_empty() {
                    start
                } else {
                    parse_addr(&self.watch_end)
                };
                match (start, end, self.watch_read || self.watch_write) {
                    (Some(s), Some(e), true) => {
                        gb.breakpoints_mut().watches.push(Watchpoint::new(
                            s,
                            e,
                            self.watch_read,
                            self.watch_write,
                        ));
                        self.watch_start.clear();
                        self.watch_end.clear();
                        self.error = None;
                    }
                    (_, _, false) => self.error = Some("choose read and/or write".into()),
                    _ => self.error = Some("enter a valid address (and optional end)".into()),
                }
            }
        });
        if let Some(err) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, err);
        }
        let mut remove_bp = None;
        let mut remove_watch = None;
        ui.horizontal_wrapped(|ui| {
            for &pc in &gb.breakpoints().pc {
                if ui
                    .button(format!("break ${pc:04X} ✖"))
                    .on_hover_text("remove")
                    .clicked()
                {
                    remove_bp = Some(pc);
                }
            }
            for (i, w) in gb.breakpoints().watches.iter().enumerate() {
                if ui
                    .button(format!("watch {} ✖", describe_watch(w)))
                    .on_hover_text("remove")
                    .clicked()
                {
                    remove_watch = Some(i);
                }
            }
            if gb.breakpoints().is_empty() {
                ui.label("No breakpoints. Click ○ in the disassembly or add one above.");
            }
        });
        if let Some(pc) = remove_bp {
            gb.breakpoints_mut().pc.remove(&pc);
        }
        if let Some(i) = remove_watch {
            gb.breakpoints_mut().watches.remove(i);
        }
    }
}

fn stack(ui: &mut egui::Ui, gb: &GameBoy) {
    ui.strong("Stack");
    let sp = gb.registers().sp;
    for i in 0..8u16 {
        let addr = sp.wrapping_add(i * 2);
        let value = u16::from_le_bytes([gb.peek(addr), gb.peek(addr.wrapping_add(1))]);
        let marker = if i == 0 { "SP→" } else { "   " };
        ui.monospace(format!("{marker} ${addr:04X}  ${value:04X}"));
    }
}

/// Parses an address: hex, with or without `$` / `0x`.
fn parse_addr(s: &str) -> Option<u16> {
    let s = s.trim();
    let hex = s
        .strip_prefix('$')
        .or_else(|| s.strip_prefix("0x"))
        .unwrap_or(s);
    u16::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_hex() {
        assert_eq!(parse_addr("C000"), Some(0xC000));
        assert_eq!(parse_addr("$ff80"), Some(0xFF80));
        assert_eq!(parse_addr("0x150"), Some(0x150));
        assert_eq!(parse_addr("zz"), None);
    }
}
