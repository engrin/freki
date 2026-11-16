//! Gamepad input through gilrs. Controllers can be plugged in or removed at any time.
//!
//! Which controller button presses which Game Boy button is configurable (see `input`);
//! the left stick always steers as well.

use freki_core::Button;
use gilrs::{Axis, Button as Pad, Gilrs};

/// How far the left stick must be pushed to count as a D-pad press.
const STICK_THRESHOLD: f32 = 0.5;

pub struct Gamepads {
    /// `None` if the platform's gamepad backend could not be opened.
    gilrs: Option<Gilrs>,
    /// Most recent button press, for binding buttons in the controls window.
    last_pressed: Option<Pad>,
}

impl Gamepads {
    pub fn open() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(gilrs) => Some(gilrs),
            Err(err) => {
                eprintln!("freki: no gamepad support ({err})");
                None
            }
        };
        Self {
            gilrs,
            last_pressed: None,
        }
    }

    /// Processes pending controller events; call once per frame before reading buttons.
    pub fn update(&mut self) {
        if let Some(gilrs) = &mut self.gilrs {
            while let Some(event) = gilrs.next_event() {
                match event.event {
                    gilrs::EventType::Connected => {
                        eprintln!(
                            "freki: gamepad connected: {}",
                            gilrs.gamepad(event.id).name()
                        );
                    }
                    gilrs::EventType::Disconnected => eprintln!("freki: gamepad disconnected"),
                    gilrs::EventType::ButtonPressed(button, _) => {
                        self.last_pressed = Some(button);
                    }
                    _ => {}
                }
            }
        }
    }

    /// The button pressed since the last call, if any (call `update` first).
    pub fn take_pressed(&mut self) -> Option<Pad> {
        self.last_pressed.take()
    }

    /// Whether Game Boy `button`, bound to controller button `bound`, is held on any
    /// connected controller (directions also by the left stick).
    pub fn is_pressed(&self, button: Button, bound: Pad) -> bool {
        let Some(gilrs) = &self.gilrs else {
            return false;
        };
        gilrs.gamepads().any(|(_, pad)| {
            let stick_x = pad.value(Axis::LeftStickX);
            let stick_y = pad.value(Axis::LeftStickY); // positive is up
            let stick = match button {
                Button::Right => stick_x > STICK_THRESHOLD,
                Button::Left => stick_x < -STICK_THRESHOLD,
                Button::Up => stick_y > STICK_THRESHOLD,
                Button::Down => stick_y < -STICK_THRESHOLD,
                _ => false,
            };
            stick || pad.is_pressed(bound)
        })
    }
}
