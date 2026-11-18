//! Tool windows: the cheat manager, memory search, debugger and video memory viewers each
//! open as a window of their own.

use eframe::egui;

/// Shows a tool window (cheats, memory search, debugger, viewers) as a window of its own, which can be moved
/// and sized independently of the game's; where the platform cannot open more windows,
/// inside the main window with a Close button. Returns whether it is still open.
pub fn tool_window(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    size: [f32; 2],
    mut contents: impl FnMut(&mut egui::Ui),
) -> bool {
    let builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(size)
        .with_min_inner_size([320.0, 240.0]);
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(id), builder, |ui, class| {
        if class == egui::ViewportClass::EmbeddedWindow {
            let close = ui.button("Close").clicked();
            contents(ui);
            !close
        } else {
            egui::CentralPanel::default().show(ui, |ui| contents(ui));
            !ui.ctx().input(|i| i.viewport().close_requested())
        }
    })
}
