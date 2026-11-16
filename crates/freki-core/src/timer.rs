//! DIV/TIMA/TMA/TAC timer ($FF04–$FF07).

use crate::bus::interrupt;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default)]
pub struct Timer {
    /// Internal 16-bit counter; DIV is its upper byte.
    counter: u16,
    tima: u8,
    tma: u8,
    tac: u8,
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
            0xFF04 => self.counter = 0,
            0xFF05 => self.tima = value,
            0xFF06 => self.tma = value,
            0xFF07 => self.tac = value & 0x07,
            _ => {}
        }
    }

    /// Bit of the internal counter whose falling edge increments TIMA.
    fn tima_bit(&self) -> u16 {
        match self.tac & 0x03 {
            0 => 9, // 4096 Hz
            1 => 3, // 262144 Hz
            2 => 5, // 65536 Hz
            _ => 7, // 16384 Hz
        }
    }

    /// Advances the timer by `cycles` T-cycles. Returns interrupt bits to raise.
    pub fn tick(&mut self, cycles: u32) -> u8 {
        let mut irq = 0;
        for _ in 0..cycles {
            let old = self.counter;
            self.counter = self.counter.wrapping_add(1);
            if self.tac & 0x04 == 0 {
                continue;
            }
            let bit = self.tima_bit();
            let fell = (old >> bit) & 1 == 1 && (self.counter >> bit) & 1 == 0;
            if fell {
                let (next, overflow) = self.tima.overflowing_add(1);
                if overflow {
                    self.tima = self.tma;
                    irq |= interrupt::TIMER;
                } else {
                    self.tima = next;
                }
            }
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
        assert_eq!(t.tick(16), interrupt::TIMER);
        assert_eq!(t.read(0xFF05), 0x42);
    }
}
