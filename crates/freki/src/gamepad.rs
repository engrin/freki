//! Gamepad input through gilrs. Controllers can be plugged in or removed at any time.
//!
//! Buttons are mapped by position, like on Nintendo controllers: the right face button
//! (East) is A and the bottom one (South) is B. The D-pad and left stick both steer.

use freki_core::Button;
use gilrs::{Axis, Button as Pad, Gilrs};

/// How far the left stick must be pushed to count as a D-pad press.
const STICK_THRESHOLD: f32 = 0.5;

pub struct Gamepads {
    /// `None` if the platform's gamepad backend could not be opened.
    gilrs: Option<Gilrs>,
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
        Self { gilrs }
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
                    _ => {}
                }
            }
        }
    }

    /// Whether `button` is held on any connected controller.
    pub fn is_pressed(&self, button: Button) -> bool {
        let Some(gilrs) = &self.gilrs else {
            return false;
        };
        gilrs.gamepads().any(|(_, pad)| {
            let stick_x = pad.value(Axis::LeftStickX);
            let stick_y = pad.value(Axis::LeftStickY); // positive is up
            match button {
                Button::Right => pad.is_pressed(Pad::DPadRight) || stick_x > STICK_THRESHOLD,
                Button::Left => pad.is_pressed(Pad::DPadLeft) || stick_x < -STICK_THRESHOLD,
                Button::Up => pad.is_pressed(Pad::DPadUp) || stick_y > STICK_THRESHOLD,
                Button::Down => pad.is_pressed(Pad::DPadDown) || stick_y < -STICK_THRESHOLD,
                Button::A => pad.is_pressed(Pad::East),
                Button::B => pad.is_pressed(Pad::South),
                Button::Select => pad.is_pressed(Pad::Select),
                Button::Start => pad.is_pressed(Pad::Start),
            }
        })
    }
}
