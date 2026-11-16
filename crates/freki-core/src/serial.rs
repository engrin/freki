//! Serial port SB/SC ($FF01/$FF02), with no link partner attached.
//!
//! A transfer with the internal clock shifts one bit on each falling edge of bit 8 of the
//! timer's counter (8192 Hz), so it takes 8 such edges and is aligned with DIV. With
//! nothing connected, the incoming bits are all 1s. Transfers with the external clock
//! never finish.
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
    /// Bits left in the running internal-clock transfer (0 = none).
    bits_left: u8,
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
            bits_left: 0,
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
                if value & 0x81 == 0x81 {
                    self.output.push(self.data);
                    self.bits_left = 8;
                } else {
                    self.bits_left = 0;
                }
            }
        }
    }

    /// Shifts `edges` bits of a running internal-clock transfer (one per falling edge of
    /// the serial clock).
    pub fn clock(&mut self, edges: u32) {
        for _ in 0..edges {
            if self.bits_left == 0 {
                return;
            }
            self.data = self.data << 1 | 1;
            self.bits_left -= 1;
            if self.bits_left == 0 {
                self.control &= !0x80;
                self.irq = true;
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
