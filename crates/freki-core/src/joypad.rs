//! Joypad register P1/JOYP ($FF00).

use crate::bus::interrupt;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Right,
    Left,
    Up,
    Down,
    A,
    B,
    Select,
    Start,
}

#[derive(Serialize, Deserialize)]
pub struct Joypad {
    /// Pressed buttons; bit set = pressed. Low nibble d-pad, high nibble actions.
    pressed: u8,
    /// Selection bits 4–5 as last written (0 = selected).
    select: u8,
    irq: bool,
}

impl Default for Joypad {
    fn default() -> Self {
        Self::new()
    }
}

impl Joypad {
    pub fn new() -> Self {
        Self {
            pressed: 0,
            select: 0x00, // as the boot ROM leaves it (P1 reads $CF)
            irq: false,
        }
    }

    fn mask(button: Button) -> u8 {
        match button {
            Button::Right => 0x01,
            Button::Left => 0x02,
            Button::Up => 0x04,
            Button::Down => 0x08,
            Button::A => 0x10,
            Button::B => 0x20,
            Button::Select => 0x40,
            Button::Start => 0x80,
        }
    }

    pub fn set(&mut self, button: Button, pressed: bool) {
        let mask = Self::mask(button);
        if pressed && self.pressed & mask == 0 {
            self.irq = true;
        }
        if pressed {
            self.pressed |= mask;
        } else {
            self.pressed &= !mask;
        }
    }

    pub fn read(&self) -> u8 {
        let mut low = 0x0F;
        if self.select & 0x10 == 0 {
            low &= !(self.pressed & 0x0F);
        }
        if self.select & 0x20 == 0 {
            low &= !(self.pressed >> 4);
        }
        0xC0 | self.select | low
    }

    /// The selection bits 4–5 as last written (0 = selected).
    pub fn select(&self) -> u8 {
        self.select
    }

    pub fn write(&mut self, value: u8) {
        self.select = value & 0x30;
    }

    pub fn take_interrupt(&mut self) -> u8 {
        if std::mem::take(&mut self.irq) {
            interrupt::JOYPAD
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_selected_group_active_low() {
        let mut j = Joypad::new();
        j.set(Button::Start, true);
        j.set(Button::Left, true);
        j.write(0x10); // select action buttons
        assert_eq!(j.read() & 0x0F, 0b0111);
        j.write(0x20); // select d-pad
        assert_eq!(j.read() & 0x0F, 0b1101);
    }
}
