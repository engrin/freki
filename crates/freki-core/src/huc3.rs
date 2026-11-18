//! HuC3's real-time clock, reached through a small command protocol rather than
//! registers.
//!
//! $0000–$1FFF picks what $A000–$BFFF does:
//! - $0A: RAM, read and write;
//! - $0B: writes are clock commands;
//! - $0C: reads return the last command's result;
//! - $0D: reads say whether the clock is ready (bit 0);
//! - $0E: the infrared port;
//! - anything else: RAM, read only.
//!
//! A command byte holds the command in bits 4–6 and an argument in bits 0–3. The clock has
//! 256 nibbles of memory behind an index: 1 reads the nibble at the index (and moves on),
//! 3 writes the argument there (and moves on), 4 and 5 set the index's low and high
//! nibble, and 6 runs an extended command: 0 copies the time into nibbles 0–6, 1 sets the
//! time from them, 2 reports ready. The time is minutes into the day (nibbles 0–2) and a
//! day count (nibbles 3–6), least significant nibble first.

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

use crate::CLOCK_HZ;

const MINUTES_PER_DAY: u32 = 24 * 60;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct HuC3Clock {
    #[serde(with = "BigArray")]
    memory: [u8; 256],
    index: u8,
    /// Last command and its result nibble, read back in mode $0C.
    response: u8,
    minutes: u32,
    days: u32,
    /// T-cycles into the current minute.
    cycles: u64,
}

impl Default for HuC3Clock {
    fn default() -> Self {
        Self {
            memory: [0; 256],
            index: 0,
            response: 0,
            minutes: 0,
            days: 0,
            cycles: 0,
        }
    }
}

impl HuC3Clock {
    /// Advances the clock by `cycles` T-cycles (at normal speed).
    pub fn tick(&mut self, cycles: u32) {
        self.cycles += u64::from(cycles);
        let minute = u64::from(CLOCK_HZ) * 60;
        while self.cycles >= minute {
            self.cycles -= minute;
            self.minutes += 1;
            if self.minutes == MINUTES_PER_DAY {
                self.minutes = 0;
                self.days = (self.days + 1) & 0xFFFF;
            }
        }
    }

    /// A command written in mode $0B.
    pub fn command(&mut self, value: u8) {
        let arg = value & 0x0F;
        let command = (value >> 4) & 0x07;
        match command {
            1 => {
                let nibble = self.memory[self.index as usize];
                self.index = self.index.wrapping_add(1);
                self.response = command << 4 | nibble;
            }
            3 => {
                self.memory[self.index as usize] = arg;
                self.index = self.index.wrapping_add(1);
            }
            4 => self.index = (self.index & 0xF0) | arg,
            5 => self.index = (self.index & 0x0F) | arg << 4,
            6 => match arg {
                0 => {
                    let time = self.minutes | self.days << 12;
                    for (i, nibble) in self.memory[..7].iter_mut().enumerate() {
                        *nibble = (time >> (4 * i)) as u8 & 0x0F;
                    }
                }
                1 => {
                    let time =
                        (0..7).fold(0u32, |acc, i| acc | u32::from(self.memory[i]) << (4 * i));
                    self.minutes = (time & 0xFFF) % MINUTES_PER_DAY;
                    self.days = time >> 12 & 0xFFFF;
                    self.cycles = 0;
                }
                2 => self.response = command << 4 | 1,
                // $E rings the tone generator, which freki does not play.
                _ => {}
            },
            _ => {}
        }
    }

    /// The last command's result (mode $0C).
    pub fn response(&self) -> u8 {
        self.response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_time_runs_and_reads_back() {
        let mut clock = HuC3Clock::default();
        // Index 0, then write minutes = 1439 ($59F) and days = 2.
        clock.command(0x40);
        clock.command(0x50);
        for nibble in [0xF, 0x9, 0x5, 0x2, 0x0, 0x0, 0x0] {
            clock.command(0x30 | nibble);
        }
        clock.command(0x61);
        // One minute later it is day 3, minute 0.
        clock.tick(CLOCK_HZ * 60);
        clock.command(0x60);
        clock.command(0x40);
        let nibbles: Vec<u8> = (0..7)
            .map(|_| {
                clock.command(0x10);
                assert_eq!(clock.response() >> 4, 1);
                clock.response() & 0x0F
            })
            .collect();
        assert_eq!(nibbles, [0, 0, 0, 3, 0, 0, 0]);
        clock.command(0x62);
        assert_eq!(clock.response() & 1, 1, "ready");
    }
}
