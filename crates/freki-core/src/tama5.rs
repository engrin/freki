//! Bandai TAMA5: the cartridge of Game de Hakken!! Tamagotchi: Osutchi to Mesutchi
//! ("Tamagotchi 3"). Its registers follow mGBA's model; the clock chip ("TAMA6") keeps
//! the pet's time.
//!
//! Everything goes through two addresses, four bits at a time: $A001 selects a register,
//! $A000 writes its low nibble or reads it back.
//!
//! | Register | Write | Read |
//! |---|---|---|
//! | 0, 1 | ROM bank, low and high nibble | |
//! | 4, 5 | Data, low and high nibble | |
//! | 6, 7 | Address high bit and command (6), address low nibble (7); writing 7 runs it | |
//! | $A | | $F1: ready |
//! | $C, $D | | Result, low and high nibble |
//!
//! Register 6 bits 1–3 pick what the address means: 0 writes a byte of the 32-byte save
//! memory, 1 reads one, 2 runs a clock command (at the address), 4 reaches a clock
//! register (number in register 4, page in register 7).
//!
//! The clock has four pages of 13 nibbles: the time (seconds to years, in BCD digits),
//! the alarm (with the 24-hour and leap-year settings), and two free pages.

use serde::{Deserialize, Serialize};

use crate::CLOCK_HZ;

pub const RAM_BYTES: usize = 32;

// Time page nibbles.
const SECOND_1: usize = 0x0;
const SECOND_10: usize = 0x1;
const MINUTE_1: usize = 0x2;
const MINUTE_10: usize = 0x3;
const HOUR_1: usize = 0x4;
const HOUR_10: usize = 0x5;
const WEEK: usize = 0x6;
const DAY_1: usize = 0x7;
const DAY_10: usize = 0x8;
const MONTH_1: usize = 0x9;
const MONTH_10: usize = 0xA;
const YEAR_1: usize = 0xB;
const YEAR_10: usize = 0xC;
// Alarm page nibbles.
const HOUR_24: usize = 0xA;
const LEAP_YEAR: usize = 0xB;
/// On every page: bit 3 timer running, bit 2 alarm on, bits 0–1 the page number.
const PAGE: usize = 0xD;

/// Which bits of each time-page (first row) and alarm-page (second row) nibble exist.
const MASKS: [u8; 32] = [
    0xF, 0x7, 0xF, 0x7, 0xF, 0x3, 0x7, 0xF, 0x3, 0xF, 0x1, 0xF, 0xF, 0x0, 0x0, 0x0, //
    0x0, 0x0, 0xF, 0x7, 0xF, 0x3, 0x7, 0xF, 0x3, 0x0, 0x1, 0x3, 0x0, 0x0, 0x0, 0x0,
];

/// Size of the clock block at the end of the save file (mGBA's layout): the four pages,
/// two nibbles to a byte (8 bytes each), then the Unix time it was written at (64-bit
/// little-endian).
const SAVE_BYTES: usize = 40;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Tama5 {
    /// Register selected through $A001.
    reg: u8,
    registers: [u8; 8],
    rom_bank: u8,
    timer_stopped: bool,
    time: [u8; 16],
    alarm: [u8; 16],
    free0: [u8; 16],
    free1: [u8; 16],
    /// T-cycles into the current second.
    cycles: u32,
}

impl Default for Tama5 {
    fn default() -> Self {
        let mut alarm = [0; 16];
        let mut free0 = [0; 16];
        let mut free1 = [0; 16];
        alarm[PAGE] = 1;
        free0[PAGE] = 2;
        free1[PAGE] = 3;
        Self {
            reg: 0,
            registers: [0; 8],
            rom_bank: 1,
            timer_stopped: false,
            time: [0; 16],
            alarm,
            free0,
            free1,
            cycles: 0,
        }
    }
}

impl Tama5 {
    /// The ROM bank at $4000–$7FFF.
    pub fn rom_bank(&self) -> usize {
        usize::from(self.rom_bank)
    }

    /// The save-memory address the registers point at.
    fn address(&self) -> usize {
        usize::from(self.registers[6] << 4 & 0x10 | self.registers[7])
    }

    /// A write to $A000–$BFFF. Returns `true` if the save memory changed.
    pub fn write(&mut self, addr: u16, value: u8, ram: &mut [u8]) -> bool {
        if addr & 1 != 0 {
            self.reg = value;
            return false;
        }
        let reg = usize::from(self.reg);
        if reg >= self.registers.len() {
            return false;
        }
        self.registers[reg] = value & 0x0F;
        let data = self.registers[5] << 4 | self.registers[4];
        match reg {
            0 | 1 => self.rom_bank = self.registers[0] | self.registers[1] << 4,
            7 => match self.registers[6] >> 1 {
                0 => {
                    let i = self.address();
                    let changed = ram[i] != data;
                    ram[i] = data;
                    return changed;
                }
                2 => self.clock_command(self.address() as u8, data),
                4 => {
                    let nibble = usize::from(self.registers[4]);
                    if nibble < PAGE {
                        let value = self.registers[5];
                        match self.registers[7] {
                            0 => self.time[nibble] = value & MASKS[nibble],
                            2 => self.alarm[nibble] = value & MASKS[nibble | 0x10],
                            4 => self.free0[nibble] = value,
                            6 => self.free1[nibble] = value,
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
        false
    }

    fn set_page_bits(&mut self, keep: u8, set: u8) {
        for page in [
            &mut self.time,
            &mut self.alarm,
            &mut self.free0,
            &mut self.free1,
        ] {
            page[PAGE] = page[PAGE] & keep | set;
        }
    }

    fn clock_command(&mut self, command: u8, data: u8) {
        match command {
            0x00 => {
                self.timer_stopped = true;
                self.set_page_bits(0x7, 0);
            }
            0x01 => {
                self.timer_stopped = false;
                self.time[SECOND_1] = 0;
                self.time[SECOND_10] = 0;
                self.cycles = 0;
                self.set_page_bits(0xF, 0x8);
            }
            0x04 => {
                self.time[MINUTE_1] = data & 0x0F;
                self.time[MINUTE_10] = data >> 4;
            }
            0x05 => {
                self.time[HOUR_1] = data & 0x0F;
                self.time[HOUR_10] = data >> 4;
            }
            0x10 => self.set_page_bits(0xB, 0),
            0x11 => self.set_page_bits(0xF, 0x4),
            _ => {}
        }
    }

    /// A read from $A000–$BFFF.
    pub fn read(&self, addr: u16, ram: &[u8]) -> u8 {
        if addr & 1 != 0 {
            return 0xFF;
        }
        let high = match self.reg {
            0x0C => false,
            0x0D => true,
            _ => return 0xF1, // ready
        };
        let value = match self.registers[6] >> 1 {
            1 => ram[self.address()],
            2 => match self.address() {
                0x06 => self.time[MINUTE_10] << 4 | self.time[MINUTE_1],
                0x07 => self.time[HOUR_10] << 4 | self.time[HOUR_1],
                other => other as u8,
            },
            // Clock registers come back one nibble at a time, through the low half.
            4 if !high => match usize::from(self.registers[4]) {
                n if n > PAGE => 0,
                n => self.time[n],
            },
            _ => 0xF0,
        };
        let value = if high { value >> 4 } else { value };
        value | 0xF0
    }

    /// Advances the clock by `cycles` T-cycles (at normal speed).
    pub fn tick(&mut self, cycles: u32) {
        if self.timer_stopped {
            return;
        }
        self.cycles += cycles;
        while self.cycles >= CLOCK_HZ {
            self.cycles -= CLOCK_HZ;
            self.advance_seconds(1);
        }
    }

    /// Moves the time page on by `seconds`, carrying through the calendar.
    fn advance_seconds(&mut self, seconds: u64) {
        if self.timer_stopped || seconds == 0 {
            return;
        }
        let t = &mut self.time;
        let digits =
            |t: &[u8; 16], one: usize, ten: usize| u64::from(t[one]) + u64::from(t[ten]) * 10;

        let s = digits(t, SECOND_1, SECOND_10) + seconds;
        t[SECOND_1] = (s % 10) as u8;
        t[SECOND_10] = (s % 60 / 10) as u8;
        let m = digits(t, MINUTE_1, MINUTE_10) + s / 60;
        t[MINUTE_1] = (m % 10) as u8;
        t[MINUTE_10] = (m % 60 / 10) as u8;

        // 24-hour, or 12-hour with bit 1 of the tens digit meaning PM.
        let h24 = self.alarm[HOUR_24] != 0;
        let tens = u64::from(t[HOUR_10]);
        let hour = u64::from(t[HOUR_1])
            + if h24 {
                tens * 10
            } else {
                (tens & 1) * 10 + (tens >> 1 & 1) * 12
            };
        let h = hour + m / 60;
        let hour = h % 24;
        if h24 {
            t[HOUR_1] = (hour % 10) as u8;
            t[HOUR_10] = (hour / 10) as u8;
        } else {
            t[HOUR_1] = (hour % 12 % 10) as u8;
            t[HOUR_10] = (hour % 12 / 10 + hour / 12 * 2) as u8;
        }
        let days = h / 24;
        if days == 0 {
            return;
        }

        t[WEEK] = ((u64::from(t[WEEK]) + days) % 7) as u8;
        let mut leap = u64::from(self.alarm[LEAP_YEAR]);
        let mut year = digits(t, YEAR_1, YEAR_10);
        let month = digits(t, MONTH_1, MONTH_10).clamp(1, 12);
        let mut day_of_year = day_of_year(digits(t, DAY_1, DAY_10).max(1), month, leap) + days;
        while day_of_year > year_length(leap) {
            day_of_year -= year_length(leap);
            year += 1;
            leap += 1;
        }
        let (month, day) = month_and_day(day_of_year, leap);
        let year = year % 100;
        t[DAY_1] = (day % 10) as u8;
        t[DAY_10] = (day / 10) as u8;
        t[MONTH_1] = (month % 10) as u8;
        t[MONTH_10] = (month / 10) as u8;
        t[YEAR_1] = (year % 10) as u8;
        t[YEAR_10] = (year / 10) as u8;
        self.alarm[LEAP_YEAR] = (leap & 3) as u8;
    }

    /// The clock's block at the end of the save file, stamped with `now` (Unix seconds).
    pub fn to_save(&self, now: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(SAVE_BYTES);
        for page in [&self.time, &self.alarm, &self.free0, &self.free1] {
            out.extend(page.chunks(2).map(|p| p[0] & 0x0F | p[1] << 4));
        }
        out.extend(now.to_le_bytes());
        out
    }

    /// Reads [`Tama5::to_save`]'s block and runs the clock on to `now`; `None` leaves it
    /// where it was saved. A short or missing block changes nothing.
    pub fn load_save(&mut self, data: &[u8], now: Option<u64>) {
        let Some(block) = data.get(..SAVE_BYTES) else {
            return;
        };
        let pages = [
            &mut self.time,
            &mut self.alarm,
            &mut self.free0,
            &mut self.free1,
        ];
        for (page, bytes) in pages.into_iter().zip(block.chunks(8)) {
            for (i, &b) in bytes.iter().enumerate() {
                page[2 * i] = b & 0x0F;
                page[2 * i + 1] = b >> 4;
            }
        }
        self.timer_stopped = self.time[PAGE] & 0x8 == 0;
        for (n, page) in [
            &mut self.time,
            &mut self.alarm,
            &mut self.free0,
            &mut self.free1,
        ]
        .into_iter()
        .enumerate()
        {
            page[PAGE] = page[PAGE] & 0xC | n as u8;
        }
        self.cycles = 0;
        let saved = u64::from_le_bytes(block[32..40].try_into().unwrap());
        if let Some(now) = now {
            self.advance_seconds(now.saturating_sub(saved));
        }
    }
}

/// Days in a year whose leap counter is `leap` (0 = leap year).
fn year_length(leap: u64) -> u64 {
    if leap & 3 == 0 { 366 } else { 365 }
}

fn month_lengths(leap: u64) -> [u64; 12] {
    let feb = if leap & 3 == 0 { 29 } else { 28 };
    [31, feb, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
}

/// Day of the year, 1-based.
fn day_of_year(day: u64, month: u64, leap: u64) -> u64 {
    month_lengths(leap)[..month as usize - 1]
        .iter()
        .sum::<u64>()
        + day
}

/// Month and day of the month (both 1-based) for a 1-based day of the year.
fn month_and_day(mut day: u64, leap: u64) -> (u64, u64) {
    for (i, &length) in month_lengths(leap).iter().enumerate() {
        if day <= length {
            return (i as u64 + 1, day);
        }
        day -= length;
    }
    (12, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes nibble `value` to register `reg`.
    fn set(t: &mut Tama5, ram: &mut [u8], reg: u8, value: u8) {
        t.write(0xA001, reg, ram);
        t.write(0xA000, value, ram);
    }

    fn get(t: &Tama5, ram: &[u8], reg: u8) -> u8 {
        let mut t = t.clone();
        t.write(0xA001, reg, &mut [0; RAM_BYTES]);
        t.read(0xA000, ram) & 0x0F
    }

    #[test]
    fn ram_bytes_go_through_the_registers_a_nibble_at_a_time() {
        let mut t = Tama5::default();
        let mut ram = [0; RAM_BYTES];
        assert_eq!(t.read(0xA000, &ram), 0xF1, "ready");
        // Write $5A to address $13.
        set(&mut t, &mut ram, 4, 0xA);
        set(&mut t, &mut ram, 5, 0x5);
        set(&mut t, &mut ram, 6, 0x1); // command 0 (write), address bit 4
        set(&mut t, &mut ram, 7, 0x3);
        assert_eq!(ram[0x13], 0x5A);
        // Read it back: command 1.
        set(&mut t, &mut ram, 6, 0x3);
        set(&mut t, &mut ram, 7, 0x3);
        assert_eq!((get(&t, &ram, 0xC), get(&t, &ram, 0xD)), (0xA, 0x5));
        // ROM bank $12.
        set(&mut t, &mut ram, 0, 0x2);
        set(&mut t, &mut ram, 1, 0x1);
        assert_eq!(t.rom_bank(), 0x12);
    }

    #[test]
    fn clock_runs_and_rolls_over_the_calendar() {
        let mut t = Tama5::default();
        let mut ram = [0; RAM_BYTES];
        // 24-hour mode (alarm page nibble $A), then 28 Feb (year 00, leap), 23:59:59.
        let mut reg = |t: &mut Tama5, page: u8, n: u8, v: u8| {
            set(t, &mut ram, 4, n);
            set(t, &mut ram, 5, v);
            set(t, &mut ram, 6, 0x8);
            set(t, &mut ram, 7, page);
        };
        reg(&mut t, 2, HOUR_24 as u8, 1);
        for (n, v) in [
            (MONTH_1, 2),
            (DAY_10, 2),
            (DAY_1, 8),
            (HOUR_10, 2),
            (HOUR_1, 3),
        ] {
            reg(&mut t, 0, n as u8, v);
        }
        reg(&mut t, 0, MINUTE_10 as u8, 5);
        reg(&mut t, 0, MINUTE_1 as u8, 9);
        reg(&mut t, 0, SECOND_10 as u8, 5);
        reg(&mut t, 0, SECOND_1 as u8, 9);
        t.tick(CLOCK_HZ);
        assert_eq!(
            &t.time[..YEAR_10 + 1],
            &[0, 0, 0, 0, 0, 0, 1, 9, 2, 2, 0, 0, 0]
        );
        // 366 days later (2000 is a leap year): 1 March 2001.
        t.advance_seconds(366 * 86_400);
        assert_eq!(
            (t.time[DAY_1], t.time[MONTH_1], t.time[YEAR_1]),
            (1, 3, 1),
            "{:?}",
            t.time
        );
        // The time page reads back through register $C.
        set(&mut t, &mut ram, 4, MONTH_1 as u8);
        set(&mut t, &mut ram, 6, 0x8);
        set(&mut t, &mut ram, 7, 1);
        assert_eq!(get(&t, &ram, 0xC), 3);
    }

    #[test]
    fn save_block_round_trips_and_catches_up() {
        let mut t = Tama5::default();
        t.clock_command(0x01, 0); // start the timer
        t.time[MINUTE_1] = 9;
        t.time[MINUTE_10] = 5;
        let block = t.to_save(1000);
        assert_eq!(block.len(), SAVE_BYTES);
        let mut loaded = Tama5::default();
        loaded.load_save(&block, Some(1000 + 60));
        assert_eq!((loaded.time[MINUTE_10], loaded.time[MINUTE_1]), (0, 0));
        assert_eq!(loaded.time[HOUR_1], 1);
        assert_eq!(loaded.alarm[PAGE] & 3, 1);
    }
}
