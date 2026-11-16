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

/// Keys freki uses itself, which cannot be bound to a Game Boy button.
pub const RESERVED_KEYS: [(Key, &str); 10] = [
    (Key::P, "Pause"),
    (Key::R, "Rewind"),
    (Key::Tab, "Fast-forward"),
    (Key::F5, "Save state"),
    (Key::F6, "Previous slot"),
    (Key::F7, "Next slot"),
    (Key::F8, "Load state"),
    (Key::F9, "Cheats on/off"),
    (Key::F10, "Debugger: continue"),
    (Key::F11, "Debugger: step"),
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

fn default_key(button: Button) -> Key {
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

/// Nintendo layout: the right face button is A, the bottom one B.
fn default_pad(button: Button) -> Pad {
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

/// Bindings as stored in the settings: Game Boy button name → key or gamepad button name.
/// Missing or unknown entries fall back to the defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bindings {
    keys: BTreeMap<String, String>,
    pad: BTreeMap<String, String>,
}

impl Bindings {
    pub fn key(&self, button: Button) -> Key {
        self.keys
            .get(button_name(button))
            .and_then(|name| Key::from_name(name))
            .unwrap_or_else(|| default_key(button))
    }

    pub fn pad(&self, button: Button) -> Pad {
        self.pad
            .get(button_name(button))
            .and_then(|name| pad_from_name(name))
            .unwrap_or_else(|| default_pad(button))
    }

    /// Binds `key` to `button`; a Game Boy button that had this key gets `button`'s old
    /// key, so no key presses two buttons.
    pub fn set_key(&mut self, button: Button, key: Key) {
        let old = self.key(button);
        for other in BUTTONS {
            if other != button && self.key(other) == key {
                self.keys
                    .insert(button_name(other).into(), old.name().into());
            }
        }
        self.keys
            .insert(button_name(button).into(), key.name().into());
    }

    /// Binds `pad` to `button`, swapping like [`Bindings::set_key`].
    pub fn set_pad(&mut self, button: Button, pad: Pad) {
        let old = self.pad(button);
        for other in BUTTONS {
            if other != button && self.pad(other) == pad {
                self.pad
                    .insert(button_name(other).into(), pad_name(old).into());
            }
        }
        self.pad
            .insert(button_name(button).into(), pad_name(pad).into());
    }

    pub fn is_default(&self) -> bool {
        BUTTONS
            .iter()
            .all(|&b| self.key(b) == default_key(b) && self.pad(b) == default_pad(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_swapping() {
        let mut b = Bindings::default();
        assert_eq!(b.key(Button::A), Key::X);
        assert!(b.is_default());
        b.set_key(Button::A, Key::Z); // Z was B: B gets X
        assert_eq!((b.key(Button::A), b.key(Button::B)), (Key::Z, Key::X));
        b.set_pad(Button::Start, Pad::North);
        assert_eq!(b.pad(Button::Start), Pad::North);
        assert!(!b.is_default());
    }

    #[test]
    fn unknown_names_fall_back_to_defaults() {
        let mut b = Bindings::default();
        b.keys.insert("A".into(), "NoSuchKey".into());
        b.pad.insert("B".into(), "Turbo".into());
        assert_eq!((b.key(Button::A), b.pad(Button::B)), (Key::X, Pad::South));
    }
}
