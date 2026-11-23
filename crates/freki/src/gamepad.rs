//! Gamepad input through gilrs. Controllers can be plugged in or removed at any time.
//!
//! Which controller button presses which Game Boy button is configurable (see `input`);
//! the left stick always steers as well.

use freki_core::Button;

use crate::controls::ControllerKind;
use gilrs::ff::{BaseEffect, BaseEffectType, Effect, EffectBuilder, Replay};
use gilrs::{Axis, Button as Pad, Gilrs};

/// How far the left stick must be pushed to count as a D-pad press.
const STICK_THRESHOLD: f32 = 0.5;

pub struct Gamepads {
    /// `None` if the platform's gamepad backend could not be opened.
    gilrs: Option<Gilrs>,
    /// Most recent button press, for binding buttons in the controls window.
    last_pressed: Option<Pad>,
    /// Constant vibration on every controller that supports it, built when first needed
    /// and dropped (which stops it) when controllers come and go; `None` if no controller
    /// can rumble.
    rumble: Option<Effect>,
    /// The vibration is playing (at strength > 0).
    rumbling: bool,
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
            rumble: None,
            rumbling: false,
        }
    }

    /// Processes pending controller events; call once per frame before reading buttons.
    pub fn update(&mut self) {
        if let Some(gilrs) = &mut self.gilrs {
            while let Some(event) = gilrs.next_event() {
                match event.event {
                    gilrs::EventType::Connected => {
                        (self.rumble, self.rumbling) = (None, false);
                        eprintln!(
                            "freki: gamepad connected: {}",
                            gilrs.gamepad(event.id).name()
                        );
                    }
                    gilrs::EventType::Disconnected => {
                        (self.rumble, self.rumbling) = (None, false);
                        eprintln!("freki: gamepad disconnected");
                    }
                    gilrs::EventType::ButtonPressed(button, _) => {
                        self.last_pressed = Some(button);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Vibrates the controllers at `strength` (0 to 1; 0 stops).
    pub fn set_rumble(&mut self, strength: f32) {
        if strength <= 0.0 {
            if std::mem::take(&mut self.rumbling)
                && let Some(effect) = &self.rumble
            {
                let _ = effect.stop();
            }
            return;
        }
        if self.rumble.is_none() {
            self.rumble = self.build_rumble();
        }
        let Some(effect) = &self.rumble else { return };
        let _ = effect.set_gain(strength.min(1.0));
        if !self.rumbling {
            self.rumbling = effect.play().is_ok();
        }
    }

    /// A constant strong vibration on all connected controllers that support it.
    fn build_rumble(&mut self) -> Option<Effect> {
        let gilrs = self.gilrs.as_mut()?;
        let ids: Vec<_> = gilrs
            .gamepads()
            .filter(|(_, pad)| pad.is_ff_supported())
            .map(|(id, _)| id)
            .collect();
        if ids.is_empty() {
            return None;
        }
        EffectBuilder::new()
            .add_effect(BaseEffect {
                kind: BaseEffectType::Strong {
                    magnitude: u16::MAX,
                },
                scheduling: Replay::default(),
                ..Default::default()
            })
            .gamepads(&ids)
            .finish(gilrs)
            .ok()
    }

    /// The left stick's position (x right, y up, each −1 to 1), from whichever connected
    /// controller has it pushed furthest.
    pub fn left_stick(&self) -> (f32, f32) {
        let Some(gilrs) = &self.gilrs else {
            return (0.0, 0.0);
        };
        gilrs
            .gamepads()
            .map(|(_, pad)| (pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY)))
            .max_by(|a, b| (a.0.hypot(a.1)).total_cmp(&b.0.hypot(b.1)))
            .unwrap_or((0.0, 0.0))
    }

    /// The first connected controller: its name, kind and whether it can rumble.
    pub fn connected(&self) -> Option<(String, ControllerKind, bool)> {
        let gilrs = self.gilrs.as_ref()?;
        let (_, pad) = gilrs.gamepads().next()?;
        let kind = ControllerKind::detect(pad.name(), pad.vendor_id(), pad.product_id());
        Some((pad.name().to_string(), kind, pad.is_ff_supported()))
    }

    /// The button pressed since the last call, if any (call `update` first).
    pub fn take_pressed(&mut self) -> Option<Pad> {
        self.last_pressed.take()
    }

    /// Whether controller button `bound` is held on the `index`th connected controller
    /// (counted from 0), or on any with `None`; or the left stick points the way of the
    /// Game Boy direction `stick`.
    pub fn is_pressed_on(&self, index: Option<usize>, stick: Option<Button>, bound: Pad) -> bool {
        let Some(gilrs) = &self.gilrs else {
            return false;
        };
        let mut pads = gilrs
            .gamepads()
            .enumerate()
            .filter(|&(i, _)| index.is_none_or(|n| n == i))
            .map(|(_, pad)| pad);
        pads.any(|(_, pad)| {
            let stick_x = pad.value(Axis::LeftStickX);
            let stick_y = pad.value(Axis::LeftStickY); // positive is up
            let stick = match stick {
                Some(Button::Right) => stick_x > STICK_THRESHOLD,
                Some(Button::Left) => stick_x < -STICK_THRESHOLD,
                Some(Button::Up) => stick_y > STICK_THRESHOLD,
                Some(Button::Down) => stick_y < -STICK_THRESHOLD,
                _ => false,
            };
            stick || pad.is_pressed(bound)
        })
    }
}
