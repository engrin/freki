//! The Super Game Boy's ICD2 chip: the link between the SNES and the Game Boy inside the
//! cartridge, mapped at $6000–$7FFF of the SNES (fullsnes, "SNES Cart Super Gameboy").
//!
//! - The Game Boy's LCD output is captured into four buffers of one character row each
//!   (160 × 8 pixels as 20 tiles in 2 bit planes, 320 bytes); the SNES program reads a
//!   finished row through $7800 (after selecting it in $6001) while the next is written.
//! - Packets the Game Boy sends through its joypad port arrive in a 16-byte buffer
//!   ($7000–$700F), flagged in $6002.
//! - $6003 resets the Game Boy and sets the number of players; $6004–$6007 hold the
//!   SNES joypads as the Game Boy reads them.

use serde::{Deserialize, Serialize};

/// Bytes of one character row: 20 tiles of 16 bytes.
const ROW_BYTES: usize = 320;

#[derive(Serialize, Deserialize, Clone)]
pub struct Icd2 {
    /// Four row buffers of 512 bytes (320 used).
    rows: Vec<u8>,
    /// The buffer the Game Boy's current character row goes into, and that row (0–17).
    write_buffer: u8,
    lcd_row: u8,
    read_buffer: u8,
    read_index: u16,
    packet: [u8; 16],
    packet_ready: bool,
    /// $6003: bit 7 runs the Game Boy (0 holds it in reset), bits 4–5 players.
    pub control: u8,
    /// $6004–$6007: buttons per player (bit clear = pressed).
    pub joypads: [u8; 4],
    /// The Game Boy should be (re)started: $6003 bit 7 went from 0 to 1.
    pub reset_released: bool,
}

impl Default for Icd2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Icd2 {
    pub fn new() -> Self {
        Icd2 {
            rows: vec![0xFF; 4 * 512],
            write_buffer: 0,
            lcd_row: 0,
            read_buffer: 0,
            read_index: 0,
            packet: [0; 16],
            packet_ready: false,
            control: 0x01,
            joypads: [0xFF; 4],
            reset_released: false,
        }
    }

    /// Whether the Game Boy is held in reset.
    pub fn gb_in_reset(&self) -> bool {
        self.control & 0x80 == 0
    }

    /// Players the Game Boy's joypad port cycles through (1, 2 or 4).
    pub fn players(&self) -> u8 {
        match (self.control >> 4) & 3 {
            0 => 1,
            1 => 2,
            _ => 4,
        }
    }

    pub fn read(&mut self, addr: u16, open_bus: u8) -> u8 {
        if addr & 0xF800 == 0x7800 {
            let i = self.read_index as usize;
            let v = if i < ROW_BYTES {
                self.rows[self.read_buffer as usize * 512 + i]
            } else {
                0xFF
            };
            self.read_index = (self.read_index + 1) & 0x1FF;
            return v;
        }
        if addr & 0xF800 == 0x7000 {
            let i = (addr & 0x0F) as usize;
            if i == 0 {
                self.packet_ready = false;
            }
            return self.packet[i];
        }
        match addr & 0xF80F {
            0x6000 => self.lcd_row << 3 | self.write_buffer,
            0x6002 => u8::from(self.packet_ready),
            0x600F => 0x21,
            _ => open_bus,
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        match addr & 0xF80F {
            0x6001 => {
                self.read_buffer = v & 3;
                self.read_index = 0;
            }
            0x6003 => {
                if self.control & 0x80 == 0 && v & 0x80 != 0 {
                    self.reset_released = true;
                }
                self.control = v;
            }
            0x6004..=0x6007 => self.joypads[(addr & 3) as usize] = v,
            _ => {}
        }
    }

    /// A packet from the Game Boy.
    pub fn receive_packet(&mut self, packet: &[u8; 16]) {
        self.packet = *packet;
        self.packet_ready = true;
    }

    /// The Game Boy's LCD finished line `ly` (0–143) with these shades (0–3, 160 pixels).
    pub fn gb_line(&mut self, ly: u8, shades: &[u8]) {
        if ly % 8 == 0 {
            self.write_buffer = (self.write_buffer + 1) & 3;
            self.lcd_row = ly / 8;
        }
        let row = &mut self.rows[self.write_buffer as usize * 512..][..512];
        let y = (ly % 8) as usize;
        for tile in 0..20 {
            let (mut lo, mut hi) = (0u8, 0u8);
            for x in 0..8 {
                let s = shades[tile * 8 + x] & 3;
                lo |= (s & 1) << (7 - x);
                hi |= (s >> 1) << (7 - x);
            }
            row[tile * 16 + y * 2] = lo;
            row[tile * 16 + y * 2 + 1] = hi;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_become_2bpp_tiles_in_row_buffers() {
        let mut icd2 = Icd2::new();
        // Line 0: pixel 0 shade 3, pixel 1 shade 1, pixel 8 shade 2.
        let mut line = [0u8; 160];
        line[0] = 3;
        line[1] = 1;
        line[8] = 2;
        icd2.gb_line(0, &line);
        assert_eq!(icd2.read(0x6000, 0), 0x01, "row 0 into buffer 1");
        icd2.write(0x6001, 1);
        assert_eq!(icd2.read(0x7800, 0), 0b1100_0000, "plane 0 of tile 0");
        assert_eq!(icd2.read(0x7800, 0), 0b1000_0000, "plane 1 of tile 0");
        for _ in 2..16 {
            icd2.read(0x7800, 0);
        }
        assert_eq!(icd2.read(0x7800, 0), 0, "plane 0 of tile 1");
        assert_eq!(icd2.read(0x7800, 0), 0b1000_0000, "plane 1 of tile 1");
        icd2.gb_line(8, &line);
        assert_eq!(icd2.read(0x6000, 0), 1 << 3 | 2, "row 1 into buffer 2");
    }

    #[test]
    fn reading_the_first_packet_byte_clears_the_flag() {
        let mut icd2 = Icd2::new();
        let mut packet = [0; 16];
        packet[0] = 0x89;
        packet[15] = 0x42;
        icd2.receive_packet(&packet);
        assert_eq!(icd2.read(0x6002, 0), 1);
        assert_eq!(icd2.read(0x700F, 0), 0x42);
        assert_eq!(icd2.read(0x6002, 0), 1, "only $7000 clears it");
        assert_eq!(icd2.read(0x7000, 0), 0x89);
        assert_eq!(icd2.read(0x6002, 0), 0);
    }

    #[test]
    fn control_releases_the_game_boy_and_sets_players() {
        let mut icd2 = Icd2::new();
        assert!(icd2.gb_in_reset());
        icd2.write(0x6003, 0x91);
        assert!(!icd2.gb_in_reset() && icd2.reset_released);
        assert_eq!(icd2.players(), 2);
    }
}
