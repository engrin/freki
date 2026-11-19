//! The Printer window: the paper coming out of the Game Boy Printer.

use std::path::PathBuf;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use freki_core::{PAPER_WIDTH, Printer};

use crate::printout;
use crate::window::tool_window;

#[derive(Default)]
pub struct PrinterView {
    pub open: bool,
    texture: Option<TextureHandle>,
    /// Shades shown: the page being printed, or else the last finished one.
    shown: Vec<u8>,
    /// The last finished page.
    last_page: Vec<u8>,
    /// Where the last finished page was saved.
    pub last_saved: Option<PathBuf>,
}

impl PrinterView {
    /// A page came out of the printer (and was saved to `saved`).
    pub fn page_done(&mut self, shades: Vec<u8>, saved: Option<PathBuf>) {
        self.last_page = shades;
        self.last_saved = saved;
    }

    /// Shows the window; returns whether the page being printed should be torn off.
    pub fn show(&mut self, ctx: &egui::Context, printer: Option<&Printer>) -> bool {
        let paper = printer.map(Printer::paper).unwrap_or_default();
        let printing = !paper.is_empty();
        let shown = if printing { paper } else { &self.last_page };
        if shown != self.shown.as_slice() || self.texture.is_none() {
            self.shown = shown.to_vec();
            let height = (self.shown.len() / PAPER_WIDTH).max(1);
            let rgba = if self.shown.is_empty() {
                vec![0xFF; PAPER_WIDTH * 4]
            } else {
                printout::rgba(&self.shown)
            };
            let image = ColorImage::from_rgba_unmultiplied([PAPER_WIDTH, height], &rgba);
            match self.texture.as_mut() {
                Some(texture) => texture.set(image, TextureOptions::NEAREST),
                None => {
                    self.texture =
                        Some(ctx.load_texture("printout", image, TextureOptions::NEAREST))
                }
            }
        }
        let mut tear_off = false;
        let open = tool_window(ctx, "printer", "freki — Printer", [400.0, 600.0], |ui| {
            ui.horizontal_wrapped(|ui| {
                let text = match (printer, printing, &self.last_saved) {
                    (None, ..) => "No printer connected (Link cable → Game Boy Printer)".into(),
                    (Some(_), true, _) => "Printing…".to_string(),
                    (Some(_), false, Some(path)) => format!(
                        "Last page saved as {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    (Some(_), false, None) => "Waiting for the game to print".into(),
                };
                ui.label(text);
                if ui
                    .add_enabled(printing, egui::Button::new("Tear off"))
                    .on_hover_text(
                        "Finishes the page now and saves it. Pages are finished and saved \
                         by themselves when the game feeds the paper out",
                    )
                    .clicked()
                {
                    tear_off = true;
                }
            });
            ui.separator();
            if self.shown.is_empty() {
                return;
            }
            let Some(texture) = &self.texture else {
                return;
            };
            let width = ui.available_width();
            let scale = (width / PAPER_WIDTH as f32).floor().clamp(1.0, 4.0);
            let size = texture.size_vec2() * scale;
            egui::ScrollArea::vertical()
                .stick_to_bottom(printing)
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Image::new((texture.id(), size)));
                    });
                });
        });
        if !open {
            self.open = false;
        }
        tear_off
    }
}
