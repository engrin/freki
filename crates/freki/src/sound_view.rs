//! The Sound channels window: what each of the four channels is doing, with a small
//! oscilloscope, and buttons to mute or solo them.

use eframe::egui::{self, Color32, RichText, Stroke, pos2, vec2};
use freki_core::{ChannelInfo, GameBoy};

use crate::window::tool_window;

const NAMES: [&str; 4] = ["Square + sweep", "Square", "Wave", "Noise"];
const DUTIES: [&str; 4] = ["12.5 %", "25 %", "50 %", "75 %"];
const NOTES: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];
const COLORS: [Color32; 4] = [
    Color32::from_rgb(0x6C, 0xC4, 0xF0),
    Color32::from_rgb(0x8A, 0xD8, 0x7C),
    Color32::from_rgb(0xF0, 0xB8, 0x5C),
    Color32::from_rgb(0xE0, 0x80, 0xC8),
];

#[derive(Default)]
pub struct SoundView {
    pub open: bool,
    /// Channels muted here; applied to the game every frame (and to a reset game).
    pub muted: [bool; 4],
}

impl SoundView {
    pub fn show(&mut self, ctx: &egui::Context, gb: &GameBoy) {
        let channels = gb.sound_channels();
        self.open = tool_window(
            ctx,
            "sound",
            "freki — Sound channels",
            [520.0, 640.0],
            |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(self.muted.contains(&true), egui::Button::new("Unmute all"))
                        .clicked()
                    {
                        self.muted = [false; 4];
                    }
                    ui.weak("Mute and solo apply to what you hear and to recorded video.");
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (n, info) in channels.iter().enumerate() {
                            self.channel(ui, gb, n, info);
                            ui.separator();
                        }
                    });
            },
        );
    }

    fn channel(&mut self, ui: &mut egui::Ui, gb: &GameBoy, n: usize, info: &ChannelInfo) {
        let muted = self.muted[n];
        let solo = !muted && self.muted.iter().enumerate().all(|(i, &m)| i == n || m);
        ui.horizontal(|ui| {
            let dot = if info.on && !muted {
                RichText::new("●").color(COLORS[n])
            } else {
                RichText::new("○").color(Color32::DARK_GRAY)
            };
            ui.label(dot);
            ui.strong(format!("{} · {}", n + 1, NAMES[n]));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .selectable_label(solo, "Solo")
                    .on_hover_text("Hear only this channel (click again for all)")
                    .clicked()
                {
                    self.muted = if solo {
                        [false; 4]
                    } else {
                        std::array::from_fn(|i| i != n)
                    };
                }
                if ui.selectable_label(muted, "Mute").clicked() {
                    self.muted[n] = !muted;
                }
            });
        });
        ui.label(RichText::new(describe(n, info)).monospace().small());
        scope(ui, gb, n, muted || !info.on);
        if let Some(wave) = &info.wave {
            wave_ram(ui, wave);
        }
    }
}

/// One line of settings: pitch, duty, volume and envelope, sweep, length, routing.
fn describe(n: usize, info: &ChannelInfo) -> String {
    let mut parts = Vec::new();
    if n == 3 {
        let width = if info.short_noise == Some(true) {
            "7-bit"
        } else {
            "15-bit"
        };
        parts.push(format!("{} Hz {width}", hz(info.frequency)));
    } else {
        parts.push(format!(
            "{} Hz {}",
            hz(info.frequency),
            note(info.frequency)
        ));
    }
    if let Some(duty) = info.duty {
        parts.push(format!("duty {}", DUTIES[usize::from(duty & 3)]));
    }
    parts.push(format!("vol {:>2}", info.volume));
    if let Some((initial, rising, pace)) = info.envelope {
        let way = if pace == 0 {
            "–"
        } else if rising {
            "↑"
        } else {
            "↓"
        };
        parts.push(format!("env {initial}{way}{pace}"));
    }
    if let Some((pace, decrease, step)) = info.sweep {
        let way = if decrease { "↓" } else { "↑" };
        parts.push(if pace == 0 {
            "sweep off".to_string()
        } else {
            format!("sweep {pace}{way}{step}")
        });
    }
    let (enabled, remaining) = info.length;
    if enabled {
        parts.push(format!("len {remaining}"));
    }
    let side = match (info.left, info.right) {
        (true, true) => "L R",
        (true, false) => "L",
        (false, true) => "R",
        (false, false) => "silent",
    };
    parts.push(side.into());
    if !info.dac_on {
        parts.push("DAC off".into());
    }
    parts.join(" · ")
}

fn hz(frequency: f64) -> String {
    if frequency >= 10_000.0 {
        format!("{:.0}", frequency)
    } else {
        format!("{frequency:.1}")
    }
}

/// The nearest note with the distance in cents: `A4`, `C♯5 +12¢`.
fn note(frequency: f64) -> String {
    if !(16.0..=20_000.0).contains(&frequency) {
        return String::new();
    }
    let semitones = 12.0 * (frequency / 440.0).log2() + 69.0;
    let midi = semitones.round();
    let cents = ((semitones - midi) * 100.0).round() as i32;
    let midi = midi as i32;
    let name = format!("{}{}", NOTES[midi.rem_euclid(12) as usize], midi / 12 - 1);
    if cents.abs() < 3 {
        name
    } else {
        format!("{name} {cents:+}¢")
    }
}

/// The channel's recent output (0–15) as a line.
fn scope(ui: &mut egui::Ui, gb: &GameBoy, n: usize, dim: bool) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 44.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let samples: Vec<u8> = gb.sound_scope(n).collect();
    if samples.is_empty() {
        return;
    }
    let step = rect.width() / (samples.len() - 1).max(1) as f32;
    let points: Vec<egui::Pos2> = samples
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            pos2(
                rect.left() + i as f32 * step,
                rect.bottom() - 3.0 - f32::from(v) / 15.0 * (rect.height() - 6.0),
            )
        })
        .collect();
    let color = if dim {
        Color32::from_gray(0x60)
    } else {
        COLORS[n]
    };
    painter.add(egui::Shape::line(points, Stroke::new(1.2, color)));
}

/// Channel 3's 32 samples as bars.
fn wave_ram(ui: &mut egui::Ui, wave: &[u8; 32]) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("wave RAM").small().weak());
        let (rect, _) = ui.allocate_exact_size(vec2(32.0 * 6.0, 32.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
        for (i, &v) in wave.iter().enumerate() {
            let x = rect.left() + i as f32 * 6.0;
            let h = (f32::from(v) + 1.0) / 16.0 * rect.height();
            painter.rect_filled(
                egui::Rect::from_min_max(
                    pos2(x + 1.0, rect.bottom() - h),
                    pos2(x + 5.0, rect.bottom()),
                ),
                0.0,
                COLORS[2],
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_and_cents() {
        assert_eq!(note(440.0), "A4");
        assert_eq!(note(439.8), "A4");
        assert_eq!(note(261.63), "C4");
        assert_eq!(note(277.18 * 1.01), "C♯4 +17¢");
        assert_eq!(note(5.0), "");
    }
}
