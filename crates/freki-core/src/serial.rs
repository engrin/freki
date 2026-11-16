//! Serial port SB/SC ($FF01/$FF02), with no link partner attached.
//!
//! Every byte the game sends is recorded in [`Serial::output`]. Test ROMs such as
//! Blargg's print their results this way, so tests can run without a screen.

use crate::bus::interrupt;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Serial {
    data: u8,
    control: u8,
    output: Vec<u8>,
    irq: bool,
}

impl Default for Serial {
    fn default() -> Self {
        Self::new()
    }
}

impl Serial {
    pub fn new() -> Self {
        Self {
            data: 0,
            control: 0x7E,
            output: Vec::new(),
            irq: false,
        }
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF01 => self.data,
            _ => self.control | 0x7E,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF01 => self.data = value,
            _ => {
                self.control = value;
                // Transfer requested with the internal clock: complete it immediately.
                // With nothing connected, the incoming bits are all 1s.
                if value & 0x81 == 0x81 {
                    self.output.push(self.data);
                    self.data = 0xFF;
                    self.control &= !0x80;
                    self.irq = true;
                }
            }
        }
    }

    /// Every byte sent over the serial port so far.
    pub fn output(&self) -> &[u8] {
        &self.output
    }

    pub fn take_interrupt(&mut self) -> u8 {
        if std::mem::take(&mut self.irq) {
            interrupt::SERIAL
        } else {
            0
        }
    }
}
