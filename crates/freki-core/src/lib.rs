//! freki-core: a platform-independent Game Boy (DMG) emulation core.
//!
//! This crate has no OS, windowing or audio dependencies. A frontend feeds it
//! a ROM and button state, calls [`GameBoy::run_frame`] ~59.7 times a second,
//! and draws [`GameBoy::framebuffer`].

pub mod apu;
pub mod bus;
pub mod cartridge;
pub mod cpu;
pub mod joypad;
pub mod ppu;
pub mod serial;
pub mod timer;

pub use cartridge::{Cartridge, CartridgeError};
pub use cpu::CpuError;
pub use joypad::Button;
pub use ppu::{SCREEN_HEIGHT, SCREEN_WIDTH};

use bus::Bus;
use cpu::Cpu;

/// CPU clock in T-cycles per second.
pub const CLOCK_HZ: u32 = 4_194_304;
/// T-cycles in one full frame (154 scanlines × 456 dots).
pub const CYCLES_PER_FRAME: u32 = 70_224;
/// Native refresh rate (~59.73 Hz).
pub const FRAMES_PER_SECOND: f64 = CLOCK_HZ as f64 / CYCLES_PER_FRAME as f64;

/// A complete Game Boy: CPU plus everything on the memory bus.
pub struct GameBoy {
    cpu: Cpu,
    bus: Bus,
}

impl GameBoy {
    /// Creates a Game Boy with `rom` inserted, in the post-boot-ROM state.
    pub fn new(rom: Vec<u8>) -> Result<Self, CartridgeError> {
        let cart = Cartridge::from_rom(rom)?;
        let logo: [u8; 48] = std::array::from_fn(|i| cart.read_rom(0x104 + i as u16));
        let mut bus = Bus::new(cart);
        bus.ppu.load_boot_logo(&logo);
        Ok(Self {
            cpu: Cpu::new_post_boot(),
            bus,
        })
    }

    /// Executes one CPU instruction. The rest of the hardware is advanced during the
    /// instruction, one M-cycle at a time. Returns the number of T-cycles it took.
    pub fn step(&mut self) -> Result<u32, CpuError> {
        self.cpu.step(&mut self.bus)
    }

    /// Runs until the PPU has produced a complete frame.
    pub fn run_frame(&mut self) -> Result<(), CpuError> {
        let mut elapsed = 0;
        while elapsed < CYCLES_PER_FRAME {
            elapsed += self.step()?;
            if self.bus.ppu.take_frame_ready() {
                break;
            }
        }
        Ok(())
    }

    /// The current screen as 160×144 shade indices (0 = lightest, 3 = darkest).
    pub fn framebuffer(&self) -> &[u8; SCREEN_WIDTH * SCREEN_HEIGHT] {
        self.bus.ppu.framebuffer()
    }

    pub fn set_button(&mut self, button: Button, pressed: bool) {
        self.bus.joypad.set(button, pressed);
    }

    /// Address of the next instruction to execute.
    pub fn pc(&self) -> u16 {
        self.cpu.regs.pc
    }

    /// Reads a byte from the memory map without side effects (for debuggers and tests).
    pub fn peek(&self, addr: u16) -> u8 {
        self.bus.read(addr)
    }

    /// Every byte the game has sent over the serial port (test ROMs print results here).
    pub fn serial_output(&self) -> &[u8] {
        self.bus.serial.output()
    }

    /// Starts producing audio at `rate` Hz (e.g. the output device's rate). Until this is
    /// called, no samples are generated.
    pub fn set_audio_sample_rate(&mut self, rate: u32) {
        self.bus.apu.set_sample_rate(rate);
    }

    /// Moves the audio produced so far into `out` as interleaved stereo `f32` samples.
    pub fn drain_audio(&mut self, out: &mut Vec<f32>) {
        self.bus.apu.drain_samples(out);
    }

    /// Battery-backed cartridge RAM to write to a save file, or `None` if the cartridge
    /// has no battery.
    pub fn save_data(&self) -> Option<&[u8]> {
        self.bus.cart.has_battery().then(|| self.bus.cart.ram())
    }

    /// Restores cartridge RAM from a save file.
    pub fn load_save_data(&mut self, data: &[u8]) {
        self.bus.cart.load_ram(data);
    }

    /// Returns `true` if cartridge RAM changed since the last call (i.e. the save is stale).
    pub fn take_save_dirty(&mut self) -> bool {
        self.bus.cart.take_ram_dirty()
    }

    pub fn cartridge(&self) -> &Cartridge {
        &self.bus.cart
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 32 KiB ROM-only image whose entry point is `JP $0150` and whose code
    /// at $0150 is an endless `JR -2` loop.
    pub(crate) fn test_rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]); // NOP; JP $0150
        rom[0x134..0x13A].copy_from_slice(b"FREKI\0");
        rom[0x147] = 0x00; // ROM only
        rom[0x148] = 0x00; // 32 KiB
        rom[0x149] = 0x00; // no RAM
        rom[0x150..0x152].copy_from_slice(&[0x18, 0xFE]); // JR -2
        rom
    }

    #[test]
    fn runs_a_frame_without_errors() {
        let mut gb = GameBoy::new(test_rom()).unwrap();
        gb.run_frame().unwrap();
        assert_eq!(gb.cartridge().title(), "FREKI");
    }
}
