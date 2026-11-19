//! Configurable controls: which keyboard key and gamepad button press each Game Boy
//! button. Stored by name in the settings; the left stick always steers as well.

use std::collections::BTreeMap;

use eframe::egui::Key;
use freki_core::Button;
use gilrs::Button as Pad;
use serde::{Deserialize, Serialize};

/// The Game Boy's buttons, in the order the controls window lists them.
pub const BUTTONS: [Button; 8] = [
    Button::Up,
    Button::Down,
    Button::Left,
    Button::Right,
    Button::A,
    Button::B,
    Button::Start,
    Button::Select,
];

/// Something a key or gamepad button can be bound to: a Game Boy button, or a turbo
/// button that presses A or B over and over while held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Button(Button),
    /// Turbo A or turbo B (only those two).
    Turbo(Button),
}

impl From<Button> for Control {
    fn from(button: Button) -> Self {
        Control::Button(button)
    }
}

/// The turbo buttons, listed below the console in the controls window.
pub const TURBOS: [Control; 2] = [Control::Turbo(Button::A), Control::Turbo(Button::B)];

/// Every control: the Game Boy's buttons, then the turbo buttons.
const CONTROLS: [Control; 10] = [
    Control::Button(Button::Up),
    Control::Button(Button::Down),
    Control::Button(Button::Left),
    Control::Button(Button::Right),
    Control::Button(Button::A),
    Control::Button(Button::B),
    Control::Button(Button::Start),
    Control::Button(Button::Select),
    TURBOS[0],
    TURBOS[1],
];

/// Turbo buttons alternate between pressed and released every this many frames (15
/// presses a second): slow enough for games that read the buttons only every few frames.
pub const TURBO_FRAMES: u32 = 2;

/// Whether a turbo button held for `held_frames` frames (0 = the first) presses now.
pub fn turbo_pressed(held_frames: u32) -> bool {
    (held_frames / TURBO_FRAMES).is_multiple_of(2)
}

/// Keys freki uses itself, which cannot be bound to a Game Boy button.
pub const RESERVED_KEYS: [(Key, &str); 12] = [
    (Key::P, "Pause"),
    (Key::M, "Mute"),
    (Key::R, "Rewind"),
    (Key::Tab, "Fast-forward"),
    (Key::F5, "Save state"),
    (Key::F6, "Previous slot"),
    (Key::F7, "Next slot"),
    (Key::F8, "Load state"),
    (Key::F9, "Cheats on/off"),
    (Key::F10, "Debugger: continue"),
    (Key::F11, "Debugger: step"),
    (Key::F12, "Screenshot"),
];

/// Gamepad buttons that can be bound, with the names used in the settings.
const PAD_BUTTONS: [(Pad, &str); 17] = [
    (Pad::South, "South"),
    (Pad::East, "East"),
    (Pad::North, "North"),
    (Pad::West, "West"),
    (Pad::LeftTrigger, "L1"),
    (Pad::RightTrigger, "R1"),
    (Pad::LeftTrigger2, "L2"),
    (Pad::RightTrigger2, "R2"),
    (Pad::Select, "Select"),
    (Pad::Start, "Start"),
    (Pad::Mode, "Mode"),
    (Pad::LeftThumb, "Left stick"),
    (Pad::RightThumb, "Right stick"),
    (Pad::DPadUp, "D-pad up"),
    (Pad::DPadDown, "D-pad down"),
    (Pad::DPadLeft, "D-pad left"),
    (Pad::DPadRight, "D-pad right"),
];

pub fn button_name(button: Button) -> &'static str {
    match button {
        Button::Up => "Up",
        Button::Down => "Down",
        Button::Left => "Left",
        Button::Right => "Right",
        Button::A => "A",
        Button::B => "B",
        Button::Start => "Start",
        Button::Select => "Select",
    }
}

/// "A", "Start", "Turbo A", …
pub fn control_name(control: Control) -> &'static str {
    match control {
        Control::Button(button) => button_name(button),
        Control::Turbo(Button::B) => "Turbo B",
        Control::Turbo(_) => "Turbo A",
    }
}

pub fn pad_name(pad: Pad) -> &'static str {
    PAD_BUTTONS
        .iter()
        .find(|(p, _)| *p == pad)
        .map_or("?", |(_, name)| name)
}

fn pad_from_name(name: &str) -> Option<Pad> {
    PAD_BUTTONS
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(p, _)| *p)
}

/// Whether a gamepad button can be bound (and is named in the settings).
pub fn bindable_pad(pad: Pad) -> bool {
    PAD_BUTTONS.iter().any(|(p, _)| *p == pad)
}

/// What a reserved key does, if `key` is one.
pub fn reserved(key: Key) -> Option<&'static str> {
    RESERVED_KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, what)| *what)
}

fn default_key(control: Control) -> Key {
    let button = match control {
        Control::Button(button) => button,
        // Next to X (A) and Z (B).
        Control::Turbo(Button::B) => return Key::A,
        Control::Turbo(_) => return Key::S,
    };
    match button {
        Button::Up => Key::ArrowUp,
        Button::Down => Key::ArrowDown,
        Button::Left => Key::ArrowLeft,
        Button::Right => Key::ArrowRight,
        Button::A => Key::X,
        Button::B => Key::Z,
        Button::Start => Key::Enter,
        Button::Select => Key::Backspace,
    }
}

/// Nintendo layout: the right face button is A, the bottom one B; turbo A is the top one,
/// turbo B the left one.
fn default_pad(control: Control) -> Pad {
    let button = match control {
        Control::Button(button) => button,
        Control::Turbo(Button::B) => return Pad::West,
        Control::Turbo(_) => return Pad::North,
    };
    match button {
        Button::Up => Pad::DPadUp,
        Button::Down => Pad::DPadDown,
        Button::Left => Pad::DPadLeft,
        Button::Right => Pad::DPadRight,
        Button::A => Pad::East,
        Button::B => Pad::South,
        Button::Start => Pad::Start,
        Button::Select => Pad::Select,
    }
}

/// Bindings as stored in the settings: control name (see [`control_name`]) → key or
/// gamepad button name.
/// Missing or unknown entries fall back to the defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bindings {
    keys: BTreeMap<String, String>,
    pad: BTreeMap<String, String>,
}

impl Bindings {
    pub fn key(&self, control: impl Into<Control>) -> Key {
        let control = control.into();
        self.keys
            .get(control_name(control))
            .and_then(|name| Key::from_name(name))
            .unwrap_or_else(|| default_key(control))
    }

    pub fn pad(&self, control: impl Into<Control>) -> Pad {
        let control = control.into();
        self.pad
            .get(control_name(control))
            .and_then(|name| pad_from_name(name))
            .unwrap_or_else(|| default_pad(control))
    }

    /// Binds `key` to `control`; a control that had this key gets `control`'s old key, so
    /// no key presses two buttons.
    pub fn set_key(&mut self, control: Control, key: Key) {
        let old = self.key(control);
        for other in CONTROLS {
            if other != control && self.key(other) == key {
                self.keys
                    .insert(control_name(other).into(), old.name().into());
            }
        }
        self.keys
            .insert(control_name(control).into(), key.name().into());
    }

    /// Binds `pad` to `control`, swapping like [`Bindings::set_key`].
    pub fn set_pad(&mut self, control: Control, pad: Pad) {
        let old = self.pad(control);
        for other in CONTROLS {
            if other != control && self.pad(other) == pad {
                self.pad
                    .insert(control_name(other).into(), pad_name(old).into());
            }
        }
        self.pad
            .insert(control_name(control).into(), pad_name(pad).into());
    }

    pub fn keys_default(&self) -> bool {
        CONTROLS.iter().all(|&c| self.key(c) == default_key(c))
    }

    pub fn pads_default(&self) -> bool {
        CONTROLS.iter().all(|&c| self.pad(c) == default_pad(c))
    }

    pub fn reset_keys(&mut self) {
        self.keys.clear();
    }

    pub fn reset_pads(&mut self) {
        self.pad.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_swapping() {
        let mut b = Bindings::default();
        assert_eq!(b.key(Button::A), Key::X);
        assert!(b.keys_default() && b.pads_default());
        b.set_key(Button::A.into(), Key::Z); // Z was B: B gets X
        assert_eq!((b.key(Button::A), b.key(Button::B)), (Key::Z, Key::X));
        b.set_pad(Button::Start.into(), Pad::East); // East was A: A gets Start
        assert_eq!(b.pad(Button::Start), Pad::East);
        assert_eq!(b.pad(Button::A), Pad::Start);
        assert!(!(b.keys_default() && b.pads_default()));
    }

    #[test]
    fn turbo_buttons_bind_and_swap_like_the_others() {
        let mut b = Bindings::default();
        let turbo_a = TURBOS[0];
        assert_eq!((b.key(turbo_a), b.pad(turbo_a)), (Key::S, Pad::North));
        assert_eq!((b.key(TURBOS[1]), b.pad(TURBOS[1])), (Key::A, Pad::West));
        b.set_key(turbo_a, Key::X); // X was A: A gets S
        assert_eq!((b.key(turbo_a), b.key(Button::A)), (Key::X, Key::S));
        assert!(!b.keys_default());
        b.reset_keys();
        assert!(b.keys_default());
    }

    #[test]
    fn turbo_presses_two_frames_on_two_off() {
        let pattern: Vec<bool> = (0..8).map(turbo_pressed).collect();
        assert_eq!(
            pattern,
            [true, true, false, false, true, true, false, false]
        );
    }

    #[test]
    fn unknown_names_fall_back_to_defaults() {
        let mut b = Bindings::default();
        b.keys.insert("A".into(), "NoSuchKey".into());
        b.pad.insert("B".into(), "Nonsense".into());
        assert_eq!((b.key(Button::A), b.pad(Button::B)), (Key::X, Pad::South));
    }
}
