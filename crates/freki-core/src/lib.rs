//! freki-core: a platform-independent Game Boy (DMG) and Game Boy Color emulation core.
//!
//! This crate has no OS, windowing or audio dependencies. A frontend feeds it
//! a ROM and button state, calls [`GameBoy::run_frame`] ~59.7 times a second,
//! and draws [`GameBoy::framebuffer`].

pub mod apu;
pub mod boot;
pub mod bus;
pub mod cartridge;
pub mod cheats;
pub mod cpu;
pub mod debugger;
pub mod disasm;
pub mod huc3;
pub mod joypad;
pub mod mbc7;
pub mod mmm01;
pub mod ppu;
pub mod serial;
pub mod state;
pub mod timer;

pub use boot::{BootRom, BootRomError};
pub use cartridge::{Cartridge, CartridgeError};
pub use cheats::{CheatError, Cheats, Code, Filter, Location, MemorySearch};
pub use cpu::{CpuError, Registers};
pub use debugger::{Access, Breakpoints, Stop, WatchHit, Watchpoint};
pub use disasm::{Instruction, disassemble};
pub use joypad::Button;

pub use ppu::{SCREEN_HEIGHT, SCREEN_WIDTH};
pub use state::{Rewind, StateError};

use bus::Bus;
use cpu::Cpu;

/// CPU clock in T-cycles per second.
pub const CLOCK_HZ: u32 = 4_194_304;
/// T-cycles in one full frame (154 scanlines × 456 dots).
pub const CYCLES_PER_FRAME: u32 = 70_224;
/// Native refresh rate (~59.73 Hz).
pub const FRAMES_PER_SECOND: f64 = CLOCK_HZ as f64 / CYCLES_PER_FRAME as f64;

/// DIV counter when the CGB boot ROM hands over to a CGB game (from Gambatte's hwtests,
/// div/start_inc_*_cgb04c; freki's run of the boot ROM ends 24 T-cycles earlier), and to a
/// DMG game (measured with the real boot ROM; it varies a little with the header).
const CGB_BOOT_COUNTER: u16 = 0x1EA0;
const CGB_COMPAT_BOOT_COUNTER: u16 = 0x2678;

/// What the CGB boot ROM leaves in B for a DMG game: the sum of the title bytes if the
/// licensee is Nintendo, otherwise 0.
fn cart_title_sum(cart: &Cartridge) -> u8 {
    let old = cart.read_rom(0x14B);
    let new = [cart.read_rom(0x144), cart.read_rom(0x145)];
    if old == 0x01 || (old == 0x33 && new == *b"01") {
        (0x134..=0x143).fold(0u8, |sum, addr| sum.wrapping_add(cart.read_rom(addr)))
    } else {
        0
    }
}

/// Which hardware is emulated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    /// Original Game Boy.
    Dmg,
    /// Game Boy Color: CGB games in colour mode, original Game Boy games in its DMG
    /// compatibility mode.
    Cgb,
}

/// The current picture, 160×144 pixels row by row.
#[derive(Debug, Clone, Copy)]
pub enum Screen<'a> {
    /// DMG: shades 0 (lightest) to 3 (darkest); the frontend picks the colours.
    Shades(&'a [u8; SCREEN_WIDTH * SCREEN_HEIGHT]),
    /// CGB: RGB555 colours (bits 0–4 red, 5–9 green, 10–14 blue).
    Rgb555(&'a [u16]),
}

/// Converts an RGB555 colour to 8-bit RGB by bit replication (no colour correction).
pub fn rgb555_to_rgb888(color: u16) -> [u8; 3] {
    let channel = |shift: u16| {
        let c = ((color >> shift) & 0x1F) as u8;
        (c << 3) | (c >> 2)
    };
    [channel(0), channel(5), channel(10)]
}

/// A complete Game Boy: CPU plus everything on the memory bus.
pub struct GameBoy {
    cpu: Cpu,
    bus: Bus,
    /// Set after stopping at a breakpoint, so resuming executes that instruction.
    resume_at: Option<u16>,
}

impl GameBoy {
    /// Creates a Game Boy with `rom` inserted, in the post-boot-ROM state. Games made for
    /// the Game Boy Color run on a CGB; all others on a DMG.
    pub fn new(rom: Vec<u8>) -> Result<Self, CartridgeError> {
        let cgb = rom.get(0x143).is_some_and(|flag| flag & 0x80 != 0);
        Self::with_model(rom, if cgb { Model::Cgb } else { Model::Dmg })
    }

    /// Creates a Game Boy of the given model with `rom` inserted, in the post-boot state.
    /// On a [`Model::Cgb`], original Game Boy games run in DMG compatibility mode, in
    /// shades of grey (the CGB boot ROM, see [`GameBoy::with_boot_rom`], picks colours).
    pub fn with_model(rom: Vec<u8>, model: Model) -> Result<Self, CartridgeError> {
        let cart = Cartridge::from_rom(rom)?;
        let cgb_game = cart.supports_cgb();
        let logo: [u8; 48] = std::array::from_fn(|i| cart.read_rom(0x104 + i as u16));
        let mut bus = Bus::new(cart);
        bus.ppu.load_boot_logo(&logo);
        bus.ppu.skip_to_post_boot();
        let cpu = if model == Model::Cgb {
            bus.set_cgb();
            bus.ppu.skip_to_post_boot_cgb(!cgb_game);
            if cgb_game {
                bus.timer = timer::Timer::with_counter(CGB_BOOT_COUNTER);
                Cpu::new_post_boot_cgb()
            } else {
                bus.enter_dmg_compat();
                bus.ppu.set_grey_compat_palettes();
                bus.timer = timer::Timer::with_counter(CGB_COMPAT_BOOT_COUNTER);
                // The boot ROM leaves the joypad with neither button group selected.
                bus.joypad.write(0x30);
                Cpu::new_post_boot_cgb_compat(cart_title_sum(&bus.cart))
            }
        } else {
            Cpu::new_post_boot()
        };
        Ok(Self {
            cpu,
            bus,
            resume_at: None,
        })
    }

    /// The emulated hardware.
    pub fn model(&self) -> Model {
        if self.bus.cgb_hardware {
            Model::Cgb
        } else {
            Model::Dmg
        }
    }

    /// Creates a Game Boy with `rom` inserted, at power-on: `boot` runs first (logo and
    /// sound), then hands over to the game. The boot ROM decides the model: with the CGB
    /// boot ROM, original Game Boy games get colours it picks for them.
    pub fn with_boot_rom(rom: Vec<u8>, boot: BootRom) -> Result<Self, CartridgeError> {
        let cart = Cartridge::from_rom(rom)?;
        let cgb = boot.is_cgb();
        let mut bus = Bus::power_on(cart, boot);
        if cgb {
            bus.set_cgb();
            bus.timer = timer::Timer::power_on_cgb();
        }
        Ok(Self {
            cpu: Cpu::new_power_on(),
            bus,
            resume_at: None,
        })
    }

    /// Whether a Game Boy Color runs an original Game Boy game (DMG compatibility mode).
    pub fn dmg_compat(&self) -> bool {
        self.bus.cgb_hardware && !self.bus.cgb && !self.bus.boot_rom_mapped()
    }

    /// Whether the boot ROM is still mapped (it has not handed over to the game yet).
    pub fn in_boot_rom(&self) -> bool {
        self.bus.boot_rom_mapped()
    }

    /// Executes one CPU instruction. The rest of the hardware is advanced during the
    /// instruction, one M-cycle at a time. Returns the number of T-cycles it took.
    pub fn step(&mut self) -> Result<u32, CpuError> {
        self.cpu.step(&mut self.bus)
    }

    /// Advances everything but the CPU by `cycles` T-cycles, as a real Game Boy does once
    /// an illegal opcode has locked up its CPU (see [`CpuError::IllegalOpcode`]).
    pub fn run_without_cpu(&mut self, cycles: u32) {
        self.bus.tick(cycles);
    }

    /// Runs until the PPU has produced a complete frame, then applies GameShark cheats.
    /// Ignores breakpoints; see [`GameBoy::run_frame_checked`].
    pub fn run_frame(&mut self) -> Result<(), CpuError> {
        let mut elapsed = 0;
        while elapsed < self.frame_cycles() {
            elapsed += self.step()?;
            if self.bus.ppu.take_frame_ready() {
                break;
            }
        }
        self.apply_frame_cheats();
        Ok(())
    }

    /// CPU cycles in one frame: twice as many in CGB double speed. Bounds a frame while
    /// the LCD is off (no frame is produced then).
    fn frame_cycles(&self) -> u32 {
        CYCLES_PER_FRAME << u32::from(self.bus.double_speed)
    }

    fn apply_frame_cheats(&mut self) {
        let writes: Vec<(Location, u8)> = self.bus.cheats.frame_writes().collect();
        for (location, value) in writes {
            self.write_location(location, value);
        }
    }

    /// Executes one instruction, honouring breakpoints and watchpoints.
    ///
    /// Returns `Some(stop)` when a breakpoint is reached (the instruction has *not* run;
    /// the next call runs it) or a watchpoint was hit (the instruction has run).
    pub fn step_checked(&mut self) -> Result<Option<Stop>, CpuError> {
        self.step_checked_cycles().map(|(stop, _)| stop)
    }

    /// [`GameBoy::step_checked`], also returning the T-cycles taken.
    fn step_checked_cycles(&mut self) -> Result<(Option<Stop>, u32), CpuError> {
        let pc = self.cpu.regs.pc;
        let resume = self.resume_at.take() == Some(pc);
        if !resume && !self.cpu.halted && self.bus.breaks.pc.contains(&pc) {
            self.resume_at = Some(pc);
            return Ok((Some(Stop::Breakpoint { pc }), 0));
        }
        let cycles = self.step()?;
        let stop = self
            .bus
            .breaks
            .take_hit()
            .map(|hit| Stop::Watch { pc, hit });
        Ok((stop, cycles))
    }

    /// Like [`GameBoy::run_frame`], but stops early at breakpoints and watchpoints.
    /// After a stop, calling it again continues the same frame.
    pub fn run_frame_checked(&mut self) -> Result<Option<Stop>, CpuError> {
        if self.bus.breaks.is_empty() && self.resume_at.is_none() {
            return self.run_frame().map(|()| None);
        }
        // Counted from where the frame (re)started; resuming after a stop starts afresh,
        // which only matters with the LCD off.
        let mut elapsed = 0;
        loop {
            let (stop, cycles) = self.step_checked_cycles()?;
            if let Some(stop) = stop {
                return Ok(Some(stop));
            }
            elapsed += cycles;
            if self.bus.ppu.take_frame_ready() || elapsed >= self.frame_cycles() {
                self.apply_frame_cheats();
                return Ok(None);
            }
        }
    }

    /// Like [`GameBoy::run_frame_checked`], but calls `before` ahead of every step the CPU
    /// takes while not halted (an instruction, or an interrupt dispatch): for tracing.
    pub fn run_frame_traced(
        &mut self,
        mut before: impl FnMut(&Self),
    ) -> Result<Option<Stop>, CpuError> {
        let mut elapsed = 0;
        loop {
            if !self.cpu.halted {
                before(self);
            }
            let (stop, cycles) = self.step_checked_cycles()?;
            if let Some(stop) = stop {
                return Ok(Some(stop));
            }
            elapsed += cycles;
            if self.bus.ppu.take_frame_ready() || elapsed >= self.frame_cycles() {
                self.apply_frame_cheats();
                return Ok(None);
            }
        }
    }

    /// Breakpoints and watchpoints (not part of save states).
    pub fn breakpoints(&self) -> &Breakpoints {
        &self.bus.breaks
    }

    pub fn breakpoints_mut(&mut self) -> &mut Breakpoints {
        &mut self.bus.breaks
    }

    /// The current screen as 160×144 shade indices (0 = lightest, 3 = darkest).
    /// A VRAM byte from bank 0 or 1, whichever bank the CPU has selected (for debuggers).
    pub fn peek_vram(&self, bank: u8, addr: u16) -> u8 {
        self.bus.ppu.vram_in_bank(bank, addr)
    }

    /// CGB palette RAM (background, objects): 8 palettes × 4 colours, RGB555 little-endian.
    pub fn palette_ram(&self) -> (&[u8; 64], &[u8; 64]) {
        self.bus.ppu.palette_ram()
    }

    /// Whether the game uses Game Boy Color features (not on a DMG, nor in the GBC's
    /// compatibility mode for original Game Boy games).
    pub fn cgb_mode(&self) -> bool {
        self.bus.ppu.cgb_mode()
    }

    /// The current picture: shades on a DMG, colours on a CGB.
    pub fn screen(&self) -> Screen<'_> {
        if self.bus.ppu.outputs_rgb() {
            Screen::Rgb555(self.bus.ppu.rgb_framebuffer())
        } else {
            Screen::Shades(self.bus.ppu.framebuffer())
        }
    }

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

    /// CPU registers.
    pub fn registers(&self) -> &Registers {
        &self.cpu.regs
    }

    /// Interrupt master enable.
    pub fn ime(&self) -> bool {
        self.cpu.ime
    }

    /// Whether the CPU is halted, waiting for an interrupt.
    pub fn halted(&self) -> bool {
        self.cpu.halted
    }

    /// Disassembles the instruction at `addr` (reads without side effects).
    pub fn disassemble(&self, addr: u16) -> Instruction {
        disassemble(addr, |a| self.peek(a))
    }

    /// Reads a byte from the memory map without side effects (for debuggers and tests).
    pub fn peek(&self, addr: u16) -> u8 {
        self.bus.read(addr)
    }

    /// Writes a byte to the memory map, as the CPU would (for debuggers and cheats).
    pub fn poke(&mut self, addr: u16, value: u8) {
        self.bus.write(addr, value);
    }

    /// Starts a memory search over the RAM this cartridge actually has (work RAM, high RAM
    /// and, if present, cartridge RAM).
    /// Starts a memory search over cartridge RAM (if any), work RAM and high RAM. On the
    /// Game Boy Color it covers all seven work RAM banks at $D000–$DFFF.
    pub fn start_memory_search(&self) -> MemorySearch {
        let has_cart_ram = !self.bus.cart.ram().is_empty();
        let mut locations: Vec<Location> = Vec::new();
        for range in cheats::SEARCH_RANGES {
            match *range.start() {
                0xA000 if !has_cart_ram => {}
                0xC000 if self.bus.cgb => {
                    locations.extend((0xC000..=0xCFFF).map(Location::new));
                    for bank in 1..=7 {
                        locations.extend((0xD000..=0xDFFF).map(|address| Location {
                            address,
                            wram_bank: Some(bank),
                        }));
                    }
                }
                _ => locations.extend(range.map(Location::new)),
            }
        }
        MemorySearch::start_at(locations, |l| self.peek_location(l))
    }

    /// Reads `location` without side effects (a banked one from its work RAM bank).
    pub fn peek_location(&self, location: Location) -> u8 {
        match location.wram_bank {
            Some(bank) if self.bus.cgb && (0xD000..=0xDFFF).contains(&location.address) => {
                self.bus.wram_in_bank(bank, location.address)
            }
            _ => self.peek(location.address),
        }
    }

    /// Writes `location` (a banked one into its work RAM bank, on the GBC).
    pub fn write_location(&mut self, location: Location, value: u8) {
        match location.wram_bank {
            Some(bank) if self.bus.cgb && (0xD000..=0xDFFF).contains(&location.address) => {
                self.bus.write_wram_in_bank(bank, location.address, value);
            }
            _ => self.bus.write(location.address, value),
        }
    }

    /// The active cheat codes.
    pub fn cheats(&self) -> &Cheats {
        &self.bus.cheats
    }

    pub fn cheats_mut(&mut self) -> &mut Cheats {
        &mut self.bus.cheats
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

    /// Nudges the audio output rate (e.g. ±0.5 %) without a click, so the frontend can keep
    /// audio buffering steady while video runs at the Game Boy's own frame rate.
    pub fn adjust_audio_sample_rate(&mut self, rate: u32) {
        self.bus.apu.adjust_sample_rate(rate);
    }

    /// Moves the audio produced so far into `out` as interleaved stereo `f32` samples.
    pub fn drain_audio(&mut self, out: &mut Vec<f32>) {
        self.bus.apu.drain_samples(out);
    }

    /// The save file for a battery-backed cartridge, or `None` without a battery: its
    /// RAM, then the real-time clock (if any) stamped with `now`, the current Unix time in
    /// seconds.
    pub fn save_data(&self, now: u64) -> Option<Vec<u8>> {
        self.bus
            .cart
            .has_battery()
            .then(|| self.bus.cart.save_data(now))
    }

    /// Restores cartridge RAM and clock from a save file. The clock catches up on the real
    /// time since the save was written, up to `now` (Unix seconds); `None` carries on from
    /// where it was saved.
    pub fn load_save_data(&mut self, data: &[u8], now: Option<u64>) {
        self.bus.cart.load_ram(data, now);
    }

    /// Whether the cartridge has a real-time clock. Its save file then changes with time
    /// alone, so it is worth writing when the game closes even if RAM did not change.
    pub fn has_clock(&self) -> bool {
        self.bus.cart.has_clock()
    }

    /// Whether the cartridge has a tilt sensor (MBC7: Kirby Tilt 'n' Tumble, Command
    /// Master); see [`GameBoy::set_tilt`].
    pub fn has_tilt_sensor(&self) -> bool {
        self.bus.cart.has_tilt_sensor()
    }

    /// How far the Game Boy is tilted, in g, by which way things on it would roll: `x` to
    /// the right, `y` towards the bottom of the screen (about ±1 at most). Only MBC7
    /// cartridges notice.
    pub fn set_tilt(&mut self, x: f32, y: f32) {
        self.bus.cart.set_tilt(x, y);
    }

    /// Rumble strength since the last call, 0 to 1; `None` without a rumble cartridge or
    /// if no time has passed.
    pub fn take_rumble(&mut self) -> Option<f32> {
        self.bus.cart.take_rumble()
    }

    /// Returns `true` if cartridge RAM changed since the last call (i.e. the save is stale).
    pub fn take_save_dirty(&mut self) -> bool {
        self.bus.cart.take_ram_dirty()
    }

    pub fn cartridge(&self) -> &Cartridge {
        &self.bus.cart
    }

    /// Title plus header and global checksums: identifies the game a state belongs to.
    fn cartridge_identity(&self) -> (String, [u8; 3]) {
        let cart = &self.bus.cart;
        let checksums = [
            cart.read_rom(0x14D),
            cart.read_rom(0x14E),
            cart.read_rom(0x14F),
        ];
        (cart.title(), checksums)
    }

    /// Captures the complete machine state (everything except the ROM).
    pub fn save_state(&self) -> Vec<u8> {
        let (title, checksums) = self.cartridge_identity();
        let header = state::Header::new(title, checksums);
        let mut out = postcard::to_stdvec(&header).expect("header serializes");
        out.extend(postcard::to_stdvec(&(&self.cpu, &self.bus)).expect("state serializes"));
        out
    }

    /// Restores a state from [`GameBoy::save_state`]. The state must come from the same
    /// game; on error the current state is left untouched.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), StateError> {
        let (header, rest): (state::Header, _) = postcard::take_from_bytes(data)?;
        let (title, checksums) = self.cartridge_identity();
        header.check(&title, checksums)?;
        let (cpu, mut bus): (Cpu, Bus) = postcard::from_bytes(rest)?;
        bus.adopt_runtime_from(&mut self.bus);
        self.cpu = cpu;
        self.bus = bus;
        Ok(())
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
    fn cgb_boot_leaves_title_checksum_in_b_for_nintendo_dmg_games() {
        let regs = |rom: Vec<u8>| GameBoy::with_model(rom, Model::Cgb).unwrap().cpu.regs;
        let r = regs(test_rom());
        assert_eq!((r.bc(), r.de(), r.hl()), (0x0000, 0x0008, 0x007C));

        let mut rom = test_rom();
        rom[0x14B] = 0x01; // Nintendo
        let sum = b"FREKI".iter().fold(0u8, |a, &b| a.wrapping_add(b));
        assert_eq!(regs(rom).bc(), (sum as u16) << 8);
    }

    #[test]
    fn runs_a_frame_without_errors() {
        let mut gb = GameBoy::new(test_rom()).unwrap();
        gb.run_frame().unwrap();
        assert_eq!(gb.cartridge().title(), "FREKI");
    }

    #[test]
    fn boot_rom_runs_first_and_unmaps_itself() {
        // A stand-in boot ROM: NOPs, then `LD A,1; LDH ($50),A` at $FC, ending at $0100.
        let mut image = [0u8; 0x100];
        image[0xFC..].copy_from_slice(&[0x3E, 0x01, 0xE0, 0x50]);
        let boot = BootRom::new(&image).unwrap();
        let mut gb = GameBoy::with_boot_rom(test_rom(), boot).unwrap();
        assert_eq!((gb.pc(), gb.in_boot_rom()), (0x0000, true));
        assert_eq!(gb.peek(0xFD), 0x01, "boot ROM mapped over $0000-$00FF");
        while gb.pc() != 0x100 {
            gb.step().unwrap();
        }
        assert!(!gb.in_boot_rom());
        assert_eq!(
            gb.peek(0xFD),
            0x00,
            "cartridge visible after $FF50 was written"
        );
        assert_eq!(gb.peek(0x101), 0xC3);
    }

    #[test]
    fn boot_rom_must_be_256_or_2304_bytes() {
        assert_eq!(BootRom::new(&[0; 300]), Err(BootRomError::WrongSize(300)));
        assert!(BootRom::new(&[0; 2304]).unwrap().is_cgb());
    }

    #[test]
    fn cgb_games_get_banked_memory_and_a_11() {
        let mut rom = test_rom();
        rom[0x143] = 0x80;
        let mut gb = GameBoy::new(rom).unwrap();
        assert_eq!(gb.model(), Model::Cgb);
        assert_eq!(
            gb.registers().a,
            0x11,
            "A = $11 tells the game it runs on a CGB"
        );
        assert_eq!(gb.cartridge().title(), "FREKI");
        for (bank, value) in [(1, 0x11), (2, 0x22), (7, 0x77)] {
            gb.poke(0xFF70, bank);
            gb.poke(0xD000, value);
        }
        gb.poke(0xFF70, 0); // bank 0 selects bank 1
        assert_eq!(gb.peek(0xD000), 0x11);
        gb.poke(0xFF70, 2);
        assert_eq!(
            (gb.peek(0xD000), gb.peek(0xF000)),
            (0x22, 0x22),
            "echo follows SVBK"
        );
        gb.poke(0xFF4F, 1);
        gb.poke(0x8000, 0xB1);
        gb.poke(0xFF4F, 0);
        gb.poke(0x8000, 0xB0);
        gb.poke(0xFF4F, 1);
        assert_eq!(gb.peek(0x8000), 0xB1);
    }

    #[test]
    fn dmg_games_ignore_cgb_registers() {
        let mut gb = GameBoy::new(test_rom()).unwrap();
        assert_eq!(gb.model(), Model::Dmg);
        gb.poke(0xD000, 0x12);
        gb.poke(0xFF70, 3);
        assert_eq!((gb.peek(0xFF70), gb.peek(0xD000)), (0xFF, 0x12));
    }

    #[test]
    fn stop_switches_to_double_speed_when_armed() {
        let mut rom = test_rom();
        rom[0x143] = 0x80;
        rom[0x150..0x154].copy_from_slice(&[0x10, 0x00, 0x18, 0xFE]); // STOP; JR -2
        let mut gb = GameBoy::new(rom).unwrap();
        gb.poke(0xFF4D, 1);
        while gb.pc() != 0x152 {
            gb.step().unwrap();
        }
        assert_eq!(
            gb.peek(0xFF4D) & 0x81,
            0x80,
            "double speed, switch disarmed"
        );
        // A frame now takes twice as many CPU cycles.
        let mut cycles = 0;
        gb.bus.ppu.take_frame_ready();
        while !gb.bus.ppu.take_frame_ready() {
            cycles += gb.step().unwrap();
        }
        assert!(cycles > CYCLES_PER_FRAME, "{cycles}");
        cycles = 0;
        while !gb.bus.ppu.take_frame_ready() {
            cycles += gb.step().unwrap();
        }
        assert!((2 * CYCLES_PER_FRAME - 16..=2 * CYCLES_PER_FRAME + 16).contains(&cycles));
    }

    fn cgb_with_source_data() -> GameBoy {
        let mut rom = test_rom();
        rom[0x143] = 0x80;
        let mut gb = GameBoy::new(rom).unwrap();
        for i in 0..0x40u16 {
            gb.poke(0xC100 + i, i as u8 + 1);
        }
        gb.poke(0xFF51, 0xC1); // source $C100
        gb.poke(0xFF52, 0x00);
        gb.poke(0xFF53, 0x08); // destination $8800
        gb.poke(0xFF54, 0x00);
        gb
    }

    #[test]
    fn general_purpose_hdma_copies_everything_and_pauses_the_cpu() {
        let mut gb = cgb_with_source_data();
        gb.poke(0xFF55, 0x01); // 2 blocks, general purpose
        assert_eq!(gb.peek(0x8800), 1);
        assert_eq!(gb.peek(0x881F), 0x20);
        assert_eq!(gb.peek(0x8820), 0, "only 2 blocks");
        assert_eq!(gb.peek(0xFF55), 0xFF, "finished");
        assert_eq!(
            gb.step().unwrap(),
            72,
            "CPU paused 32 T-cycles per block plus 8"
        );
    }

    #[test]
    fn hblank_hdma_copies_one_block_per_line() {
        let mut gb = cgb_with_source_data();
        // Start right after an HBlank began, so the next block comes on the next line.
        while gb.peek(0xFF41) & 3 != 0 {
            gb.step().unwrap();
        }
        gb.poke(0xFF55, 0x82); // 3 blocks, HBlank
        assert_eq!(gb.peek(0xFF55), 0x02, "active, 3 blocks left");
        // Next line: through mode 3 into its HBlank.
        for mode in [3, 0] {
            while gb.peek(0xFF41) & 3 != mode {
                gb.step().unwrap();
            }
        }
        assert_eq!(gb.peek(0x8800), 1);
        assert_eq!(gb.peek(0x8810), 0, "one block per HBlank");
        assert_eq!(gb.peek(0xFF55), 0x01);
        gb.poke(0xFF55, 0x00); // stop
        assert_eq!(gb.peek(0xFF55), 0x81, "stopped, 2 blocks were left");
    }

    #[test]
    fn dmg_games_on_a_cgb_run_in_compatibility_mode() {
        let mut gb = GameBoy::with_model(test_rom(), Model::Cgb).unwrap();
        assert_eq!(gb.model(), Model::Cgb);
        assert!(gb.dmg_compat());
        assert_eq!(gb.registers().a, 0x11);
        gb.poke(0xFF70, 3); // CGB registers are locked in compatibility mode
        assert_eq!(gb.peek(0xFF70), 0xFF);
        gb.run_frame().unwrap();
        let Screen::Rgb555(pixels) = gb.screen() else {
            panic!("compatibility mode shows colours");
        };
        assert_eq!(
            pixels[0], 0x7FFF,
            "shade 0 through the grey default palette"
        );
    }

    #[test]
    fn cgb_memory_search_finds_values_in_other_wram_banks() {
        let mut rom = test_rom();
        rom[0x143] = 0x80;
        let mut gb = GameBoy::new(rom).unwrap();
        gb.poke(0xFF70, 3);
        gb.poke(0xD100, 42); // e.g. money, kept in bank 3
        gb.poke(0xFF70, 1); // the game has bank 1 selected
        let mut search = gb.start_memory_search();
        search.filter(Filter::Equal(42), |l| gb.peek_location(l));
        let banked = Location {
            address: 0xD100,
            wram_bank: Some(3),
        };
        assert_eq!(search.candidates(), &[(banked, 42)]);
        let code = Code::gameshark(banked, 99);
        assert_eq!(code, "936300D1");
        gb.cheats_mut().add(&code, "money").unwrap();
        gb.run_frame().unwrap();
        assert_eq!(gb.peek_location(banked), 99, "written to bank 3");
        assert_eq!(gb.peek(0xD100), 0, "bank 1 untouched");
    }
}
