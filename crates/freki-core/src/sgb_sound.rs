//! The Super Game Boy's own sound: the SNES sound chip ([`crate::spc700`] and
//! [`crate::snes_dsp`]) running Nintendo's N-SPC engine from the SGB cartridge's SNES ROM,
//! which games drive with the `SOUND` and `SOU_TRN` commands.
//!
//! On the real cartridge, the SGB's SNES program uploads the engine and its samples at
//! start-up and passes the commands on through the four APU ports. freki does that part
//! directly (the SNES CPU is not emulated), following what the SNES program in the SGB
//! ROM does:
//!
//! - start-up: the upload chain in the ROM (the engine at $0400, the sample directory at
//!   $4B00, samples from $4DB0) goes into the sound RAM and the engine starts at $0400;
//! - `SOUND` (command $08): packet bytes 1–4 go to ports 1, 2, 3 and 0 (effect A, effect
//!   B, attributes, music score) for a frame, then ports 0–2 go back to 0;
//! - `SOU_TRN` (command $09): the transferred 4 KiB hold upload blocks (size, address,
//!   data; size 0 ends with the start address), which replace parts of the sound RAM before
//!   the program restarts there.

use crate::spc700::{self, Smp};

/// Game Boy T-cycles per second (the SGB's GB CPU runs a little faster on a real SNES;
/// freki keeps the Game Boy's own clock).
const GB_HZ: u64 = 4_194_304;
/// How long the SNES program leaves a `SOUND` command on the ports: about one frame.
const PORT_HOLD: u32 = 70_224;
/// Game Boy cycles collected before the sound chip catches up.
const BATCH: u32 = 1024;
/// Levels of the two sounds while the SGB has its own. bsnes (whose SGB runs the real
/// SNES side with SameBoy for the Game Boy) adds both at the same 16-bit scale, where a
/// Game Boy channel at full volume reaches ±4080 of 32768 (SameBoy's `MAX_CH_AMP`) —
/// half of freki's ±0.25. So the SNES sound is twice freki's Game Boy scale; both are
/// lowered together so their sum stays within ±1. (No measurement of a real SGB found.)
pub const GB_LEVEL: f32 = 0.7;
const LEVEL: f32 = 2.0 * GB_LEVEL;

pub struct SgbSound {
    smp: Smp,
    /// Game Boy cycles not yet run on the sound chip, in units of 1/GB_HZ second ×
    /// spc700 clock (to keep the fraction).
    pending: u32,
    remainder: u64,
    /// Cycles until ports 0–2 are cleared after a `SOUND` command.
    hold: u32,
    /// 32 kHz output not yet resampled.
    produced: Vec<(i16, i16)>,
    /// Output rate and resampler state (see `resample`).
    rate: Option<u32>,
    phase: u32,
    last: (f32, f32),
    /// Resampled output waiting to be mixed in, interleaved stereo.
    ready: Vec<f32>,
}

/// The SNES ROM's upload chain: blocks of (address, data) and the start address.
pub struct Chain {
    pub blocks: Vec<(u16, Vec<u8>)>,
    pub entry: u16,
}

/// Reads an upload chain (size, address, data …; size 0 then the start address) from
/// `data` at `at`. `None` if it does not hold together.
pub fn read_chain(data: &[u8], mut at: usize) -> Option<Chain> {
    let mut blocks = Vec::new();
    let word = |at: usize| Some(u16::from_le_bytes([*data.get(at)?, *data.get(at + 1)?]));
    loop {
        let (size, addr) = (usize::from(word(at)?), word(at + 2)?);
        if size == 0 {
            return Some(Chain {
                blocks,
                entry: addr,
            });
        }
        if usize::from(addr) + size > 0x10000 || blocks.len() >= 64 {
            return None;
        }
        blocks.push((addr, data.get(at + 4..at + 4 + size)?.to_vec()));
        at += 4 + size;
    }
}

/// Finds the sound engine's upload chain in an SGB SNES ROM: the first chain that starts
/// the engine at $0400, holds a block there, and more than 32 KiB in all (engine,
/// tables and samples). In the SGB ROM (Rev 2) it is at $06:8000: tables at $4C30 and
/// $4C10, the engine at $0400, the sample directory at $4B00 and samples from $4DB0.
pub fn find_engine(rom: &[u8]) -> Option<Chain> {
    (0..rom.len().saturating_sub(4)).find_map(|at| {
        let size = u16::from_le_bytes([rom[at], rom[at + 1]]);
        if size == 0 {
            return None;
        }
        let chain = read_chain(rom, at)?;
        let total: usize = chain.blocks.iter().map(|(_, d)| d.len()).sum();
        let engine = chain.blocks.iter().any(|(addr, _)| *addr == 0x0400);
        // Uploads never go to the direct page or the stack ($0000–$01FF); a "chain"
        // that does starts inside other data.
        let plausible = chain.blocks.iter().all(|(addr, _)| *addr >= 0x0200);
        (chain.entry == 0x0400 && engine && plausible && total > 0x8000).then_some(chain)
    })
}

impl SgbSound {
    /// The SGB's sound from its SNES ROM (`Super Game Boy (World) (Rev 2).sfc` and the
    /// like).
    pub fn new(rom: &[u8]) -> Result<Self, String> {
        let chain = find_engine(rom).ok_or("no SGB sound engine found in this ROM")?;
        let mut smp = Smp::new();
        for (addr, data) in &chain.blocks {
            smp.upload(*addr, data);
        }
        smp.start(chain.entry);
        Ok(Self {
            smp,
            pending: 0,
            remainder: 0,
            hold: 0,
            produced: Vec::new(),
            rate: None,
            phase: 0,
            last: (0.0, 0.0),
            ready: Vec::new(),
        })
    }

    /// `SOUND`: effect A, effect B, attributes and music score to ports 1, 2, 3 and 0.
    pub fn command(&mut self, packet: &[u8]) {
        let byte = |i: usize| packet.get(i).copied().unwrap_or(0);
        self.smp.input = [byte(4), byte(1), byte(2), byte(3)];
        self.hold = PORT_HOLD;
    }

    /// `SOU_TRN`: uploads the blocks in `data` and starts the program they name.
    pub fn transfer(&mut self, data: &[u8]) {
        let Some(chain) = read_chain(data, 0) else {
            return;
        };
        for (addr, block) in &chain.blocks {
            self.smp.upload(*addr, block);
        }
        self.smp.input = [0; 4];
        self.smp.start(chain.entry);
    }

    /// Output rate for [`SgbSound::mix_into`] (None: no sound is produced).
    pub fn set_rate(&mut self, rate: Option<u32>) {
        self.rate = rate;
    }

    /// Runs the sound chip along with `cycles` Game Boy T-cycles.
    pub fn tick(&mut self, cycles: u32) {
        if self.hold > 0 {
            self.hold = self.hold.saturating_sub(cycles);
            if self.hold == 0 {
                self.smp.input[0] = 0;
                self.smp.input[1] = 0;
                self.smp.input[2] = 0;
            }
        }
        self.pending += cycles;
        if self.pending < BATCH {
            return;
        }
        let total = u64::from(self.pending) * u64::from(spc700::CLOCK_HZ) + self.remainder;
        self.pending = 0;
        let spc_cycles = total / GB_HZ;
        self.remainder = total % GB_HZ;
        self.smp.run(spc_cycles as u32, &mut self.produced);
        self.resample();
    }

    /// Turns the 32 kHz output into samples at the output rate (linear interpolation).
    fn resample(&mut self) {
        let Some(rate) = self.rate else {
            self.produced.clear();
            return;
        };
        let source = spc700::CLOCK_HZ / spc700::CYCLES_PER_SAMPLE;
        for (l, r) in self.produced.drain(..) {
            let next = (f32::from(l) / 32768.0, f32::from(r) / 32768.0);
            // Time counts in 1/(32000 × rate) s: an input sample lasts `rate`, an output
            // sample `source`. Output samples falling between the last and this input:
            while self.phase < rate {
                let t = self.phase as f32 / rate as f32;
                self.ready
                    .push((self.last.0 + (next.0 - self.last.0) * t) * LEVEL);
                self.ready
                    .push((self.last.1 + (next.1 - self.last.1) * t) * LEVEL);
                self.phase += source;
            }
            self.phase -= rate;
            self.last = next;
        }
        // Never let the sound run far ahead of the Game Boy's (a dropped frame or a
        // missing drain).
        let max = (rate as usize / 5) * 2;
        if self.ready.len() > max {
            let excess = self.ready.len() - max;
            self.ready.drain(..excess & !1);
        }
    }

    /// Mixes the sound produced so far into `out` (interleaved stereo, the Game Boy's
    /// samples at the same rate, which are lowered to [`GB_LEVEL`]); what doesn't fit
    /// waits for the next call.
    pub fn mix_into(&mut self, out: &mut [f32]) {
        for o in out.iter_mut() {
            *o *= GB_LEVEL;
        }
        let n = out.len().min(self.ready.len());
        for (o, s) in out.iter_mut().zip(self.ready.drain(..n)) {
            *o += s;
        }
    }

    pub fn smp(&self) -> &Smp {
        &self.smp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chains_are_read_and_found() {
        let mut rom = vec![0xFFu8; 0x100];
        // A chain: 2 bytes to $0400, 1 byte to $0600, start $0400.
        rom.extend([
            0x02, 0x00, 0x00, 0x04, 0xAA, 0xBB, 0x01, 0x00, 0x00, 0x06, 0xCC,
        ]);
        rom.extend([0x00, 0x00, 0x00, 0x04]);
        let chain = read_chain(&rom, 0x100).unwrap();
        assert_eq!(chain.entry, 0x0400);
        assert_eq!(
            chain.blocks,
            vec![(0x0400, vec![0xAA, 0xBB]), (0x0600, vec![0xCC])]
        );
        assert!(find_engine(&rom).is_none(), "too small to be the engine");
        assert!(
            read_chain(&[0x05, 0x00, 0x00, 0x04], 0).is_none(),
            "cut short"
        );
    }

    #[test]
    fn resampling_keeps_time() {
        let mut rom = vec![0u8; 0x10];
        rom.extend([0x00, 0x90, 0x00, 0x04]);
        rom.extend(vec![0xFF; 0x9000]);
        rom.extend([0x00, 0x00, 0x00, 0x04]);
        let mut sound = SgbSound::new(&rom).unwrap();
        sound.set_rate(Some(48_000));
        sound.tick(GB_HZ as u32 / 10); // 0.1 s
        let frames = sound.ready.len() / 2;
        assert!(
            (4790..=4810).contains(&frames),
            "{frames} samples for 0.1 s at 48 kHz"
        );
    }

    #[test]
    fn sound_commands_reach_the_ports_for_a_frame() {
        let mut rom = vec![0u8; 0x10];
        // An "engine": 0x9000 bytes of STOP at $0400, starting at $0400.
        rom.extend([0x00, 0x90, 0x00, 0x04]);
        rom.extend(vec![0xFF; 0x9000]);
        rom.extend([0x00, 0x00, 0x00, 0x04]);
        let mut sound = SgbSound::new(&rom).unwrap();
        sound.command(&[0x41, 0x0B, 0x0C, 0x05, 0x00]);
        assert_eq!(sound.smp().input, [0x00, 0x0B, 0x0C, 0x05]);
        sound.tick(PORT_HOLD);
        assert_eq!(
            sound.smp().input,
            [0x00, 0x00, 0x00, 0x05],
            "attributes stay"
        );
    }
}
