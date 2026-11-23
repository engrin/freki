//! A link cable between two Game Boys running in the same process.
//!
//! [`Link::run_frame`] runs both machines in lockstep, one instruction at a time, always
//! advancing the one that is behind in real time, so they never drift more than an
//! instruction apart. When a game starts a transfer with its own (internal) clock and the
//! other game is waiting with the external clock, the two bytes are swapped: each side
//! shifts in the other's byte, and the waiting side finishes (with its serial interrupt)
//! when the clocking side does. If the other side is not waiting, the clocking side
//! receives $FF, as with no cable attached.
//!
//! The two also face each other: each one's infrared sensor (the CGB's RP port, HuC1/HuC3
//! cartridges) sees the other's LED.

use std::fmt;

use crate::{CpuError, GameBoy};

/// Which Game Boy of a linked pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Player {
    One,
    Two,
}

/// An emulation error on one side of the link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkError {
    pub player: Player,
    pub error: CpuError,
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = match self.player {
            Player::One => 1,
            Player::Two => 2,
        };
        write!(f, "Game Boy {n}: {}", self.error)
    }
}

impl std::error::Error for LinkError {}

/// The cable's state between frames.
#[derive(Debug, Default, Clone)]
pub struct Link {
    /// How far the second Game Boy is ahead of the first, in half T-cycles of normal
    /// speed (a double-speed T-cycle is one, a normal one two).
    lead: i64,
}

impl Link {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs until the first Game Boy has finished a frame, keeping the second one in step
    /// (its frames finish whenever they do). Cheats are applied after each one's frames.
    pub fn run_frame(&mut self, one: &mut GameBoy, two: &mut GameBoy) -> Result<(), LinkError> {
        let mut elapsed = 0;
        loop {
            if self.lead <= 0 {
                let (cycles, _) = two.step_linked().map_err(|error| LinkError {
                    player: Player::Two,
                    error,
                })?;
                self.lead += half_cycles(two, cycles);
                exchange(two, one);
            } else {
                let (cycles, frame) = one.step_linked().map_err(|error| LinkError {
                    player: Player::One,
                    error,
                })?;
                self.lead -= half_cycles(one, cycles);
                exchange(one, two);
                elapsed += cycles;
                if frame || elapsed >= one.frame_cycles() {
                    return Ok(());
                }
            }
        }
    }
}

/// `cycles` T-cycles of `gb` in half T-cycles of normal speed.
fn half_cycles(gb: &GameBoy, cycles: u32) -> i64 {
    i64::from(cycles) << u32::from(!gb.bus.double_speed)
}

/// Passes on what `from` just did on the cable to `to`, and shows `to` its infrared LED
/// (the two face each other).
fn exchange(from: &mut GameBoy, to: &mut GameBoy) {
    to.bus.ir_input = from.bus.ir_led();
    if let Some(byte) = from.bus.serial.take_started() {
        let back = if to.bus.serial.external_ready() {
            to.bus.serial.start_external(byte)
        } else {
            0xFF
        };
        from.bus.serial.set_incoming(back);
    }
    if from.bus.serial.take_finished() {
        to.bus.serial.finish_external();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Model;

    /// A ROM that runs `code` at $150 (after a jump at the entry point).
    fn rom(code: &[u8]) -> Vec<u8> {
        let mut rom = vec![0; 0x8000];
        rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
        rom[0x150..0x150 + code.len()].copy_from_slice(code);
        rom
    }

    #[test]
    fn swaps_bytes_and_interrupts_the_waiting_side() {
        // Waits with the external clock: SB = $22, SC = $80, then spins.
        let waiting = rom(&[0x3E, 0x22, 0xE0, 0x01, 0x3E, 0x80, 0xE0, 0x02, 0x18, 0xFE]);
        // Waits a little, then sends $11 with the internal clock: SB = $11, SC = $81.
        let clocking = rom(&[
            0x06, 0x40, 0x05, 0x20, 0xFD, // delay loop
            0x3E, 0x11, 0xE0, 0x01, 0x3E, 0x81, 0xE0, 0x02, 0x18, 0xFE,
        ]);
        let mut one = GameBoy::with_model(clocking, Model::Dmg).unwrap();
        let mut two = GameBoy::with_model(waiting, Model::Dmg).unwrap();
        let mut link = Link::new();
        for _ in 0..3 {
            link.run_frame(&mut one, &mut two).unwrap();
        }
        assert_eq!(one.peek(0xFF01), 0x22);
        assert_eq!(two.peek(0xFF01), 0x11);
        assert_eq!(one.peek(0xFF02) & 0x80, 0, "the clocking side finished");
        assert_eq!(two.peek(0xFF02) & 0x80, 0, "the waiting side finished");
        assert_ne!(
            two.peek(0xFF0F) & 0x08,
            0,
            "serial interrupt on the waiting side"
        );
        assert_ne!(
            one.peek(0xFF0F) & 0x08,
            0,
            "serial interrupt on the clocking side"
        );
    }

    #[test]
    fn nobody_waiting_sends_ff() {
        let clocking = rom(&[0x3E, 0x11, 0xE0, 0x01, 0x3E, 0x81, 0xE0, 0x02, 0x18, 0xFE]);
        let idle = rom(&[0x18, 0xFE]);
        let mut one = GameBoy::with_model(idle, Model::Dmg).unwrap();
        let mut two = GameBoy::with_model(clocking, Model::Dmg).unwrap();
        let mut link = Link::new();
        link.run_frame(&mut one, &mut two).unwrap();
        assert_eq!(two.peek(0xFF01), 0xFF);
    }

    #[test]
    fn infrared_ports_see_each_other() {
        let cgb = |code: &[u8]| {
            let mut rom = rom(code);
            rom[0x143] = 0x80;
            rom
        };
        // RP = $C1: reading enabled, LED on; RP = $C0: reading enabled, LED off.
        let sending = cgb(&[0x3E, 0xC1, 0xE0, 0x56, 0x18, 0xFE]);
        let receiving = cgb(&[0x3E, 0xC0, 0xE0, 0x56, 0x18, 0xFE]);
        let mut one = GameBoy::with_model(receiving.clone(), Model::Cgb).unwrap();
        let mut two = GameBoy::with_model(receiving, Model::Cgb).unwrap();
        let mut link = Link::new();
        link.run_frame(&mut one, &mut two).unwrap();
        assert_eq!(one.peek(0xFF56) & 0xC3, 0xC2, "dark");
        let mut two = GameBoy::with_model(sending, Model::Cgb).unwrap();
        link.run_frame(&mut one, &mut two).unwrap();
        assert_eq!(one.peek(0xFF56) & 0xC3, 0xC0, "light seen");
        assert_eq!(two.peek(0xFF56) & 0xC3, 0xC1, "its own LED");
    }

    #[test]
    fn keeps_both_in_step() {
        let spin = rom(&[0x18, 0xFE]);
        let mut one = GameBoy::with_model(spin.clone(), Model::Dmg).unwrap();
        let mut two = GameBoy::with_model(spin, Model::Dmg).unwrap();
        let mut link = Link::new();
        for _ in 0..10 {
            link.run_frame(&mut one, &mut two).unwrap();
            assert!(link.lead.abs() <= 2 * 24, "lead {}", link.lead);
        }
    }
}
