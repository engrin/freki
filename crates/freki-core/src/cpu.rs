//! The Sharp SM83 CPU.
//!
//! Opcodes are decoded from their bit fields rather than a 512-entry table:
//!
//! ```text
//!   7 6 | 5 4 3 | 2 1 0
//!    x  |   y   |   z        y = p:q  (p = bits 5–4, q = bit 3)
//! ```
//!
//! See <https://gbdev.io/gb-opcodes/optables/> and
//! <https://archive.gbdev.io/salvage/decoding_gbz80_opcodes/Decoding%20Gamboy%20Z80%20Opcodes.html>.
//!
//! Timing is per M-cycle, with "pending cycles" bookkeeping: before each memory access the
//! rest of the hardware is caught up on the preceding M-cycles, then the access happens, and
//! the instruction's last M-cycle is caught up when it finishes. An instruction's k-th access
//! therefore lands k × 4 T-cycles after it starts.
//!
//! Writes to some DMG I/O registers land a T-cycle or two off that grid, or briefly show an
//! intermediate value; see [`Cpu::mem_write`].

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::bus::Bus;
use crate::debugger::Access;

pub mod flag {
    pub const Z: u8 = 1 << 7;
    pub const N: u8 = 1 << 6;
    pub const H: u8 = 1 << 5;
    pub const C: u8 = 1 << 4;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpuError {
    /// One of the 11 opcodes that lock up a real Game Boy ($D3, $DB, $DD, $E3, $E4,
    /// $EB, $EC, $ED, $F4, $FC, $FD).
    IllegalOpcode { opcode: u8, pc: u16 },
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IllegalOpcode { opcode, pc } => {
                write!(f, "illegal opcode ${opcode:02X} at ${pc:04X}")
            }
        }
    }
}

impl std::error::Error for CpuError {}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct Registers {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
}

impl Registers {
    pub fn af(&self) -> u16 {
        u16::from_be_bytes([self.a, self.f])
    }
    pub fn bc(&self) -> u16 {
        u16::from_be_bytes([self.b, self.c])
    }
    pub fn de(&self) -> u16 {
        u16::from_be_bytes([self.d, self.e])
    }
    pub fn hl(&self) -> u16 {
        u16::from_be_bytes([self.h, self.l])
    }
    pub fn set_af(&mut self, v: u16) {
        [self.a, self.f] = v.to_be_bytes();
        self.f &= 0xF0; // low nibble of F is always zero
    }
    pub fn set_bc(&mut self, v: u16) {
        [self.b, self.c] = v.to_be_bytes();
    }
    pub fn set_de(&mut self, v: u16) {
        [self.d, self.e] = v.to_be_bytes();
    }
    pub fn set_hl(&mut self, v: u16) {
        [self.h, self.l] = v.to_be_bytes();
    }
    pub fn flag(&self, mask: u8) -> bool {
        self.f & mask != 0
    }
    pub fn set_flag(&mut self, mask: u8, on: bool) {
        if on {
            self.f |= mask;
        } else {
            self.f &= !mask;
        }
    }
    /// Sets all four flags at once.
    fn set_flags(&mut self, z: bool, n: bool, h: bool, c: bool) {
        self.f = (u8::from(z) << 7) | (u8::from(n) << 6) | (u8::from(h) << 5) | (u8::from(c) << 4);
    }
}

#[derive(Serialize, Deserialize)]
pub struct Cpu {
    pub regs: Registers,
    /// Interrupt master enable.
    pub ime: bool,
    /// EI enables interrupts after the *following* instruction.
    ime_pending: bool,
    pub halted: bool,
    /// HALT with IME=0 and an interrupt already pending: the next opcode byte is read twice.
    halt_bug: bool,
    /// T-cycles spent so far in the current `step`.
    cycles: u32,
    /// T-cycles of the current instruction not yet run on the rest of the hardware.
    pending: u32,
}

impl Cpu {
    /// Register state of a DMG right after the boot ROM hands over to the cartridge.
    pub fn new_post_boot() -> Self {
        let mut regs = Registers::default();
        regs.set_af(0x01B0);
        regs.set_bc(0x0013);
        regs.set_de(0x00D8);
        regs.set_hl(0x014D);
        regs.sp = 0xFFFE;
        regs.pc = 0x0100;
        Self {
            regs,
            ime: false,
            ime_pending: false,
            halted: false,
            halt_bug: false,
            cycles: 0,
            pending: 0,
        }
    }

    // ---- bus timing -------------------------------------------------------------------

    /// Runs the rest of the hardware for `cycles` T-cycles.
    fn advance(&mut self, bus: &mut Bus, cycles: u32) {
        if cycles > 0 {
            bus.tick(cycles);
            self.cycles += cycles;
        }
    }

    /// Catches the hardware up on everything still pending.
    fn flush(&mut self, bus: &mut Bus) {
        let pending = std::mem::take(&mut self.pending);
        self.advance(bus, pending);
    }

    /// One M-cycle without a memory access.
    fn idle(&mut self, _bus: &mut Bus) {
        self.pending += 4;
    }

    /// One M-cycle in which the CPU drives the bus without a data access.
    fn bus_cycle(&mut self, bus: &mut Bus) {
        self.flush(bus);
        self.pending = 4;
    }

    /// Memory read taking one M-cycle; reported to read watchpoints.
    fn mem_read(&mut self, bus: &mut Bus, addr: u16) -> u8 {
        let value = self.fetch_read(bus, addr);
        if !bus.breaks.watches.is_empty() {
            bus.breaks.access(addr, value, Access::Read);
        }
        value
    }

    /// Instruction fetch taking one M-cycle; not reported to read watchpoints
    /// (executing code is what PC breakpoints are for).
    fn fetch_read(&mut self, bus: &mut Bus, addr: u16) -> u8 {
        self.flush(bus);
        let value = bus.read(addr);
        self.pending = 4;
        value
    }

    /// Memory write taking one M-cycle.
    ///
    /// On the DMG some I/O registers are latched slightly off the M-cycle grid or show an
    /// intermediate value for one T-cycle (timings from SameBoy's hardware research):
    /// - IF: written 1 T-cycle late.
    /// - SCY and SCX: written 2 T-cycles early. (SameBoy uses 1 for SCY; 2 matches the DMG
    ///   reference screenshots of mealybug-tearoom's `m3_scy_change` in this PPU model.)
    /// - BGP/OBP0/OBP1: 2 T-cycles early, holding `old | new` for the first T-cycle.
    /// - LCDC: 2 T-cycles early, with an intermediate value for one T-cycle.
    /// - STAT: reads as $FF (all interrupt sources enabled) for one T-cycle — the DMG
    ///   STAT write bug.
    /// - WX: marks the value as just changed for one T-cycle (window start quirk).
    fn mem_write(&mut self, bus: &mut Bus, addr: u16, value: u8) {
        if !bus.breaks.watches.is_empty() {
            bus.breaks.access(addr, value, Access::Write);
        }
        let pending = std::mem::take(&mut self.pending);
        match addr {
            0xFF0F => {
                self.advance(bus, pending + 1);
                bus.write(addr, value);
                self.pending = 3;
            }
            0xFF40 => {
                let old = bus.read(addr);
                self.advance(bus, pending.saturating_sub(2));
                let intermediate = bus.ppu.dmg_lcdc_write_conflict(old, value);
                bus.write(addr, intermediate);
                self.advance(bus, 1);
                bus.write(addr, value);
                bus.ppu.lcdc_written(old, value);
                self.pending = 5;
            }
            0xFF41 => {
                self.advance(bus, pending);
                bus.write(addr, 0xFF);
                self.advance(bus, 1);
                bus.write(addr, value);
                self.pending = 3;
            }
            0xFF42 | 0xFF43 => {
                self.advance(bus, pending.saturating_sub(2));
                bus.write(addr, value);
                self.pending = 6;
            }
            0xFF47..=0xFF49 => {
                self.advance(bus, pending.saturating_sub(2));
                let old = bus.read(addr);
                bus.write(addr, old | value);
                self.advance(bus, 1);
                bus.write(addr, value);
                self.pending = 5;
            }
            0xFF4B => {
                self.advance(bus, pending);
                bus.write(addr, value);
                bus.ppu.set_wx_just_changed(true);
                self.advance(bus, 1);
                bus.ppu.set_wx_just_changed(false);
                self.pending = 3;
            }
            _ => {
                self.advance(bus, pending);
                bus.write(addr, value);
                self.pending = 4;
            }
        }
    }

    // ---- fetch and stack helpers -------------------------------------------------------

    fn fetch(&mut self, bus: &mut Bus) -> u8 {
        let byte = self.fetch_read(bus, self.regs.pc);
        if self.halt_bug {
            self.halt_bug = false;
        } else {
            self.regs.pc = self.regs.pc.wrapping_add(1);
        }
        byte
    }

    fn fetch_u16(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.fetch(bus);
        let hi = self.fetch(bus);
        u16::from_le_bytes([lo, hi])
    }

    /// One internal M-cycle followed by two writes, as in PUSH, CALL, RST and interrupts.
    fn push(&mut self, bus: &mut Bus, value: u16) {
        self.idle(bus);
        let [hi, lo] = value.to_be_bytes();
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        self.mem_write(bus, self.regs.sp, hi);
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        self.mem_write(bus, self.regs.sp, lo);
    }

    fn pop(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.mem_read(bus, self.regs.sp);
        self.regs.sp = self.regs.sp.wrapping_add(1);
        let hi = self.mem_read(bus, self.regs.sp);
        self.regs.sp = self.regs.sp.wrapping_add(1);
        u16::from_le_bytes([lo, hi])
    }

    // ---- operand decoding --------------------------------------------------------------

    /// 8-bit operand `r[i]`: B C D E H L (HL) A.
    fn r8(&mut self, bus: &mut Bus, i: u8) -> u8 {
        let r = &self.regs;
        match i {
            0 => r.b,
            1 => r.c,
            2 => r.d,
            3 => r.e,
            4 => r.h,
            5 => r.l,
            6 => {
                let hl = r.hl();
                self.mem_read(bus, hl)
            }
            _ => r.a,
        }
    }

    fn set_r8(&mut self, bus: &mut Bus, i: u8, v: u8) {
        let r = &mut self.regs;
        match i {
            0 => r.b = v,
            1 => r.c = v,
            2 => r.d = v,
            3 => r.e = v,
            4 => r.h = v,
            5 => r.l = v,
            6 => {
                let hl = r.hl();
                self.mem_write(bus, hl, v);
            }
            _ => r.a = v,
        }
    }

    /// 16-bit operand `rp[i]`: BC DE HL SP.
    fn rp(&self, i: u8) -> u16 {
        match i {
            0 => self.regs.bc(),
            1 => self.regs.de(),
            2 => self.regs.hl(),
            _ => self.regs.sp,
        }
    }

    fn set_rp(&mut self, i: u8, v: u16) {
        match i {
            0 => self.regs.set_bc(v),
            1 => self.regs.set_de(v),
            2 => self.regs.set_hl(v),
            _ => self.regs.sp = v,
        }
    }

    /// 16-bit operand `rp2[i]` (PUSH/POP): BC DE HL AF.
    fn rp2(&self, i: u8) -> u16 {
        if i == 3 { self.regs.af() } else { self.rp(i) }
    }

    fn set_rp2(&mut self, i: u8, v: u16) {
        if i == 3 {
            self.regs.set_af(v);
        } else {
            self.set_rp(i, v);
        }
    }

    /// Condition `cc[i]`: NZ Z NC C.
    fn cond(&self, i: u8) -> bool {
        match i & 3 {
            0 => !self.regs.flag(flag::Z),
            1 => self.regs.flag(flag::Z),
            2 => !self.regs.flag(flag::C),
            _ => self.regs.flag(flag::C),
        }
    }

    // ---- ALU ---------------------------------------------------------------------------

    /// `alu[op] A, v`: ADD ADC SUB SBC AND XOR OR CP.
    fn alu(&mut self, op: u8, v: u8) {
        let a = self.regs.a;
        let carry = u8::from(self.regs.flag(flag::C));
        match op {
            0 | 1 => {
                let c = if op == 1 { carry } else { 0 };
                let sum = a as u16 + v as u16 + c as u16;
                let r = sum as u8;
                let h = (a & 0xF) + (v & 0xF) + c > 0xF;
                self.regs.set_flags(r == 0, false, h, sum > 0xFF);
                self.regs.a = r;
            }
            2 | 3 | 7 => {
                let c = if op == 3 { carry } else { 0 };
                let r = a.wrapping_sub(v).wrapping_sub(c);
                let h = (a & 0xF) < (v & 0xF) + c;
                let cy = (a as u16) < v as u16 + c as u16;
                self.regs.set_flags(r == 0, true, h, cy);
                if op != 7 {
                    self.regs.a = r;
                }
            }
            4 => {
                self.regs.a = a & v;
                self.regs.set_flags(self.regs.a == 0, false, true, false);
            }
            5 => {
                self.regs.a = a ^ v;
                self.regs.set_flags(self.regs.a == 0, false, false, false);
            }
            _ => {
                self.regs.a = a | v;
                self.regs.set_flags(self.regs.a == 0, false, false, false);
            }
        }
    }

    /// CB-prefix `rot[op] v`: RLC RRC RL RR SLA SRA SWAP SRL. Sets Z from the result.
    fn rot(&mut self, op: u8, v: u8) -> u8 {
        let carry_in = u8::from(self.regs.flag(flag::C));
        let (r, c) = match op {
            0 => (v.rotate_left(1), v & 0x80 != 0),
            1 => (v.rotate_right(1), v & 0x01 != 0),
            2 => ((v << 1) | carry_in, v & 0x80 != 0),
            3 => ((v >> 1) | (carry_in << 7), v & 0x01 != 0),
            4 => (v << 1, v & 0x80 != 0),
            5 => ((v >> 1) | (v & 0x80), v & 0x01 != 0),
            6 => (v.rotate_left(4), false),
            _ => (v >> 1, v & 0x01 != 0),
        };
        self.regs.set_flags(r == 0, false, false, c);
        r
    }

    /// SP + signed 8-bit offset, setting flags as ADD SP,e / LD HL,SP+e do.
    fn sp_plus_e8(&mut self, bus: &mut Bus) -> u16 {
        let e = self.fetch(bus);
        let sp = self.regs.sp;
        let h = (sp & 0xF) + (e as u16 & 0xF) > 0xF;
        let c = (sp & 0xFF) + e as u16 > 0xFF;
        self.regs.set_flags(false, false, h, c);
        sp.wrapping_add_signed(e as i8 as i16)
    }

    fn daa(&mut self) {
        let mut a = self.regs.a;
        let n = self.regs.flag(flag::N);
        let mut c = self.regs.flag(flag::C);
        let h = self.regs.flag(flag::H);
        if !n {
            if c || a > 0x99 {
                a = a.wrapping_add(0x60);
                c = true;
            }
            if h || (a & 0x0F) > 0x09 {
                a = a.wrapping_add(0x06);
            }
        } else {
            if c {
                a = a.wrapping_sub(0x60);
            }
            if h {
                a = a.wrapping_sub(0x06);
            }
        }
        self.regs.a = a;
        self.regs.set_flags(a == 0, n, false, c);
    }

    // ---- execution ---------------------------------------------------------------------

    /// Dispatches a pending interrupt, if any. Returns `true` if one was dispatched.
    ///
    /// Dispatch takes 5 M-cycles: a discarded opcode read, an internal cycle, an idle cycle,
    /// then PC is pushed. Which interrupt wins is decided only after the high byte of PC is
    /// pushed, so a push that overwrites IE ($FFFF) can cancel it (PC then becomes $0000).
    fn handle_interrupts(&mut self, bus: &mut Bus) -> bool {
        let requested = bus.interrupt_enable & bus.interrupt_flag & 0x1F;
        if requested == 0 {
            return false;
        }
        self.halted = false;
        if !self.ime {
            return false;
        }
        self.ime = false;
        self.fetch_read(bus, self.regs.pc);
        self.bus_cycle(bus);
        self.idle(bus);
        let [hi, lo] = self.regs.pc.to_be_bytes();
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        self.mem_write(bus, self.regs.sp, hi);
        let enabled = bus.interrupt_enable;
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        self.mem_write(bus, self.regs.sp, lo);
        let queue = enabled & bus.interrupt_flag & 0x1F;
        if queue == 0 {
            self.regs.pc = 0x0000;
            return true;
        }
        let bit = queue.trailing_zeros() as u16;
        let pending = std::mem::take(&mut self.pending);
        self.advance(bus, pending - 2);
        self.pending = 2;
        bus.interrupt_flag &= !(1 << bit);
        self.regs.pc = 0x0040 + bit * 8;
        true
    }

    /// Executes one instruction (or services an interrupt) and returns T-cycles taken.
    pub fn step(&mut self, bus: &mut Bus) -> Result<u32, CpuError> {
        self.cycles = 0;
        debug_assert_eq!(self.pending, 0);
        if self.handle_interrupts(bus) {
            self.flush(bus);
            debug_assert_eq!(self.cycles, 20, "interrupt dispatch timing");
            return Ok(self.cycles);
        }
        if self.halted {
            self.idle(bus);
            self.flush(bus);
            return Ok(self.cycles);
        }
        if self.ime_pending {
            self.ime_pending = false;
            self.ime = true;
        }

        let pc = self.regs.pc;
        let op = self.fetch(bus);
        let expected = self.execute(bus, op);
        self.flush(bus);
        let expected = expected.ok_or(CpuError::IllegalOpcode { opcode: op, pc })?;
        debug_assert_eq!(self.cycles, expected, "timing of opcode ${op:02X}");
        Ok(self.cycles)
    }

    /// Executes `op`. Returns T-cycles, or `None` for an illegal opcode.
    fn execute(&mut self, bus: &mut Bus, op: u8) -> Option<u32> {
        let x = op >> 6;
        let y = (op >> 3) & 7;
        let z = op & 7;
        let p = y >> 1;
        let q = y & 1;
        // Extra cost of an (HL) memory operand in r8 slot `i`.
        let hl_cost = |i: u8, extra: u32| if i == 6 { extra } else { 0 };

        let cycles = match (x, z) {
            // ---- x = 0 ----
            (0, 0) => match y {
                0 => 4, // NOP
                1 => {
                    // LD (a16), SP
                    let addr = self.fetch_u16(bus);
                    let [hi, lo] = self.regs.sp.to_be_bytes();
                    self.mem_write(bus, addr, lo);
                    self.mem_write(bus, addr.wrapping_add(1), hi);
                    20
                }
                2 => {
                    // STOP (treated as a 2-byte NOP until CGB speed switching exists)
                    self.fetch(bus);
                    8
                }
                _ => {
                    // JR e8 / JR cc, e8
                    let e = self.fetch(bus) as i8;
                    if y == 3 || self.cond(y - 4) {
                        self.idle(bus);
                        self.regs.pc = self.regs.pc.wrapping_add_signed(e.into());
                        12
                    } else {
                        8
                    }
                }
            },
            (0, 1) => {
                if q == 0 {
                    // LD rp, d16
                    let v = self.fetch_u16(bus);
                    self.set_rp(p, v);
                    12
                } else {
                    // ADD HL, rp
                    self.idle(bus);
                    let hl = self.regs.hl();
                    let v = self.rp(p);
                    let (r, c) = hl.overflowing_add(v);
                    let h = (hl & 0x0FFF) + (v & 0x0FFF) > 0x0FFF;
                    let z = self.regs.flag(flag::Z);
                    self.regs.set_flags(z, false, h, c);
                    self.regs.set_hl(r);
                    8
                }
            }
            (0, 2) => {
                // LD (BC/DE/HL+/HL-), A and LD A, (BC/DE/HL+/HL-)
                let addr = match p {
                    0 => self.regs.bc(),
                    1 => self.regs.de(),
                    _ => self.regs.hl(),
                };
                match p {
                    2 => self.regs.set_hl(addr.wrapping_add(1)),
                    3 => self.regs.set_hl(addr.wrapping_sub(1)),
                    _ => {}
                }
                if q == 0 {
                    self.mem_write(bus, addr, self.regs.a);
                } else {
                    self.regs.a = self.mem_read(bus, addr);
                }
                8
            }
            (0, 3) => {
                // INC rp / DEC rp
                self.idle(bus);
                let v = self.rp(p);
                let r = if q == 0 {
                    v.wrapping_add(1)
                } else {
                    v.wrapping_sub(1)
                };
                self.set_rp(p, r);
                8
            }
            (0, 4) | (0, 5) => {
                // INC r / DEC r (carry unaffected)
                let v = self.r8(bus, y);
                let c = self.regs.flag(flag::C);
                let r = if z == 4 {
                    let r = v.wrapping_add(1);
                    self.regs.set_flags(r == 0, false, v & 0xF == 0xF, c);
                    r
                } else {
                    let r = v.wrapping_sub(1);
                    self.regs.set_flags(r == 0, true, v & 0xF == 0, c);
                    r
                };
                self.set_r8(bus, y, r);
                4 + hl_cost(y, 8)
            }
            (0, 6) => {
                // LD r, d8
                let v = self.fetch(bus);
                self.set_r8(bus, y, v);
                8 + hl_cost(y, 4)
            }
            (0, 7) => {
                match y {
                    // RLCA RRCA RLA RRA: like the CB versions, but Z is always cleared
                    0..=3 => {
                        self.regs.a = self.rot(y, self.regs.a);
                        self.regs.set_flag(flag::Z, false);
                    }
                    4 => self.daa(),
                    5 => {
                        // CPL
                        self.regs.a = !self.regs.a;
                        self.regs.set_flag(flag::N, true);
                        self.regs.set_flag(flag::H, true);
                    }
                    6 | 7 => {
                        // SCF / CCF
                        let c = y == 6 || !self.regs.flag(flag::C);
                        let z = self.regs.flag(flag::Z);
                        self.regs.set_flags(z, false, false, c);
                    }
                    _ => unreachable!(),
                }
                4
            }

            // ---- x = 1: LD r, r' and HALT ----
            (1, _) => {
                if y == 6 && z == 6 {
                    let pending = bus.interrupt_enable & bus.interrupt_flag & 0x1F != 0;
                    if !self.ime && pending {
                        self.halt_bug = true;
                    } else {
                        self.halted = true;
                    }
                    4
                } else {
                    let v = self.r8(bus, z);
                    self.set_r8(bus, y, v);
                    4 + hl_cost(y, 4) + hl_cost(z, 4)
                }
            }

            // ---- x = 2: ALU A, r ----
            (2, _) => {
                let v = self.r8(bus, z);
                self.alu(y, v);
                4 + hl_cost(z, 4)
            }

            // ---- x = 3 ----
            (3, 0) => match y {
                0..=3 => {
                    // RET cc
                    self.idle(bus);
                    if self.cond(y) {
                        self.regs.pc = self.pop(bus);
                        self.idle(bus);
                        20
                    } else {
                        8
                    }
                }
                4 => {
                    // LDH (a8), A
                    let addr = 0xFF00 | self.fetch(bus) as u16;
                    self.mem_write(bus, addr, self.regs.a);
                    12
                }
                5 => {
                    // ADD SP, e8
                    self.regs.sp = self.sp_plus_e8(bus);
                    self.idle(bus);
                    self.idle(bus);
                    16
                }
                6 => {
                    // LDH A, (a8)
                    let addr = 0xFF00 | self.fetch(bus) as u16;
                    self.regs.a = self.mem_read(bus, addr);
                    12
                }
                _ => {
                    // LD HL, SP+e8
                    let v = self.sp_plus_e8(bus);
                    self.idle(bus);
                    self.regs.set_hl(v);
                    12
                }
            },
            (3, 1) => {
                if q == 0 {
                    // POP rp2
                    let v = self.pop(bus);
                    self.set_rp2(p, v);
                    12
                } else {
                    match p {
                        0 | 1 => {
                            // RET / RETI
                            self.regs.pc = self.pop(bus);
                            self.idle(bus);
                            if p == 1 {
                                self.ime = true;
                            }
                            16
                        }
                        2 => {
                            // JP HL
                            self.regs.pc = self.regs.hl();
                            4
                        }
                        _ => {
                            // LD SP, HL
                            self.idle(bus);
                            self.regs.sp = self.regs.hl();
                            8
                        }
                    }
                }
            }
            (3, 2) => match y {
                0..=3 => {
                    // JP cc, a16
                    let addr = self.fetch_u16(bus);
                    if self.cond(y) {
                        self.idle(bus);
                        self.regs.pc = addr;
                        16
                    } else {
                        12
                    }
                }
                4 => {
                    // LD (C), A
                    self.mem_write(bus, 0xFF00 | self.regs.c as u16, self.regs.a);
                    8
                }
                5 => {
                    // LD (a16), A
                    let addr = self.fetch_u16(bus);
                    self.mem_write(bus, addr, self.regs.a);
                    16
                }
                6 => {
                    // LD A, (C)
                    self.regs.a = self.mem_read(bus, 0xFF00 | self.regs.c as u16);
                    8
                }
                _ => {
                    // LD A, (a16)
                    let addr = self.fetch_u16(bus);
                    self.regs.a = self.mem_read(bus, addr);
                    16
                }
            },
            (3, 3) => match y {
                0 => {
                    // JP a16
                    self.regs.pc = self.fetch_u16(bus);
                    self.idle(bus);
                    16
                }
                1 => {
                    let cb = self.fetch(bus);
                    self.execute_cb(bus, cb)
                }
                6 => {
                    // DI
                    self.ime = false;
                    self.ime_pending = false;
                    4
                }
                7 => {
                    // EI
                    self.ime_pending = true;
                    4
                }
                _ => return None,
            },
            (3, 4) => {
                // CALL cc, a16
                if y > 3 {
                    return None;
                }
                let addr = self.fetch_u16(bus);
                if self.cond(y) {
                    self.push(bus, self.regs.pc);
                    self.regs.pc = addr;
                    24
                } else {
                    12
                }
            }
            (3, 5) => {
                if q == 0 {
                    // PUSH rp2
                    self.push(bus, self.rp2(p));
                    16
                } else if p == 0 {
                    // CALL a16
                    let addr = self.fetch_u16(bus);
                    self.push(bus, self.regs.pc);
                    self.regs.pc = addr;
                    24
                } else {
                    return None;
                }
            }
            (3, 6) => {
                // ALU A, d8
                let v = self.fetch(bus);
                self.alu(y, v);
                8
            }
            (3, 7) => {
                // RST y*8
                self.push(bus, self.regs.pc);
                self.regs.pc = (y as u16) * 8;
                16
            }
            _ => unreachable!("x and z are 2- and 3-bit fields"),
        };
        Some(cycles)
    }

    /// Executes a CB-prefixed opcode. Returns total T-cycles including the prefix.
    fn execute_cb(&mut self, bus: &mut Bus, op: u8) -> u32 {
        let x = op >> 6;
        let y = (op >> 3) & 7;
        let z = op & 7;
        let v = self.r8(bus, z);
        match x {
            0 => {
                let r = self.rot(y, v);
                self.set_r8(bus, z, r);
            }
            1 => {
                // BIT y, r (carry unaffected)
                let c = self.regs.flag(flag::C);
                self.regs.set_flags(v & (1 << y) == 0, false, true, c);
                return if z == 6 { 12 } else { 8 };
            }
            2 => self.set_r8(bus, z, v & !(1 << y)), // RES
            _ => self.set_r8(bus, z, v | (1 << y)),  // SET
        }
        if z == 6 { 16 } else { 8 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::Cartridge;

    /// Runs `code` placed at $C000 (WRAM) for `steps` instructions.
    fn run(code: &[u8], steps: usize, setup: impl FnOnce(&mut Cpu)) -> (Cpu, Bus) {
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x00;
        let mut bus = Bus::new(Cartridge::from_rom(rom).unwrap());
        for (i, &b) in code.iter().enumerate() {
            bus.write(0xC000 + i as u16, b);
        }
        let mut cpu = Cpu::new_post_boot();
        cpu.regs.pc = 0xC000;
        setup(&mut cpu);
        for _ in 0..steps {
            cpu.step(&mut bus).unwrap();
        }
        (cpu, bus)
    }

    #[test]
    fn af_masks_low_nibble() {
        let mut r = Registers::default();
        r.set_af(0x12FF);
        assert_eq!(r.af(), 0x12F0);
    }

    #[test]
    fn post_boot_state() {
        let cpu = Cpu::new_post_boot();
        assert_eq!(cpu.regs.af(), 0x01B0);
        assert_eq!(cpu.regs.pc, 0x0100);
        assert_eq!(cpu.regs.sp, 0xFFFE);
    }

    #[test]
    fn add_sets_half_carry_and_carry() {
        // LD A,$8F ; ADD A,$81
        let (cpu, _) = run(&[0x3E, 0x8F, 0xC6, 0x81], 2, |_| {});
        assert_eq!(cpu.regs.a, 0x10);
        assert_eq!(cpu.regs.f, flag::H | flag::C);
    }

    #[test]
    fn push_pop_af_masks_flags() {
        // LD BC,$12FF ; PUSH BC ; POP AF
        let (cpu, _) = run(&[0x01, 0xFF, 0x12, 0xC5, 0xF1], 3, |_| {});
        assert_eq!(cpu.regs.af(), 0x12F0);
    }

    #[test]
    fn call_and_ret() {
        // CALL $C005 ; NOP ; NOP ; RET at $C005
        let (cpu, _) = run(&[0xCD, 0x05, 0xC0, 0x00, 0x00, 0xC9], 2, |_| {});
        assert_eq!(cpu.regs.pc, 0xC003);
        assert_eq!(cpu.regs.sp, 0xFFFE);
    }

    #[test]
    fn cb_bit_res_set_on_hl() {
        // LD HL,$C100 ; SET 3,(HL) ; BIT 3,(HL) ; RES 3,(HL)
        let code = [0x21, 0x00, 0xC1, 0xCB, 0xDE, 0xCB, 0x5E, 0xCB, 0x9E];
        let (cpu, bus) = run(&code, 3, |_| {});
        assert!(!cpu.regs.flag(flag::Z));
        assert_eq!(bus.read(0xC100), 0x08);
        let (_, bus) = run(&code, 4, |_| {});
        assert_eq!(bus.read(0xC100), 0x00);
    }

    #[test]
    fn daa_after_bcd_add() {
        // LD A,$45 ; ADD A,$38 ; DAA  -> 83 (BCD)
        let (cpu, _) = run(&[0x3E, 0x45, 0xC6, 0x38, 0x27], 3, |_| {});
        assert_eq!(cpu.regs.a, 0x83);
    }

    #[test]
    fn illegal_opcode_is_reported() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100] = 0xD3;
        let mut bus = Bus::new(Cartridge::from_rom(rom).unwrap());
        let mut cpu = Cpu::new_post_boot();
        assert_eq!(
            cpu.step(&mut bus),
            Err(CpuError::IllegalOpcode {
                opcode: 0xD3,
                pc: 0x0100
            })
        );
    }
}
