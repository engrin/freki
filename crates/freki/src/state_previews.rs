//! Thumbnails of the save state slots for the States menu, loaded from the `.ssN.png`
//! files and kept as textures until the slot or the game changes.

use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use crate::states::{SLOTS, StateSlots, age};

/// Width of a thumbnail in the menu, in points.
const THUMB_WIDTH: f32 = 80.0;

#[derive(Clone)]
struct Entry {
    saved: Option<SystemTime>,
    texture: Option<TextureHandle>,
}

#[derive(Default)]
pub struct StatePreviews {
    rom: PathBuf,
    entries: Vec<Option<Entry>>,
}

/// What the player did with the slot grid.
pub enum Pick {
    Select(u8),
    Load(u8),
}

impl StatePreviews {
    /// The slot's save time and thumbnail, reloaded when the state file changed.
    fn entry(&mut self, ctx: &egui::Context, slots: &StateSlots, slot: u8) -> Entry {
        if self.rom != slots.rom_path() || self.entries.len() != usize::from(SLOTS) {
            self.rom = slots.rom_path().to_path_buf();
            self.entries = vec![None; usize::from(SLOTS)];
        }
        let saved = slots.saved_at(slot);
        let cached = &mut self.entries[usize::from(slot)];
        if cached.as_ref().is_none_or(|e| e.saved != saved) {
            let texture = slots.preview(slot).map(|(rgba, size)| {
                let image = ColorImage::from_rgba_unmultiplied(size, &rgba);
                ctx.load_texture(format!("state {slot}"), image, TextureOptions::LINEAR)
            });
            *cached = Some(Entry { saved, texture });
        }
        cached.clone().expect("filled above")
    }

    /// The slots as a grid of thumbnails, the current one highlighted.
    pub fn grid(&mut self, ui: &mut egui::Ui, slots: &StateSlots) -> Option<Pick> {
        let mut pick = None;
        let height = THUMB_WIDTH * 0.9;
        egui::Grid::new("state slots")
            .spacing([6.0, 6.0])
            .show(ui, |ui| {
                for slot in 0..SLOTS {
                    let entry = self.entry(ui.ctx(), slots, slot);
                    let response = slot_cell(ui, slot, &entry, slot == slots.slot(), height);
                    if response.double_clicked() && entry.saved.is_some() {
                        pick = Some(Pick::Load(slot));
                    } else if response.clicked() {
                        pick = Some(Pick::Select(slot));
                    }
                    if slot % 5 == 4 {
                        ui.end_row();
                    }
                }
            });
        pick
    }
}

fn slot_cell(
    ui: &mut egui::Ui,
    slot: u8,
    entry: &Entry,
    selected: bool,
    height: f32,
) -> egui::Response {
    let label = match entry.saved {
        Some(time) => format!("{slot} · {}", age(time)),
        None => format!("{slot} · empty"),
    };
    let visuals = ui.visuals().clone();
    let stroke = if selected {
        visuals.selection.stroke
    } else {
        egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color)
    };
    let frame = egui::Frame::new()
        .inner_margin(3.0)
        .corner_radius(3.0)
        .stroke(stroke)
        .fill(if selected {
            visuals.selection.bg_fill.gamma_multiply(0.4)
        } else {
            egui::Color32::TRANSPARENT
        });
    let inner = frame.show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.set_width(THUMB_WIDTH);
            let size = egui::vec2(THUMB_WIDTH, height);
            match &entry.texture {
                Some(texture) => {
                    // Fit the picture (160×144 or the Super Game Boy's 256×224) in the box.
                    let [w, h] = texture.size().map(|v| v as f32);
                    let scale = (size.x / w).min(size.y / h);
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    let image = egui::Rect::from_center_size(
                        rect.center(),
                        egui::vec2(w * scale, h * scale),
                    );
                    ui.painter().image(
                        texture.id(),
                        image,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                None => {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 2.0, visuals.extreme_bg_color);
                    let text = if entry.saved.is_some() {
                        "no preview"
                    } else {
                        ""
                    };
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::TextStyle::Small.resolve(ui.style()),
                        visuals.weak_text_color(),
                    );
                }
            }
            ui.add(egui::Label::new(egui::RichText::new(label).small()).truncate());
        });
    });
    let hint = if entry.saved.is_some() {
        "Click to select, double-click to load"
    } else {
        "Click to select"
    };
    ui.interact(
        inner.response.rect,
        ui.scope_id().with(("state slot", slot)),
        egui::Sense::click(),
    )
    .on_hover_text(hint)
}
