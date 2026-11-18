//! Serial port SB/SC ($FF01/$FF02).
//!
//! A transfer with the internal clock advances on each falling edge of bit 7 of the timer's
//! counter (bit 2 with the CGB's fast clock): 16 half-steps, shifting one bit on every
//! second one, so it is aligned with DIV to a half bit (Gambatte's hwtests, serial/*).
//! With nothing connected, the incoming bits are all 1s and transfers with the external
//! clock never finish. With a link cable (see [`crate::link`]) the partner's byte comes in,
//! and an external-clock transfer finishes when the partner's clock has sent 8 bits.
//!
//! The bytes the game sends are recorded in [`Serial::output`] (the latest
//! [`OUTPUT_LIMIT`] of them; not kept in save states). Test ROMs such as Blargg's print
//! their results this way, so tests can run without a screen.

use crate::bus::interrupt;
use serde::{Deserialize, Serialize};

/// Most bytes [`Serial::output`] keeps.
pub const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Serialize, Deserialize)]
pub struct Serial {
    data: u8,
    control: u8,
    /// Not saved: a long link-cable session would make every state bigger.
    #[serde(skip)]
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
    /// Link cable: the bits still to come in during the running internal-clock transfer,
    /// most significant first (1s once the partner's byte is in).
    #[serde(skip)]
    incoming: u16,
    /// Link cable: an internal-clock transfer started sending this byte (taken by the link).
    #[serde(skip)]
    started: Option<u8>,
    /// Link cable: the running internal-clock transfer finished (taken by the link).
    #[serde(skip)]
    finished: bool,
    /// Link cable: the byte the partner's clock is shifting into an external-clock transfer.
    #[serde(skip)]
    external_in: Option<u8>,
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
            incoming: 0xFFFF,
            started: None,
            finished: false,
            external_in: None,
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
                if value & 0x81 != 0x80 {
                    self.external_in = None;
                }
                if value & 0x81 == 0x81 {
                    self.record(self.data);
                    self.half_steps_left = 16;
                    self.fast = cgb && value & 0x02 != 0;
                    self.acked = false;
                    self.incoming = 0xFFFF;
                    self.started = Some(self.data);
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
                self.data = self.data << 1 | (self.incoming >> 15) as u8;
                self.incoming = self.incoming << 1 | 1;
            }
            if self.half_steps_left == 0 {
                self.control &= !0x80;
                self.irq = !std::mem::take(&mut self.acked);
                self.finished = true;
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

    /// Link cable: the byte an internal-clock transfer started sending since the last call.
    pub(crate) fn take_started(&mut self) -> Option<u8> {
        self.started.take()
    }

    /// Link cable: the partner's byte for the running internal-clock transfer. Bits that
    /// were already shifted in stay 1s.
    pub(crate) fn set_incoming(&mut self, byte: u8) {
        let shifted = 8 - u32::from(self.half_steps_left).div_ceil(2);
        self.incoming = ((u16::from(byte) << 8 | 0xFF) << shifted) | ((1 << shifted) - 1);
    }

    /// Link cable: whether the internal-clock transfer finished since the last call.
    pub(crate) fn take_finished(&mut self) -> bool {
        std::mem::take(&mut self.finished)
    }

    /// Link cable: whether an external-clock transfer is waiting for the partner's clock.
    pub(crate) fn external_ready(&self) -> bool {
        self.control & 0x81 == 0x80 && self.external_in.is_none()
    }

    /// Link cable: the partner's clock starts shifting `byte` in. Returns the byte that
    /// goes out.
    pub(crate) fn start_external(&mut self, byte: u8) -> u8 {
        self.external_in = Some(byte);
        self.record(self.data);
        self.data
    }

    /// Link cable: the partner's clock has shifted all 8 bits.
    pub(crate) fn finish_external(&mut self) {
        if let Some(byte) = self.external_in.take() {
            self.data = byte;
            self.control &= !0x80;
            self.irq = true;
        }
    }

    /// Adds a sent byte to [`Serial::output`], dropping the older half once it is full.
    fn record(&mut self, byte: u8) {
        if self.output.len() >= OUTPUT_LIMIT {
            self.output.drain(..OUTPUT_LIMIT / 2);
        }
        self.output.push(byte);
    }

    /// The bytes sent over the serial port: all of them, or the latest ones once more
    /// than [`OUTPUT_LIMIT`] were sent.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_keeps_the_latest_bytes() {
        let mut serial = Serial::new();
        for i in 0..OUTPUT_LIMIT + 10 {
            serial.record(i as u8);
        }
        let out = serial.output();
        assert!(out.len() <= OUTPUT_LIMIT);
        assert_eq!(out.last(), Some(&((OUTPUT_LIMIT + 9) as u8)));
    }
}
