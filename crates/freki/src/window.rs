//! Tool windows: the cheat manager, memory search, debugger, video memory viewers and
//! printer each open as a window of their own. They reopen where they were last (position
//! and size are kept with the settings), and the emulator's hotkeys work in them too.

use eframe::egui::{self, Event, Key, Modifiers, ViewportId};

/// Hotkeys passed from a focused tool window to the main window: the function keys, P
/// (pause) and M (mute), plus Ctrl+O / Ctrl+Q / Ctrl+R. Game buttons and the keys held down (rewind,
/// fast-forward) stay with the main window.
const FORWARDED: [Key; 11] = [
    Key::M,
    Key::F4,
    Key::F5,
    Key::F6,
    Key::F7,
    Key::F8,
    Key::F9,
    Key::F10,
    Key::F11,
    Key::F12,
    Key::P,
];
const FORWARDED_COMMANDS: [Key; 3] = [Key::O, Key::Q, Key::R];

/// Frames after opening before a window's geometry is trusted and remembered (the first
/// ones can report where the window manager has not put it yet).
const SETTLE_FRAMES: u64 = 3;

fn forwarded_id() -> egui::Id {
    ViewportId::ROOT.0.with("forwarded hotkeys")
}

/// Where a tool window was last: outer position (unknown on Wayland) and inner size.
type Geometry = (Option<[f32; 2]>, [f32; 2]);

/// Shows a tool window as a window of its own, which can be moved and sized independently
/// of the game's; where the platform cannot open more windows, inside the main window with
/// a Close button. Returns whether it is still open.
pub fn tool_window(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    size: [f32; 2],
    mut contents: impl FnMut(&mut egui::Ui),
) -> bool {
    let viewport = ViewportId::from_hash_of(id);
    let geometry_id = viewport.0.with("geometry");
    let opened_id = viewport.0.with("opened");
    let frame = ctx.cumulative_frame_nr_for(ViewportId::ROOT);

    // The geometry to open with is read once when the window opens; giving the builder a
    // new position every frame would fight the player dragging it.
    let (opened_at, start) =
        ctx.data_mut(
            |d| match d.get_temp::<(u64, u64, Option<Geometry>)>(opened_id) {
                Some((opened, last, start)) if last + 1 >= frame => (opened, start),
                _ => (frame, d.get_persisted::<Geometry>(geometry_id)),
            },
        );
    ctx.data_mut(|d| d.insert_temp(opened_id, (opened_at, frame, start)));

    let mut builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(start.map_or(size, |(_, size)| size))
        .with_min_inner_size([320.0, 240.0]);
    if let Some((Some(position), _)) = start {
        builder = builder.with_position(position);
    }
    ctx.show_viewport_immediate(viewport, builder, |ui, class| {
        if class == egui::ViewportClass::EmbeddedWindow {
            let close = ui.button("Close").clicked();
            contents(ui);
            return !close;
        }
        egui::CentralPanel::default().show(ui, |ui| contents(ui));
        let ctx = ui.ctx().clone();
        if frame >= opened_at + SETTLE_FRAMES {
            remember_geometry(&ctx, geometry_id);
        }
        forward_hotkeys(&ctx);
        if ctx.input(|i| i.viewport().focused == Some(true)) {
            // The main window's frame count: each window counts its own frames.
            let frame = ctx.cumulative_frame_nr_for(ViewportId::ROOT);
            ctx.data_mut(|d| d.insert_temp(tool_focus_id(), frame));
        }
        !ctx.input(|i| i.viewport().close_requested())
    })
}

fn remember_geometry(ctx: &egui::Context, id: egui::Id) {
    let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
    let Some(inner) = inner else {
        return;
    };
    let geometry: Geometry = (
        outer.map(|r| [r.min.x, r.min.y]),
        [inner.width(), inner.height()],
    );
    if ctx.data_mut(|d| d.get_persisted::<Geometry>(id)) != Some(geometry) {
        ctx.data_mut(|d| d.insert_persisted(id, geometry));
    }
}

/// Queues the hotkeys pressed in this (tool) window for the main window, unless the
/// player is typing into a text field here.
fn forward_hotkeys(ctx: &egui::Context) {
    if ctx.text_edit_focused() {
        return;
    }
    let keys: Vec<(Modifiers, Key)> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|event| match event {
                Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if FORWARDED.contains(key)
                    || (modifiers.command && FORWARDED_COMMANDS.contains(key)) =>
                {
                    Some((*modifiers, *key))
                }
                _ => None,
            })
            .collect()
    });
    if keys.is_empty() {
        return;
    }
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(Modifiers, Key)>>(forwarded_id())
            .extend(keys)
    });
    ctx.request_repaint_of(ViewportId::ROOT);
}

fn tool_focus_id() -> egui::Id {
    ViewportId::ROOT.0.with("tool window focused")
}

/// Whether the main window or a tool window has keyboard focus (on platforms that don't
/// say, it counts as focused).
pub fn any_window_focused(ctx: &egui::Context) -> bool {
    if ctx.input(|i| i.viewport().focused != Some(false)) {
        return true;
    }
    let frame = ctx.cumulative_frame_nr_for(ViewportId::ROOT);
    ctx.data(|d| d.get_temp::<u64>(tool_focus_id()))
        .is_some_and(|seen| seen + 2 >= frame)
}

/// Hotkeys pressed in tool windows since the last call, with their modifiers.
pub fn take_forwarded_hotkeys(ctx: &egui::Context) -> Vec<(Modifiers, Key)> {
    ctx.data_mut(|d| d.remove_temp::<Vec<(Modifiers, Key)>>(forwarded_id()))
        .unwrap_or_default()
}
