//! A Super Game Boy with its SNES side emulated as hardware ([`crate::snes`]): the SGB
//! cartridge's own SNES program receives the Game Boy's picture and packets through the
//! ICD2 and does everything the SGB does, including running code that games upload
//! (Space Invaders' arcade mode). The alternative to [`crate::sgb`]'s built-in model.
//!
//! The Game Boy is held in reset until the SNES program releases it (`$6003` bit 7). Then
//! the SGB's Game Boy boot ROM would send six packets with the cartridge header ($104–$14F);
//! freki, which starts games after the boot ROM, sends them itself (in the format of
//! SameBoy's open-source SGB boot ROM: $F1 + 2·n, checksum, 14 header bytes) before letting
//! the game run.

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

use crate::joypad::Button;
use crate::sgb_sound::Resampler;
use crate::snes::{GB_DIVIDER, Snes};

/// Game Boy T-cycles per frame, and frames between the boot packets.
const GB_FRAME: u32 = 70_224;
const BOOT_PACKET_FRAMES: u32 = 4;
/// Game Boy cycles collected before the SNES catches up.
const BATCH: u32 = 64;

/// The SNES joypad's shoulder buttons: together they open the SGB's own menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SgbShoulder {
    L,
    R,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
enum Phase {
    /// In reset (power-on until the SNES program releases it).
    Held,
    /// "Boot ROM": sending header packet `sent` next, `wait` cycles from now.
    Booting {
        sent: u8,
        wait: u32,
    },
    Running,
}

#[derive(Serialize, Deserialize)]
pub struct SgbLle {
    pub snes: Snes,
    #[serde(with = "BigArray")]
    header: [u8; 84],
    phase: Phase,
    /// Game Boy cycles the SNES has not run yet, and the master clock it has to reach
    /// (kept apart from the SNES's own, which an instruction can carry past it).
    debt: u32,
    target: u64,
    /// The Game Boy LCD line last seen, to capture each finished line once.
    last_ly: u8,
    /// The SNES program reset the Game Boy after it had started: it must start over.
    gb_reset: bool,
    /// The SNES's sound, on its way to the output (taken over across state loads).
    #[serde(skip, default = "new_resampler")]
    out: Resampler,
}

/// The SNES sound chip's 32 kHz counted in Game Boy time at [`crate::CLOCK_HZ`] (as the
/// APU counts it; [`crate::GameBoy::clock_hz`] makes both run at the SGB's real speed).
fn new_resampler() -> Resampler {
    Resampler::new(SOUND_HZ)
}

const SOUND_HZ: u32 = (32_000 * 4_194_304 * GB_DIVIDER as u64 / crate::snes::MASTER_HZ) as u32;

impl SgbLle {
    /// `sfc`: the SGB cartridge's SNES ROM; `gb_rom`: the game, for its header.
    pub fn new(sfc: Vec<u8>, gb_rom: &[u8]) -> Self {
        let mut header = [0; 84];
        for (i, b) in header.iter_mut().enumerate() {
            let a = 0x104 + i;
            *b = if a <= 0x14F {
                gb_rom.get(a).copied().unwrap_or(0)
            } else {
                0
            };
        }
        SgbLle {
            snes: Snes::new(sfc),
            header,
            phase: Phase::Held,
            debt: 0,
            target: 0,
            last_ly: 0xFF,
            gb_reset: false,
            out: new_resampler(),
        }
    }

    /// After a state load into `self`: the ROM and the sound output from the machine it
    /// replaces.
    pub fn adopt_runtime_from(&mut self, old: &mut SgbLle) {
        self.snes.bus.rom = std::mem::take(&mut old.snes.bus.rom);
        self.out = std::mem::replace(&mut old.out, new_resampler());
    }

    /// Whether the SNES has its cartridge ROM (states don't carry it).
    pub fn has_rom(&self) -> bool {
        !self.snes.bus.rom.is_empty()
    }

    /// Whether the Game Boy has to start over (the SNES program reset it).
    pub fn take_gb_reset(&mut self) -> bool {
        std::mem::take(&mut self.gb_reset)
    }

    /// Whether the Game Boy's CPU is stopped (in reset, or in the "boot ROM").
    pub fn holds_gb(&self) -> bool {
        self.phase != Phase::Running
    }

    /// Runs the SNES for `cycles` Game Boy T-cycles. `line`: the Game Boy LCD line now
    /// being drawn and the screen so far, to capture finished lines.
    pub fn tick(&mut self, cycles: u32, line: Option<(u8, &[u8])>) {
        if let Some((ly, screen)) = line
            && ly != self.last_ly
        {
            if (self.last_ly as usize) < crate::ppu::SCREEN_HEIGHT {
                let y = self.last_ly as usize;
                let row = &screen[y * crate::ppu::SCREEN_WIDTH..][..crate::ppu::SCREEN_WIDTH];
                self.snes.bus.icd2.gb_line(self.last_ly, row);
            }
            self.last_ly = ly;
        }
        self.debt += cycles;
        if self.debt >= BATCH {
            self.run_snes();
        }
        self.advance_phase(cycles);
    }

    fn run_snes(&mut self) {
        self.target += self.debt as u64 * GB_DIVIDER as u64;
        self.debt = 0;
        self.snes.run_until(self.target);
        self.out.push(&mut self.snes.bus.audio);
    }

    /// Output rate for the SNES's sound (None: none is produced).
    pub fn set_audio_rate(&mut self, rate: Option<u32>) {
        self.out.set_rate(rate);
    }

    /// Mixes the SNES's sound into the Game Boy's (see [`Resampler::mix_into`]).
    pub fn mix_into(&mut self, out: &mut [f32]) {
        self.out.mix_into(out);
    }

    fn advance_phase(&mut self, cycles: u32) {
        let in_reset = self.snes.bus.icd2.gb_in_reset();
        if in_reset && self.phase != Phase::Held {
            self.gb_reset = true;
        }
        self.phase = match self.phase {
            _ if in_reset => Phase::Held,
            Phase::Held => Phase::Booting {
                sent: 0,
                wait: GB_FRAME * BOOT_PACKET_FRAMES,
            },
            Phase::Booting { sent, wait } if wait > cycles => Phase::Booting {
                sent,
                wait: wait - cycles,
            },
            Phase::Booting { sent, .. } => {
                self.send_header_packet(sent);
                if sent == 5 {
                    Phase::Running
                } else {
                    Phase::Booting {
                        sent: sent + 1,
                        wait: GB_FRAME * BOOT_PACKET_FRAMES,
                    }
                }
            }
            Phase::Running => Phase::Running,
        };
    }

    fn send_header_packet(&mut self, n: u8) {
        let bytes = &self.header[n as usize * 14..][..14];
        let mut packet = [0; 16];
        packet[0] = 0xF1 + 2 * n;
        packet[1] = bytes.iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
        packet[2..].copy_from_slice(bytes);
        self.snes.bus.icd2.receive_packet(&packet);
    }

    /// A packet the game sent.
    pub fn packet(&mut self, packet: &[u8]) {
        let mut p = [0; 16];
        p.copy_from_slice(&packet[..16]);
        self.snes.bus.icd2.receive_packet(&p);
    }

    /// Players the joypad port cycles through.
    pub fn players(&self) -> u8 {
        self.snes.bus.icd2.players()
    }

    /// The low nibble of P1 for `player` with `select` (P1 bits 4–5) as written.
    pub fn joypad_nibble(&self, player: u8, select: u8) -> u8 {
        let pad = self.snes.bus.icd2.joypads[player as usize & 3];
        let mut v = 0x0F;
        if select & 0x10 == 0 {
            v &= pad & 0x0F;
        }
        if select & 0x20 == 0 {
            v &= pad >> 4;
        }
        v
    }

    /// A button on the SNES joypad 1, from the Game Boy button it stands for.
    pub fn set_button(&mut self, button: Button, pressed: bool) {
        self.set_pad_bit(
            match button {
                Button::Right => 0x0100,
                Button::Left => 0x0200,
                Button::Up => 0x0800,
                Button::Down => 0x0400,
                Button::A => 0x0080,
                Button::B => 0x8000,
                Button::Select => 0x2000,
                Button::Start => 0x1000,
            },
            pressed,
        );
    }

    /// L or R on the SNES joypad 1 (the Game Boy has no such buttons).
    pub fn set_shoulder(&mut self, shoulder: SgbShoulder, pressed: bool) {
        let bit = match shoulder {
            SgbShoulder::L => 0x0020,
            SgbShoulder::R => 0x0010,
        };
        self.set_pad_bit(bit, pressed);
    }

    fn set_pad_bit(&mut self, bit: u16, pressed: bool) {
        let pad = &mut self.snes.bus.pads[0];
        if pressed {
            *pad |= bit;
        } else {
            *pad &= !bit;
        }
    }
}
