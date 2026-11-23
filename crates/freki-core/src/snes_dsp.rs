//! The SNES sound DSP (S-DSP), for the Super Game Boy's sound: eight voices playing BRR
//! samples from the sound RAM with Gaussian interpolation, ADSR/GAIN envelopes, noise and
//! pitch modulation, mixed with an echo through an 8-tap FIR filter. One call to
//! [`Dsp::sample`] produces one 32 kHz stereo sample. Written from the "fullsnes"
//! description (nocash), at sample level rather than cycle level.

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

/// Samples between envelope/noise steps for each 5-bit rate (0 = never).
const RATES: [u32; 32] = [
    0, 2048, 1536, 1280, 1024, 768, 640, 512, 384, 320, 256, 192, 160, 128, 96, 80, 64, 48, 40, 32,
    24, 20, 16, 12, 10, 8, 6, 5, 4, 3, 2, 1,
];

/// The interpolation table (fullsnes).
const GAUSS: [i32; 512] = [
    0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000,
    0x000, 0x000, 0x000, 0x001, 0x001, 0x001, 0x001, 0x001, 0x001, 0x001, 0x001, 0x001, 0x001,
    0x001, 0x002, 0x002, 0x002, 0x002, 0x002, 0x002, 0x002, 0x003, 0x003, 0x003, 0x003, 0x003,
    0x004, 0x004, 0x004, 0x004, 0x004, 0x005, 0x005, 0x005, 0x005, 0x006, 0x006, 0x006, 0x006,
    0x007, 0x007, 0x007, 0x008, 0x008, 0x008, 0x009, 0x009, 0x009, 0x00A, 0x00A, 0x00A, 0x00B,
    0x00B, 0x00B, 0x00C, 0x00C, 0x00D, 0x00D, 0x00E, 0x00E, 0x00F, 0x00F, 0x00F, 0x010, 0x010,
    0x011, 0x011, 0x012, 0x013, 0x013, 0x014, 0x014, 0x015, 0x015, 0x016, 0x017, 0x017, 0x018,
    0x018, 0x019, 0x01A, 0x01B, 0x01B, 0x01C, 0x01D, 0x01D, 0x01E, 0x01F, 0x020, 0x020, 0x021,
    0x022, 0x023, 0x024, 0x024, 0x025, 0x026, 0x027, 0x028, 0x029, 0x02A, 0x02B, 0x02C, 0x02D,
    0x02E, 0x02F, 0x030, 0x031, 0x032, 0x033, 0x034, 0x035, 0x036, 0x037, 0x038, 0x03A, 0x03B,
    0x03C, 0x03D, 0x03E, 0x040, 0x041, 0x042, 0x043, 0x045, 0x046, 0x047, 0x049, 0x04A, 0x04C,
    0x04D, 0x04E, 0x050, 0x051, 0x053, 0x054, 0x056, 0x057, 0x059, 0x05A, 0x05C, 0x05E, 0x05F,
    0x061, 0x063, 0x064, 0x066, 0x068, 0x06A, 0x06B, 0x06D, 0x06F, 0x071, 0x073, 0x075, 0x076,
    0x078, 0x07A, 0x07C, 0x07E, 0x080, 0x082, 0x084, 0x086, 0x089, 0x08B, 0x08D, 0x08F, 0x091,
    0x093, 0x096, 0x098, 0x09A, 0x09C, 0x09F, 0x0A1, 0x0A3, 0x0A6, 0x0A8, 0x0AB, 0x0AD, 0x0AF,
    0x0B2, 0x0B4, 0x0B7, 0x0BA, 0x0BC, 0x0BF, 0x0C1, 0x0C4, 0x0C7, 0x0C9, 0x0CC, 0x0CF, 0x0D2,
    0x0D4, 0x0D7, 0x0DA, 0x0DD, 0x0E0, 0x0E3, 0x0E6, 0x0E9, 0x0EC, 0x0EF, 0x0F2, 0x0F5, 0x0F8,
    0x0FB, 0x0FE, 0x101, 0x104, 0x107, 0x10B, 0x10E, 0x111, 0x114, 0x118, 0x11B, 0x11E, 0x122,
    0x125, 0x129, 0x12C, 0x130, 0x133, 0x137, 0x13A, 0x13E, 0x141, 0x145, 0x148, 0x14C, 0x150,
    0x153, 0x157, 0x15B, 0x15F, 0x162, 0x166, 0x16A, 0x16E, 0x172, 0x176, 0x17A, 0x17D, 0x181,
    0x185, 0x189, 0x18D, 0x191, 0x195, 0x19A, 0x19E, 0x1A2, 0x1A6, 0x1AA, 0x1AE, 0x1B2, 0x1B7,
    0x1BB, 0x1BF, 0x1C3, 0x1C8, 0x1CC, 0x1D0, 0x1D5, 0x1D9, 0x1DD, 0x1E2, 0x1E6, 0x1EB, 0x1EF,
    0x1F3, 0x1F8, 0x1FC, 0x201, 0x205, 0x20A, 0x20F, 0x213, 0x218, 0x21C, 0x221, 0x226, 0x22A,
    0x22F, 0x233, 0x238, 0x23D, 0x241, 0x246, 0x24B, 0x250, 0x254, 0x259, 0x25E, 0x263, 0x267,
    0x26C, 0x271, 0x276, 0x27B, 0x280, 0x284, 0x289, 0x28E, 0x293, 0x298, 0x29D, 0x2A2, 0x2A6,
    0x2AB, 0x2B0, 0x2B5, 0x2BA, 0x2BF, 0x2C4, 0x2C9, 0x2CE, 0x2D3, 0x2D8, 0x2DC, 0x2E1, 0x2E6,
    0x2EB, 0x2F0, 0x2F5, 0x2FA, 0x2FF, 0x304, 0x309, 0x30E, 0x313, 0x318, 0x31D, 0x322, 0x326,
    0x32B, 0x330, 0x335, 0x33A, 0x33F, 0x344, 0x349, 0x34E, 0x353, 0x357, 0x35C, 0x361, 0x366,
    0x36B, 0x370, 0x374, 0x379, 0x37E, 0x383, 0x388, 0x38C, 0x391, 0x396, 0x39B, 0x39F, 0x3A4,
    0x3A9, 0x3AD, 0x3B2, 0x3B7, 0x3BB, 0x3C0, 0x3C5, 0x3C9, 0x3CE, 0x3D2, 0x3D7, 0x3DC, 0x3E0,
    0x3E5, 0x3E9, 0x3ED, 0x3F2, 0x3F6, 0x3FB, 0x3FF, 0x403, 0x408, 0x40C, 0x410, 0x415, 0x419,
    0x41D, 0x421, 0x425, 0x42A, 0x42E, 0x432, 0x436, 0x43A, 0x43E, 0x442, 0x446, 0x44A, 0x44E,
    0x452, 0x455, 0x459, 0x45D, 0x461, 0x465, 0x468, 0x46C, 0x470, 0x473, 0x477, 0x47A, 0x47E,
    0x481, 0x485, 0x488, 0x48C, 0x48F, 0x492, 0x496, 0x499, 0x49C, 0x49F, 0x4A2, 0x4A6, 0x4A9,
    0x4AC, 0x4AF, 0x4B2, 0x4B5, 0x4B7, 0x4BA, 0x4BD, 0x4C0, 0x4C3, 0x4C5, 0x4C8, 0x4CB, 0x4CD,
    0x4D0, 0x4D2, 0x4D5, 0x4D7, 0x4D9, 0x4DC, 0x4DE, 0x4E0, 0x4E3, 0x4E5, 0x4E7, 0x4E9, 0x4EB,
    0x4ED, 0x4EF, 0x4F1, 0x4F3, 0x4F5, 0x4F6, 0x4F8, 0x4FA, 0x4FB, 0x4FD, 0x4FF, 0x500, 0x502,
    0x503, 0x504, 0x506, 0x507, 0x508, 0x50A, 0x50B, 0x50C, 0x50D, 0x50E, 0x50F, 0x510, 0x511,
    0x511, 0x512, 0x513, 0x514, 0x514, 0x515, 0x516, 0x516, 0x517, 0x517, 0x517, 0x518, 0x518,
    0x518, 0x518, 0x518, 0x519, 0x519,
];

#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum Phase {
    Attack,
    Decay,
    Sustain,
    #[default]
    Release,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Voice {
    /// The BRR block being played.
    addr: u16,
    header: u8,
    /// 16 decoded samples of the block, after the last 3 of the one before.
    samples: [i32; 19],
    /// The two newest decoded samples, for the BRR filters.
    p1: i32,
    p2: i32,
    counter: u32,
    env: i32,
    phase: Phase,
    /// Samples left before a keyed-on voice starts.
    delay: u8,
    /// The last output (15-bit, after the envelope), for pitch modulation and OUTX.
    out: i32,
}

#[derive(Serialize, Deserialize)]
pub struct Dsp {
    #[serde(with = "BigArray")]
    regs: [u8; 128],
    voices: [Voice; 8],
    endx: u8,
    noise: i32,
    ticks: u32,
    echo_index: usize,
    echo_length: usize,
    fir: [[i32; 8]; 2],
    fir_pos: usize,
}

impl Default for Dsp {
    fn default() -> Self {
        Self::new()
    }
}

impl Dsp {
    pub fn new() -> Self {
        let mut regs = [0; 128];
        regs[0x6C] = 0xE0; // reset, muted, echo writes off
        Self {
            regs,
            voices: Default::default(),
            endx: 0,
            noise: 0x4000,
            ticks: 0,
            echo_index: 0,
            echo_length: 0,
            fir: [[0; 8]; 2],
            fir_pos: 0,
        }
    }

    pub fn read(&self, reg: u8) -> u8 {
        match reg {
            0x7C => self.endx,
            _ => self.regs[usize::from(reg)],
        }
    }

    /// A write through $F2/$F3; needs `ram` for the voices it keys on.
    pub fn write(&mut self, reg: u8, value: u8) {
        self.regs[usize::from(reg)] = value;
        match reg {
            0x4C => {
                for v in 0..8 {
                    if value & (1 << v) != 0 {
                        self.voices[v].delay = 5;
                        self.endx &= !(1 << v);
                    }
                }
            }
            0x7C => self.endx = 0,
            _ => {}
        }
    }

    fn reg(&self, voice: usize, r: usize) -> u8 {
        self.regs[voice * 0x10 + r]
    }

    fn sreg(&self, r: usize) -> i32 {
        i32::from(self.regs[r] as i8)
    }

    fn rate_due(&self, rate: u8) -> bool {
        let period = RATES[usize::from(rate & 0x1F)];
        period != 0 && self.ticks.is_multiple_of(period)
    }

    /// Produces one 32 kHz stereo sample; reads samples from and writes the echo to `ram`.
    pub fn sample(&mut self, ram: &mut [u8; 0x10000]) -> (i16, i16) {
        self.ticks = self.ticks.wrapping_add(1);
        let flg = self.regs[0x6C];
        if self.rate_due(flg & 0x1F) {
            let bit = (self.noise ^ (self.noise >> 1)) & 1;
            self.noise = ((self.noise >> 1) & 0x3FFF) | (bit << 14);
        }
        let (mut main, mut echo_in) = ([0i32; 2], [0i32; 2]);
        let mut prev_out = 0;
        for v in 0..8 {
            let s = self.voice(v, ram, prev_out);
            prev_out = s;
            for (side, sum) in main.iter_mut().enumerate() {
                let vol = i32::from(self.reg(v, side) as i8);
                *sum = (*sum + ((s * vol) >> 6)).clamp(-0x8000, 0x7FFF);
                if self.regs[0x4D] & (1 << v) != 0 {
                    echo_in[side] = (echo_in[side] + ((s * vol) >> 6)).clamp(-0x8000, 0x7FFF);
                }
            }
        }
        let fir = self.echo(ram, echo_in);
        let mut out = [0i16; 2];
        for side in 0..2 {
            let master = (main[side] * self.sreg(0x0C + side * 0x10)) >> 7;
            let echo = (fir[side] * self.sreg(0x2C + side * 0x10)) >> 7;
            out[side] = if flg & 0x40 != 0 {
                0
            } else {
                (master + echo).clamp(-0x8000, 0x7FFF) as i16
            };
        }
        (out[0], out[1])
    }

    /// One voice's sample for this tick (15-bit, after its envelope).
    fn voice(&mut self, v: usize, ram: &[u8; 0x10000], prev_out: i32) -> i32 {
        let dir = u16::from(self.regs[0x5D]) << 8;
        let entry = dir.wrapping_add(u16::from(self.reg(v, 4)) * 4);
        let flg = self.regs[0x6C];
        let koff = self.regs[0x5C] & (1 << v) != 0;
        let voice = &mut self.voices[v];
        if voice.delay > 0 {
            voice.delay -= 1;
            if voice.delay == 4 {
                // Key on: start the sample from its start address.
                voice.addr = word(ram, entry);
                voice.samples = [0; 19];
                voice.p1 = 0;
                voice.p2 = 0;
                voice.counter = 0;
                voice.env = 0;
                voice.phase = Phase::Attack;
                let ended = decode_block(voice, ram);
                if ended {
                    self.endx |= 1 << v;
                }
            }
            voice.out = 0;
            return 0;
        }
        if flg & 0x80 != 0 {
            voice.phase = Phase::Release;
            voice.env = 0;
        } else if koff {
            voice.phase = Phase::Release;
        }

        // Pitch, with modulation by the previous voice.
        let mut step = (u32::from(self.regs[v * 0x10 + 2])
            | (u32::from(self.regs[v * 0x10 + 3] & 0x3F) << 8)) as i32;
        if v > 0 && self.regs[0x2D] & (1 << v) != 0 {
            step = (step * ((prev_out >> 4) + 0x400)) >> 10;
        }
        let step = step.clamp(0, 0x7FFF) as u32;

        let voice = &mut self.voices[v];
        let index = (voice.counter >> 12) as usize;
        let sample = if self.regs[0x3D] & (1 << v) != 0 {
            (self.noise << 17) >> 17
        } else {
            interpolate(
                &voice.samples[index..index + 4],
                ((voice.counter >> 4) & 0xFF) as usize,
            )
        };
        let out = (sample * voice.env) >> 11;
        voice.out = out;
        self.regs[v * 0x10 + 8] = (voice.env >> 4) as u8;
        self.regs[v * 0x10 + 9] = (out >> 7) as u8;

        self.envelope(v);
        let voice = &mut self.voices[v];
        voice.counter += step;
        if voice.counter >= 0x10000 {
            voice.counter -= 0x10000;
            // On to the next block: after an end block, to the loop address (and silent,
            // unless it loops).
            if voice.header & 1 != 0 {
                voice.addr = word(ram, entry.wrapping_add(2));
                if voice.header & 2 == 0 {
                    voice.phase = Phase::Release;
                    voice.env = 0;
                }
            } else {
                voice.addr = voice.addr.wrapping_add(9);
            }
            if decode_block(voice, ram) {
                self.endx |= 1 << v;
            }
        }
        out
    }

    fn envelope(&mut self, v: usize) {
        let adsr1 = self.reg(v, 5);
        let adsr2 = self.reg(v, 6);
        let gain = self.reg(v, 7);
        let phase = self.voices[v].phase;
        let env = self.voices[v].env;
        let exp = |env: i32| -(((env - 1) >> 8) + 1);
        let (rate, step) = if phase == Phase::Release {
            (31, -8)
        } else if adsr1 & 0x80 != 0 {
            match phase {
                Phase::Attack => {
                    let rate = (adsr1 & 0x0F) * 2 + 1;
                    (rate, if rate == 31 { 1024 } else { 32 })
                }
                Phase::Decay => (((adsr1 >> 4) & 7) * 2 + 16, exp(env)),
                _ => (adsr2 & 0x1F, exp(env)),
            }
        } else if gain & 0x80 == 0 {
            self.voices[v].env = i32::from(gain & 0x7F) * 16;
            return;
        } else {
            let rate = gain & 0x1F;
            match (gain >> 5) & 3 {
                0 => (rate, -32),
                1 => (rate, exp(env)),
                2 => (rate, 32),
                _ => (rate, if env < 0x600 { 32 } else { 8 }),
            }
        };
        if !self.rate_due(rate) {
            return;
        }
        let voice = &mut self.voices[v];
        voice.env = (env + step).clamp(0, 0x7FF);
        match voice.phase {
            Phase::Attack if voice.env >= 0x7E0 => voice.phase = Phase::Decay,
            Phase::Decay if voice.env <= (i32::from(adsr2 >> 5) + 1) * 0x100 => {
                voice.phase = Phase::Sustain;
            }
            _ => {}
        }
    }

    /// The echo: reads the oldest entry from the ring buffer, filters it through the FIR,
    /// and writes the new input (with feedback) in its place. Returns the FIR output.
    fn echo(&mut self, ram: &mut [u8; 0x10000], input: [i32; 2]) -> [i32; 2] {
        let base = usize::from(self.regs[0x6D]) << 8;
        if self.echo_index == 0 {
            self.echo_length = usize::from(self.regs[0x7D] & 0x0F) * 0x800;
        }
        let addr = (base + self.echo_index) & 0xFFFF;
        self.fir_pos = (self.fir_pos + 1) & 7;
        let mut out = [0i32; 2];
        for side in 0..2 {
            let a = (addr + side * 2) & 0xFFFF;
            let stored = i32::from(i16::from_le_bytes([ram[a], ram[(a + 1) & 0xFFFF]]));
            self.fir[side][self.fir_pos] = stored >> 1;
            let mut sum = 0;
            for tap in 0..8 {
                let sample = self.fir[side][(self.fir_pos + 1 + tap) & 7];
                sum += (sample * self.sreg(0x0F + tap * 0x10)) >> 6;
            }
            out[side] = sum.clamp(-0x8000, 0x7FFF);
            let feedback = (out[side] * self.sreg(0x0D)) >> 7;
            let value = ((input[side] + feedback).clamp(-0x8000, 0x7FFF) & !1) as i16;
            if self.regs[0x6C] & 0x20 == 0 {
                let [lo, hi] = value.to_le_bytes();
                ram[a] = lo;
                ram[(a + 1) & 0xFFFF] = hi;
            }
        }
        self.echo_index += 4;
        if self.echo_index >= self.echo_length.max(4) {
            self.echo_index = 0;
        }
        out
    }
}

fn word(ram: &[u8; 0x10000], addr: u16) -> u16 {
    u16::from_le_bytes([
        ram[usize::from(addr)],
        ram[usize::from(addr.wrapping_add(1))],
    ])
}

/// Decodes the 9-byte BRR block at the voice's address into its 16 samples (keeping the
/// previous block's last three in front). Returns whether it is an end block.
fn decode_block(voice: &mut Voice, ram: &[u8; 0x10000]) -> bool {
    voice.samples.copy_within(16..19, 0);
    let header = ram[usize::from(voice.addr)];
    voice.header = header;
    let shift = header >> 4;
    let filter = (header >> 2) & 3;
    for i in 0..16 {
        let byte = ram[usize::from(voice.addr.wrapping_add(1 + i as u16 / 2))];
        let nibble = if i % 2 == 0 { byte >> 4 } else { byte & 0x0F };
        let nibble = i32::from(((nibble << 4) as i8) >> 4);
        let mut s = if shift <= 12 {
            (nibble << shift) >> 1
        } else {
            ((nibble >> 3) << 12) >> 1
        };
        let (p1, p2) = (voice.p1, voice.p2);
        s += match filter {
            0 => 0,
            1 => p1 + ((-p1) >> 4),
            2 => p1 * 2 + ((-p1 * 3) >> 5) - p2 + (p2 >> 4),
            _ => p1 * 2 + ((-p1 * 13) >> 6) - p2 + ((p2 * 3) >> 4),
        };
        // Clamp to 16 bits, then wrap to 15 (the hardware loses the sign bit).
        let s = i32::from(((s.clamp(-0x8000, 0x7FFF) as i16) << 1) >> 1);
        voice.p2 = p1;
        voice.p1 = s;
        voice.samples[3 + i] = s;
    }
    header & 1 != 0
}

/// Gaussian interpolation of four samples (oldest first) at fraction `i` (0–255).
fn interpolate(s: &[i32], i: usize) -> i32 {
    let mut out = (GAUSS[0xFF - i] * s[0]) >> 10;
    out += (GAUSS[0x1FF - i] * s[1]) >> 10;
    out = i32::from(out as i16);
    out += (GAUSS[0x100 + i] * s[2]) >> 10;
    out = i32::from(out as i16);
    out += (GAUSS[i] * s[3]) >> 10;
    out.clamp(-0x8000, 0x7FFF) >> 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brr_decodes_shift_and_filter() {
        let mut ram = Box::new([0u8; 0x10000]);
        // Shift 12, filter 0, end+loop: nibbles 7,-8,1,0 ...
        ram[0x1000] = 0xC3;
        ram[0x1001] = 0x78;
        ram[0x1002] = 0x10;
        let mut voice = Voice {
            addr: 0x1000,
            ..Default::default()
        };
        assert!(decode_block(&mut voice, &ram));
        assert_eq!(&voice.samples[3..7], &[0x3800, -0x4000, 0x0800, 0]);
    }

    #[test]
    fn a_keyed_on_voice_sounds_and_stops_at_its_end() {
        let mut ram = Box::new([0u8; 0x10000]);
        // DIR at $0200: sample 0 starts at $1000 and loops there; a square-ish block.
        ram[0x0200..0x0204].copy_from_slice(&[0x00, 0x10, 0x00, 0x10]);
        ram[0x1000] = 0xC3;
        for i in 0..8 {
            ram[0x1001 + i] = if i < 4 { 0x77 } else { 0x99 };
        }
        let mut dsp = Dsp::new();
        for (reg, value) in [
            (0x6C, 0x20), // unmute, no reset, echo writes off
            (0x0C, 0x7F),
            (0x1C, 0x7F), // master volume
            (0x00, 0x7F),
            (0x01, 0x7F), // voice 0 volume
            (0x02, 0x00),
            (0x03, 0x10), // pitch $1000 = 32 kHz
            (0x04, 0x00),
            (0x07, 0x7F), // source 0, direct gain max
            (0x5D, 0x02),
            (0x4C, 0x01), // key on
        ] {
            dsp.write(reg, value);
        }
        let loud = (0..64)
            .map(|_| dsp.sample(&mut ram).0.unsigned_abs())
            .max()
            .unwrap();
        assert!(loud > 0x1000, "the voice plays: {loud:#X}");
        assert_eq!(dsp.read(0x7C) & 1, 1, "ENDX set by the end block");
        dsp.write(0x5C, 0x01); // key off: release by 8 per sample
        for _ in 0..400 {
            dsp.sample(&mut ram);
        }
        assert_eq!(dsp.voices[0].env, 0, "released");
    }
}
