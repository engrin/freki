//! Serial port SB/SC ($FF01/$FF02), with no link partner attached.
//!
//! A transfer with the internal clock advances on each falling edge of bit 7 of the timer's
//! counter (bit 2 with the CGB's fast clock): 16 half-steps, shifting one bit on every
//! second one, so it is aligned with DIV to a half bit (Gambatte's hwtests, serial/*).
//! With nothing connected, the incoming bits are all 1s. Transfers with the external clock
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
    /// Half-steps left in the running internal-clock transfer (0 = none).
    #[serde(default)]
    half_steps_left: u8,
    /// The running transfer uses the CGB's fast clock (SC bit 1, CGB mode only).
    #[serde(default)]
    fast: bool,
    /// The running transfer's interrupt was acknowledged early (see [`Serial::ack_early`]).
    #[serde(default)]
    acked: bool,
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
            half_steps_left: 0,
            fast: false,
            acked: false,
        }
    }

    /// `cgb`: CGB mode, where SC bit 1 (fast clock) exists.
    pub fn read(&self, addr: u16, cgb: bool) -> u8 {
        match addr {
            0xFF01 => self.data,
            _ if cgb => self.control | 0x7C,
            _ => self.control | 0x7E,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8, cgb: bool) {
        match addr {
            0xFF01 => self.data = value,
            _ => {
                self.control = value;
                if value & 0x81 == 0x81 {
                    self.output.push(self.data);
                    self.half_steps_left = 16;
                    self.fast = cgb && value & 0x02 != 0;
                    self.acked = false;
                } else {
                    self.half_steps_left = 0;
                }
            }
        }
    }

    /// Bit of the timer's counter whose falling edges clock the running transfer.
    pub fn clock_bit(&self) -> u32 {
        if self.fast { 2 } else { 7 }
    }

    /// Advances a running internal-clock transfer by `edges` falling edges of
    /// [`Serial::clock_bit`].
    pub fn clock(&mut self, edges: u32) {
        for _ in 0..edges {
            if self.half_steps_left == 0 {
                return;
            }
            self.half_steps_left -= 1;
            if self.half_steps_left % 2 == 0 {
                self.data = self.data << 1 | 1;
            }
            if self.half_steps_left == 0 {
                self.control &= !0x80;
                self.irq = !std::mem::take(&mut self.acked);
            }
        }
    }

    /// The CPU acknowledged a serial interrupt: a transfer finishing within the next `edges`
    /// clock edges is acknowledged with it.
    pub fn ack_early(&mut self, edges: u32) {
        if self.half_steps_left != 0 && u32::from(self.half_steps_left) <= edges {
            self.acked = true;
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
