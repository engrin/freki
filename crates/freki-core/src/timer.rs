//! DIV/TIMA/TMA/TAC timer ($FF04–$FF07).

use crate::bus::interrupt;
use serde::{Deserialize, Serialize};

/// The timer is driven by a 16-bit counter (DIV is its upper byte). TIMA increments on
/// each falling edge of `TAC enable AND counter bit`; because that signal is checked
/// whenever it can change, writing DIV or TAC can also increment TIMA (a DMG quirk).
///
/// When TIMA overflows it reads $00 for one M-cycle; then it is loaded from TMA and the
/// interrupt is requested. Writing TIMA during that M-cycle cancels the reload; during
/// the reload M-cycle itself TIMA writes are ignored and TMA writes go to TIMA too.
#[derive(Serialize, Deserialize, Default)]
pub struct Timer {
    /// Internal 16-bit counter; DIV is its upper byte.
    counter: u16,
    tima: u8,
    tma: u8,
    tac: u8,
    /// T-cycles until an overflowed TIMA is reloaded (0 = no reload pending).
    overflow: u8,
    /// T-cycles left in the M-cycle in which TIMA was reloaded.
    reloaded: u8,
}

impl Timer {
    pub fn new() -> Self {
        Self {
            counter: 0xAB00,
            ..Default::default()
        }
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF04 => (self.counter >> 8) as u8,
            0xFF05 => self.tima,
            0xFF06 => self.tma,
            0xFF07 => self.tac | 0xF8,
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF04 => {
                let before = self.signal();
                self.counter = 0;
                self.edge(before);
            }
            0xFF05 => {
                if self.reloaded == 0 {
                    self.tima = value;
                    self.overflow = 0;
                }
            }
            0xFF06 => {
                self.tma = value;
                if self.reloaded > 0 {
                    self.tima = value;
                }
            }
            0xFF07 => {
                let before = self.signal();
                self.tac = value & 0x07;
                self.edge(before);
            }
            _ => {}
        }
    }

    /// The signal whose falling edge increments TIMA.
    fn signal(&self) -> bool {
        let bit = match self.tac & 0x03 {
            0 => 9, // 4096 Hz
            1 => 3, // 262144 Hz
            2 => 5, // 65536 Hz
            _ => 7, // 16384 Hz
        };
        self.tac & 0x04 != 0 && (self.counter >> bit) & 1 == 1
    }

    /// Increments TIMA if the signal fell since `before`.
    fn edge(&mut self, before: bool) {
        if before && !self.signal() {
            let (next, overflow) = self.tima.overflowing_add(1);
            self.tima = next;
            if overflow {
                self.overflow = 4;
            }
        }
    }

    /// Advances the timer by `cycles` T-cycles. Returns interrupt bits to raise.
    pub fn tick(&mut self, cycles: u32) -> u8 {
        let mut irq = 0;
        for _ in 0..cycles {
            self.reloaded = self.reloaded.saturating_sub(1);
            if self.overflow > 0 {
                self.overflow -= 1;
                if self.overflow == 0 {
                    self.tima = self.tma;
                    self.reloaded = 4;
                    irq |= interrupt::TIMER;
                }
            }
            let before = self.signal();
            self.counter = self.counter.wrapping_add(1);
            self.edge(before);
        }
        irq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tima_overflow_reloads_and_interrupts() {
        let mut t = Timer::new();
        t.write(0xFF07, 0b101); // enabled, 262144 Hz = every 16 T-cycles
        t.write(0xFF06, 0x42);
        t.write(0xFF05, 0xFF);
        t.write(0xFF04, 0);
        assert_eq!(t.tick(16), 0, "overflow: TIMA reads 0 for one M-cycle");
        assert_eq!(t.read(0xFF05), 0);
        assert_eq!(t.tick(4), interrupt::TIMER);
        assert_eq!(t.read(0xFF05), 0x42);
    }
}
