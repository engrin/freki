//! MBC7's extras: a two-axis accelerometer (Kirby Tilt 'n' Tumble steers by tilting the
//! Game Boy) and a 93LC56 serial EEPROM that holds the save (256 bytes, 128 16-bit words).
//!
//! Both sit at $A000–$AFFF once RAM is enabled twice ($0A to $0000–$1FFF, $40 to
//! $4000–$5FFF), one register per 16 bytes:
//!
//! | Register | Read | Write |
//! |---|---|---|
//! | $A00x | | $55 erases the latched tilt |
//! | $A01x | | $AA latches the current tilt (after an erase) |
//! | $A02x / $A03x | X low / high | |
//! | $A04x / $A05x | Y low / high | |
//! | $A06x / $A07x | $00 / $FF | |
//! | $A08x | EEPROM: bit 0 data out | bit 7 chip select, bit 6 clock, bit 1 data in |

use serde::{Deserialize, Serialize};

/// Sensor value when level; one g of tilt moves it by [`ONE_G`].
const CENTER: u16 = 0x81D0;
const ONE_G: f32 = 0x70 as f32;
/// Value after an erase, before the next latch.
const ERASED: u16 = 0x8000;
pub const EEPROM_BYTES: usize = 256;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Mbc7 {
    x: u16,
    y: u16,
    /// An erase ($55) happened: the next $AA latches.
    erased: bool,
    eeprom: Eeprom,
}

impl Default for Mbc7 {
    fn default() -> Self {
        Self {
            x: ERASED,
            y: ERASED,
            erased: false,
            eeprom: Eeprom::default(),
        }
    }
}

impl Mbc7 {
    /// Reads register `reg` (the `r` of $A0r0).
    pub fn read(&self, reg: u8) -> u8 {
        match reg {
            0x2 => self.x as u8,
            0x3 => (self.x >> 8) as u8,
            0x4 => self.y as u8,
            0x5 => (self.y >> 8) as u8,
            0x6 => 0x00,
            0x8 => self.eeprom.pins(),
            _ => 0xFF,
        }
    }

    /// Writes register `reg`. `tilt` is the current tilt in g (x right, y towards the
    /// player); `eeprom` the 256 save bytes. Returns `true` if the EEPROM changed.
    pub fn write(&mut self, reg: u8, value: u8, tilt: (f32, f32), eeprom: &mut [u8]) -> bool {
        match (reg, value) {
            (0x0, 0x55) => {
                self.x = ERASED;
                self.y = ERASED;
                self.erased = true;
                false
            }
            (0x1, 0xAA) if self.erased => {
                let axis = |g: f32| (CENTER as f32 + g.clamp(-2.0, 2.0) * ONE_G).round() as u16;
                // Tilting right lowers X; tilting the top towards the player raises Y.
                self.x = axis(-tilt.0);
                self.y = axis(tilt.1);
                self.erased = false;
                false
            }
            (0x8, _) => self.eeprom.write(value, eeprom),
            _ => false,
        }
    }
}

/// The 93LC56 in 16-bit mode, driven bit by bit through its pins.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
struct Eeprom {
    cs: bool,
    clk: bool,
    di: bool,
    /// Data out.
    out: bool,
    state: State,
    write_enabled: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
enum State {
    /// Waiting for the start bit (a 1 clocked in).
    #[default]
    Idle,
    /// Collecting the 2-bit opcode and 8-bit address.
    Command { bits: u16, count: u8 },
    /// Shifting a word out, most significant bit first (then the next word).
    Read { addr: u8, word: u16, left: u8 },
    /// Collecting 16 data bits for one word (`addr`) or all of them (`None`).
    Write {
        addr: Option<u8>,
        bits: u16,
        count: u8,
    },
    /// Command done; ignoring clocks until chip select drops.
    Done,
}

fn word(eeprom: &[u8], addr: u8) -> u16 {
    let i = (addr & 0x7F) as usize * 2;
    u16::from_le_bytes([eeprom[i], eeprom[i + 1]])
}

fn set_word(eeprom: &mut [u8], addr: u8, value: u16) {
    let i = (addr & 0x7F) as usize * 2;
    eeprom[i..i + 2].copy_from_slice(&value.to_le_bytes());
}

impl Eeprom {
    fn pins(&self) -> u8 {
        u8::from(self.cs) << 7
            | u8::from(self.clk) << 6
            | u8::from(self.di) << 1
            | u8::from(self.out)
    }

    /// New pin levels from the game. Returns `true` if the stored data changed.
    fn write(&mut self, value: u8, eeprom: &mut [u8]) -> bool {
        let rising = !self.clk && value & 0x40 != 0;
        self.cs = value & 0x80 != 0;
        self.clk = value & 0x40 != 0;
        self.di = value & 0x02 != 0;
        if !self.cs {
            self.state = State::Idle;
            self.out = true; // ready
            return false;
        }
        if !rising {
            return false;
        }
        let di = self.di;
        let mut changed = false;
        self.state = match std::mem::take(&mut self.state) {
            State::Idle if di => State::Command { bits: 0, count: 0 },
            State::Idle => State::Idle,
            State::Command { bits, count } => {
                let bits = bits << 1 | u16::from(di);
                if count + 1 < 10 {
                    State::Command {
                        bits,
                        count: count + 1,
                    }
                } else {
                    let addr = bits as u8;
                    match bits >> 8 {
                        0b10 => {
                            self.out = false; // a dummy 0 before the data
                            State::Read {
                                addr,
                                word: word(eeprom, addr),
                                left: 16,
                            }
                        }
                        0b01 => State::Write {
                            addr: Some(addr),
                            bits: 0,
                            count: 0,
                        },
                        0b11 => {
                            if self.write_enabled {
                                set_word(eeprom, addr, 0xFFFF);
                                changed = true;
                            }
                            self.out = true;
                            State::Done
                        }
                        _ => match addr >> 6 {
                            0b11 => {
                                self.write_enabled = true;
                                State::Done
                            }
                            0b00 => {
                                self.write_enabled = false;
                                State::Done
                            }
                            0b10 => {
                                if self.write_enabled {
                                    eeprom.fill(0xFF);
                                    changed = true;
                                }
                                self.out = true;
                                State::Done
                            }
                            _ => State::Write {
                                addr: None,
                                bits: 0,
                                count: 0,
                            },
                        },
                    }
                }
            }
            State::Read {
                addr,
                word: w,
                left,
            } => {
                self.out = w & 0x8000 != 0;
                if left > 1 {
                    State::Read {
                        addr,
                        word: w << 1,
                        left: left - 1,
                    }
                } else {
                    // Sequential read: the next word follows.
                    let next = addr.wrapping_add(1) & 0x7F;
                    State::Read {
                        addr: next,
                        word: word(eeprom, next),
                        left: 16,
                    }
                }
            }
            State::Write { addr, bits, count } => {
                let bits = bits << 1 | u16::from(di);
                if count + 1 < 16 {
                    State::Write {
                        addr,
                        bits,
                        count: count + 1,
                    }
                } else {
                    if self.write_enabled {
                        match addr {
                            Some(a) => set_word(eeprom, a, bits),
                            None => (0..128).for_each(|a| set_word(eeprom, a, bits)),
                        }
                        changed = true;
                    }
                    self.out = true;
                    State::Done
                }
            }
            State::Done => State::Done,
        };
        changed
    }
}

#[cfg(test)]
// Bit groups in the test commands: start bit, opcode, address.
#[allow(clippy::unusual_byte_groupings)]
mod tests {
    use super::*;

    /// Clocks `bits` (most significant first, `n` of them) in with chip select held.
    fn send(m: &mut Mbc7, mem: &mut [u8], bits: u32, n: u32) -> Vec<bool> {
        let mut out = Vec::new();
        for i in (0..n).rev() {
            let di = if bits >> i & 1 != 0 { 0x02 } else { 0 };
            m.write(0x8, 0x80 | di, (0.0, 0.0), mem);
            m.write(0x8, 0xC0 | di, (0.0, 0.0), mem);
            out.push(m.read(0x8) & 1 != 0);
        }
        out
    }

    fn deselect(m: &mut Mbc7, mem: &mut [u8]) {
        m.write(0x8, 0x00, (0.0, 0.0), mem);
    }

    #[test]
    fn eeprom_write_needs_enable_then_reads_back() {
        let mut m = Mbc7::default();
        let mut mem = vec![0xFF; EEPROM_BYTES];
        // WRITE word 5 = $1234 while write-protected: ignored.
        send(&mut m, &mut mem, 0b1_01_00000101, 11);
        send(&mut m, &mut mem, 0x1234, 16);
        deselect(&mut m, &mut mem);
        assert_eq!(word(&mem, 5), 0xFFFF);
        // EWEN, then the write sticks.
        send(&mut m, &mut mem, 0b1_00_11000000, 11);
        deselect(&mut m, &mut mem);
        send(&mut m, &mut mem, 0b1_01_00000101, 11);
        send(&mut m, &mut mem, 0x1234, 16);
        deselect(&mut m, &mut mem);
        assert_eq!(word(&mem, 5), 0x1234);
        // READ word 5: the bits come out most significant first.
        let start = send(&mut m, &mut mem, 0b1_10_00000101, 11);
        assert!(!start[10], "dummy 0 after the address");
        let bits = send(&mut m, &mut mem, 0, 16);
        let value = bits.iter().fold(0u16, |acc, &b| acc << 1 | u16::from(b));
        assert_eq!(value, 0x1234);
    }

    #[test]
    fn accelerometer_latches_after_erase() {
        let mut m = Mbc7::default();
        let mut mem = vec![0; EEPROM_BYTES];
        m.write(0x1, 0xAA, (1.0, 0.0), &mut mem);
        assert_eq!(
            m.read(0x3),
            (ERASED >> 8) as u8,
            "no latch without an erase"
        );
        m.write(0x0, 0x55, (0.0, 0.0), &mut mem);
        m.write(0x1, 0xAA, (1.0, -0.5), &mut mem);
        let x = u16::from(m.read(0x2)) | u16::from(m.read(0x3)) << 8;
        let y = u16::from(m.read(0x4)) | u16::from(m.read(0x5)) << 8;
        assert_eq!(x, CENTER - 0x70);
        assert_eq!(y, CENTER - 0x38);
    }
}
