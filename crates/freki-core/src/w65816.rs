//! WDC 65C816: the CPU of the SNES (in its 5A22), which the Super Game Boy's SNES side runs.
//!
//! Every bus cycle goes through [`Bus65816`]: reads, writes and internal cycles, so the
//! machine around it can time each one (SNES memory regions run at different speeds).
//! Behaviour follows the WDC W65C816S datasheet and is checked against the per-opcode
//! SingleStepTests suite (`tests/w65816.rs`).

use serde::{Deserialize, Serialize};

/// The CPU's view of the machine: 24-bit addresses.
pub trait Bus65816 {
    fn read(&mut self, addr: u32) -> u8;
    fn write(&mut self, addr: u32, value: u8);
    /// An internal cycle (no memory access).
    fn idle(&mut self);
}

pub mod flag {
    pub const C: u8 = 0x01;
    pub const Z: u8 = 0x02;
    pub const I: u8 = 0x04;
    pub const D: u8 = 0x08;
    /// Native mode: 8-bit index registers. Emulation mode: the B (break) bit as pushed.
    pub const X: u8 = 0x10;
    /// 8-bit accumulator and memory.
    pub const M: u8 = 0x20;
    pub const V: u8 = 0x40;
    pub const N: u8 = 0x80;
}
use flag::*;

/// Interrupt vectors in bank 0: (native, emulation).
const VEC_COP: (u16, u16) = (0xFFE4, 0xFFF4);
const VEC_BRK: (u16, u16) = (0xFFE6, 0xFFFE);
const VEC_NMI: (u16, u16) = (0xFFEA, 0xFFFA);
const VEC_IRQ: (u16, u16) = (0xFFEE, 0xFFFE);
const VEC_RESET: u16 = 0xFFFC;

/// An effective address: in bank 0 with 16-bit wrap-around (direct page, stack), or a full
/// 24-bit address whose next byte may be in the next bank.
#[derive(Clone, Copy)]
enum Ea {
    Bank0(u16),
    Long(u32),
}

impl Ea {
    fn addr(self) -> u32 {
        match self {
            Ea::Bank0(a) => a as u32,
            Ea::Long(a) => a & 0xFF_FFFF,
        }
    }

    fn next(self) -> Ea {
        match self {
            Ea::Bank0(a) => Ea::Bank0(a.wrapping_add(1)),
            Ea::Long(a) => Ea::Long((a + 1) & 0xFF_FFFF),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct W65816 {
    pub a: u16,
    pub x: u16,
    pub y: u16,
    pub s: u16,
    pub d: u16,
    pub dbr: u8,
    pub pbr: u8,
    pub pc: u16,
    pub p: u8,
    /// Emulation (6502) mode.
    pub e: bool,
    /// WAI: waiting for an interrupt.
    pub waiting: bool,
    /// STP: stopped until reset.
    pub stopped: bool,
    /// An NMI edge is waiting to be taken.
    pub nmi: bool,
    /// The IRQ line is held low.
    pub irq: bool,
}

impl W65816 {
    pub fn new() -> Self {
        W65816 {
            s: 0x01FF,
            p: M | X | I,
            e: true,
            ..Default::default()
        }
    }

    /// Reset: emulation mode, PC from the reset vector.
    pub fn reset(&mut self, bus: &mut impl Bus65816) {
        self.e = true;
        self.p = (self.p | M | X | I) & !D;
        self.x &= 0xFF;
        self.y &= 0xFF;
        self.s = 0x0100 | (self.s & 0xFF);
        self.d = 0;
        self.dbr = 0;
        self.pbr = 0;
        self.waiting = false;
        self.stopped = false;
        let lo = bus.read(VEC_RESET as u32);
        let hi = bus.read(VEC_RESET as u32 + 1);
        self.pc = u16::from_le_bytes([lo, hi]);
    }

    fn m8(&self) -> bool {
        self.p & M != 0
    }

    fn x8(&self) -> bool {
        self.p & X != 0
    }

    fn set_p(&mut self, value: u8) {
        self.p = value;
        if self.e {
            self.p |= M | X;
        }
        if self.p & X != 0 {
            self.x &= 0xFF;
            self.y &= 0xFF;
        }
    }

    fn set_nz8(&mut self, v: u8) {
        self.p = (self.p & !(N | Z)) | (v & N) | if v == 0 { Z } else { 0 };
    }

    fn set_nz16(&mut self, v: u16) {
        self.p = (self.p & !(N | Z)) | ((v >> 8) as u8 & N) | if v == 0 { Z } else { 0 };
    }

    fn set_nz(&mut self, v: u16, wide: bool) {
        if wide {
            self.set_nz16(v)
        } else {
            self.set_nz8(v as u8)
        }
    }

    fn flag(&mut self, f: u8, on: bool) {
        if on { self.p |= f } else { self.p &= !f }
    }

    // ---- bus helpers ---------------------------------------------------------------------

    fn fetch(&mut self, bus: &mut impl Bus65816) -> u8 {
        let v = bus.read((self.pbr as u32) << 16 | self.pc as u32);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    fn fetch16(&mut self, bus: &mut impl Bus65816) -> u16 {
        let lo = self.fetch(bus);
        let hi = self.fetch(bus);
        u16::from_le_bytes([lo, hi])
    }

    fn read_ea(&mut self, bus: &mut impl Bus65816, ea: Ea, wide: bool) -> u16 {
        let lo = bus.read(ea.addr());
        if !wide {
            return lo as u16;
        }
        let hi = bus.read(ea.next().addr());
        u16::from_le_bytes([lo, hi])
    }

    fn write_ea(&mut self, bus: &mut impl Bus65816, ea: Ea, wide: bool, v: u16) {
        bus.write(ea.addr(), v as u8);
        if wide {
            bus.write(ea.next().addr(), (v >> 8) as u8);
        }
    }

    /// Push, wrapping in page 1 in emulation mode.
    fn push(&mut self, bus: &mut impl Bus65816, v: u8) {
        bus.write(self.s as u32, v);
        self.s = if self.e {
            0x0100 | (self.s.wrapping_sub(1) & 0xFF)
        } else {
            self.s.wrapping_sub(1)
        };
    }

    fn pull(&mut self, bus: &mut impl Bus65816) -> u8 {
        self.s = if self.e {
            0x0100 | (self.s.wrapping_add(1) & 0xFF)
        } else {
            self.s.wrapping_add(1)
        };
        bus.read(self.s as u32)
    }

    /// Push for the 65816's own instructions: no page-1 wrap in emulation mode during the
    /// instruction (call [`W65816::fix_s`] after it).
    fn push_n(&mut self, bus: &mut impl Bus65816, v: u8) {
        bus.write(self.s as u32, v);
        self.s = self.s.wrapping_sub(1);
    }

    fn pull_n(&mut self, bus: &mut impl Bus65816) -> u8 {
        self.s = self.s.wrapping_add(1);
        bus.read(self.s as u32)
    }

    fn fix_s(&mut self) {
        if self.e {
            self.s = 0x0100 | (self.s & 0xFF);
        }
    }

    /// One internal cycle if the direct page register's low byte is not zero.
    fn dp_penalty(&mut self, bus: &mut impl Bus65816) {
        if self.d & 0xFF != 0 {
            bus.idle();
        }
    }

    /// Emulation mode with DL = 0: direct page indexing wraps inside the page.
    fn dp_page_wrap(&self) -> bool {
        self.e && self.d & 0xFF == 0
    }

    // ---- addressing modes ----------------------------------------------------------------

    fn ea_dp(&mut self, bus: &mut impl Bus65816) -> Ea {
        let o = self.fetch(bus) as u16;
        self.dp_penalty(bus);
        Ea::Bank0(self.d.wrapping_add(o))
    }

    fn ea_dp_indexed(&mut self, bus: &mut impl Bus65816, index: u16) -> Ea {
        let o = self.fetch(bus) as u16;
        self.dp_penalty(bus);
        bus.idle();
        if self.dp_page_wrap() {
            Ea::Bank0(self.d | (o.wrapping_add(index) & 0xFF))
        } else {
            Ea::Bank0(self.d.wrapping_add(o).wrapping_add(index))
        }
    }

    /// Reads a 16-bit pointer from the direct page at offset `o` (+ `index`), with the
    /// emulation-mode page wrap for the old addressing modes.
    fn dp_pointer(&mut self, bus: &mut impl Bus65816, o: u16, wrap: bool) -> u16 {
        let (lo_addr, hi_addr) = if wrap && self.dp_page_wrap() {
            (self.d | (o & 0xFF), self.d | (o.wrapping_add(1) & 0xFF))
        } else {
            let a = self.d.wrapping_add(o);
            (a, a.wrapping_add(1))
        };
        let lo = bus.read(lo_addr as u32);
        let hi = bus.read(hi_addr as u32);
        u16::from_le_bytes([lo, hi])
    }

    fn data_bank(&self, a: u16) -> u32 {
        (self.dbr as u32) << 16 | a as u32
    }

    /// (dp)
    fn ea_dp_ind(&mut self, bus: &mut impl Bus65816) -> Ea {
        let o = self.fetch(bus) as u16;
        self.dp_penalty(bus);
        let ptr = self.dp_pointer(bus, o, true);
        Ea::Long(self.data_bank(ptr))
    }

    /// (dp,X)
    fn ea_dp_x_ind(&mut self, bus: &mut impl Bus65816) -> Ea {
        let o = self.fetch(bus) as u16;
        self.dp_penalty(bus);
        bus.idle();
        let ptr = if self.dp_page_wrap() {
            self.dp_pointer(bus, o.wrapping_add(self.x) & 0xFF, true)
        } else {
            self.dp_pointer(bus, o.wrapping_add(self.x), false)
        };
        Ea::Long(self.data_bank(ptr))
    }

    /// (dp),Y; `write`: stores and read-modify-writes always take the index cycle.
    fn ea_dp_ind_y(&mut self, bus: &mut impl Bus65816, write: bool) -> Ea {
        let o = self.fetch(bus) as u16;
        self.dp_penalty(bus);
        let ptr = self.dp_pointer(bus, o, true);
        if write || !self.x8() || (ptr & 0xFF) + (self.y & 0xFF) > 0xFF {
            bus.idle();
        }
        Ea::Long(self.data_bank(ptr) + self.y as u32)
    }

    /// [dp] and [dp],Y
    fn ea_dp_long(&mut self, bus: &mut impl Bus65816, index: u16) -> Ea {
        let o = self.fetch(bus) as u16;
        self.dp_penalty(bus);
        let a = self.d.wrapping_add(o);
        let lo = bus.read(a as u32);
        let hi = bus.read(a.wrapping_add(1) as u32);
        let bank = bus.read(a.wrapping_add(2) as u32);
        let ptr = (bank as u32) << 16 | (hi as u32) << 8 | lo as u32;
        Ea::Long(ptr + index as u32)
    }

    fn ea_abs(&mut self, bus: &mut impl Bus65816) -> Ea {
        let a = self.fetch16(bus);
        Ea::Long(self.data_bank(a))
    }

    fn ea_abs_indexed(&mut self, bus: &mut impl Bus65816, index: u16, write: bool) -> Ea {
        let a = self.fetch16(bus);
        if write || !self.x8() || (a & 0xFF) + (index & 0xFF) > 0xFF {
            bus.idle();
        }
        Ea::Long(self.data_bank(a) + index as u32)
    }

    fn ea_long(&mut self, bus: &mut impl Bus65816, index: u16) -> Ea {
        let a = self.fetch16(bus) as u32;
        let bank = self.fetch(bus) as u32;
        Ea::Long((bank << 16 | a) + index as u32)
    }

    fn ea_sr(&mut self, bus: &mut impl Bus65816) -> Ea {
        let o = self.fetch(bus) as u16;
        bus.idle();
        Ea::Bank0(self.s.wrapping_add(o))
    }

    fn ea_sr_ind_y(&mut self, bus: &mut impl Bus65816) -> Ea {
        let o = self.fetch(bus) as u16;
        bus.idle();
        let a = self.s.wrapping_add(o);
        let lo = bus.read(a as u32);
        let hi = bus.read(a.wrapping_add(1) as u32);
        bus.idle();
        Ea::Long(self.data_bank(u16::from_le_bytes([lo, hi])) + self.y as u32)
    }

    /// The addressing mode of the `ORA`/`AND`/`EOR`/`ADC`/`STA`/`LDA`/`CMP`/`SBC` group,
    /// from the opcode's low 5 bits (immediate is handled by the caller).
    fn ea_group1(&mut self, bus: &mut impl Bus65816, op: u8, write: bool) -> Ea {
        match op & 0x1F {
            0x01 => self.ea_dp_x_ind(bus),
            0x03 => self.ea_sr(bus),
            0x05 => self.ea_dp(bus),
            0x07 => self.ea_dp_long(bus, 0),
            0x0D => self.ea_abs(bus),
            0x0F => self.ea_long(bus, 0),
            0x11 => self.ea_dp_ind_y(bus, write),
            0x12 => self.ea_dp_ind(bus),
            0x13 => self.ea_sr_ind_y(bus),
            0x15 => {
                let x = self.x;
                self.ea_dp_indexed(bus, x)
            }
            0x17 => {
                let y = self.y;
                self.ea_dp_long(bus, y)
            }
            0x19 => {
                let y = self.y;
                self.ea_abs_indexed(bus, y, write)
            }
            0x1D => {
                let x = self.x;
                self.ea_abs_indexed(bus, x, write)
            }
            0x1F => {
                let x = self.x;
                self.ea_long(bus, x)
            }
            _ => unreachable!("not a group 1 mode: {op:02X}"),
        }
    }

    /// The operand of a read instruction: immediate (from the instruction) or from memory.
    fn operand(&mut self, bus: &mut impl Bus65816, ea: Option<Ea>, wide: bool) -> u16 {
        match ea {
            Some(ea) => self.read_ea(bus, ea, wide),
            None if wide => self.fetch16(bus),
            None => self.fetch(bus) as u16,
        }
    }

    // ---- arithmetic ----------------------------------------------------------------------

    fn adc(&mut self, v: u16) {
        let c = (self.p & C) as u32;
        let decimal = self.p & D != 0;
        if self.m8() {
            let a = self.a as u32 & 0xFF;
            let v = v as u32 & 0xFF;
            let mut r;
            if !decimal {
                r = a + v + c;
            } else {
                r = (a & 0x0F) + (v & 0x0F) + c;
                if r > 0x09 {
                    r += 0x06;
                }
                let carry = (r > 0x0F) as u32;
                r = (a & 0xF0) + (v & 0xF0) + (carry << 4) + (r & 0x0F);
            }
            self.flag(V, !(a ^ v) & (a ^ r) & 0x80 != 0);
            if decimal && r > 0x9F {
                r += 0x60;
            }
            self.flag(C, r > 0xFF);
            self.a = (self.a & 0xFF00) | (r as u16 & 0xFF);
            self.set_nz8(r as u8);
        } else {
            let a = self.a as u32;
            let v = v as u32;
            let mut r;
            if !decimal {
                r = a + v + c;
            } else {
                r = (a & 0x000F) + (v & 0x000F) + c;
                if r > 0x0009 {
                    r += 0x0006;
                }
                let mut carry = (r > 0x000F) as u32;
                r = (a & 0x00F0) + (v & 0x00F0) + (carry << 4) + (r & 0x000F);
                if r > 0x009F {
                    r += 0x0060;
                }
                carry = (r > 0x00FF) as u32;
                r = (a & 0x0F00) + (v & 0x0F00) + (carry << 8) + (r & 0x00FF);
                if r > 0x09FF {
                    r += 0x0600;
                }
                carry = (r > 0x0FFF) as u32;
                r = (a & 0xF000) + (v & 0xF000) + (carry << 12) + (r & 0x0FFF);
            }
            self.flag(V, !(a ^ v) & (a ^ r) & 0x8000 != 0);
            if decimal && r > 0x9FFF {
                r += 0x6000;
            }
            self.flag(C, r > 0xFFFF);
            self.a = r as u16;
            self.set_nz16(r as u16);
        }
    }

    fn sbc(&mut self, v: u16) {
        let c = (self.p & C) as i32;
        let decimal = self.p & D != 0;
        if self.m8() {
            let a = self.a as i32 & 0xFF;
            let v = !v as i32 & 0xFF;
            let mut r;
            if !decimal {
                r = a + v + c;
            } else {
                r = (a & 0x0F) + (v & 0x0F) + c;
                if r <= 0x0F {
                    r -= 0x06;
                }
                let carry = (r > 0x0F) as i32;
                r = (a & 0xF0) + (v & 0xF0) + (carry << 4) + (r & 0x0F);
            }
            self.flag(V, !(a ^ v) & (a ^ r) & 0x80 != 0);
            if decimal && r <= 0xFF {
                r -= 0x60;
            }
            self.flag(C, r > 0xFF);
            self.a = (self.a & 0xFF00) | (r as u16 & 0xFF);
            self.set_nz8(r as u8);
        } else {
            let a = self.a as i32;
            let v = !v as i32 & 0xFFFF;
            let mut r;
            if !decimal {
                r = a + v + c;
            } else {
                r = (a & 0x000F) + (v & 0x000F) + c;
                if r <= 0x000F {
                    r -= 0x0006;
                }
                let mut carry = (r > 0x000F) as i32;
                r = (a & 0x00F0) + (v & 0x00F0) + (carry << 4) + (r & 0x000F);
                if r <= 0x00FF {
                    r -= 0x0060;
                }
                carry = (r > 0x00FF) as i32;
                r = (a & 0x0F00) + (v & 0x0F00) + (carry << 8) + (r & 0x00FF);
                if r <= 0x0FFF {
                    r -= 0x0600;
                }
                carry = (r > 0x0FFF) as i32;
                r = (a & 0xF000) + (v & 0xF000) + (carry << 12) + (r & 0x0FFF);
            }
            self.flag(V, !(a ^ v) & (a ^ r) & 0x8000 != 0);
            if decimal && r <= 0xFFFF {
                r -= 0x6000;
            }
            self.flag(C, r > 0xFFFF);
            self.a = r as u16;
            self.set_nz16(r as u16);
        }
    }

    fn compare(&mut self, reg: u16, v: u16, wide: bool) {
        let mask = if wide { 0xFFFF } else { 0xFF };
        let r = (reg & mask).wrapping_sub(v & mask) & mask;
        self.flag(C, reg & mask >= v & mask);
        self.set_nz(r, wide);
    }

    /// Shift and rotate operations and INC/DEC/TSB/TRB on a value of the given width.
    fn modify(&mut self, kind: u8, v: u16, wide: bool) -> u16 {
        let (mask, top) = if wide { (0xFFFF, 0x8000) } else { (0xFF, 0x80) };
        let v = v & mask;
        let r = match kind {
            // ASL
            0 => {
                self.flag(C, v & top != 0);
                (v << 1) & mask
            }
            // ROL
            1 => {
                let c = (self.p & C) as u16;
                self.flag(C, v & top != 0);
                ((v << 1) | c) & mask
            }
            // LSR
            2 => {
                self.flag(C, v & 1 != 0);
                v >> 1
            }
            // ROR
            3 => {
                let c = if self.p & C != 0 { top } else { 0 };
                self.flag(C, v & 1 != 0);
                (v >> 1) | c
            }
            // INC
            4 => v.wrapping_add(1) & mask,
            // DEC
            5 => v.wrapping_sub(1) & mask,
            // TSB
            6 => {
                self.flag(Z, v & self.a & mask == 0);
                return v | (self.a & mask);
            }
            // TRB
            _ => {
                self.flag(Z, v & self.a & mask == 0);
                return v & !self.a & mask;
            }
        };
        self.set_nz(r, wide);
        r
    }

    /// A read-modify-write on memory: read, an internal cycle, write (high byte first).
    fn rmw(&mut self, bus: &mut impl Bus65816, ea: Ea, kind: u8) {
        let wide = !self.m8();
        let v = self.read_ea(bus, ea, wide);
        bus.idle();
        let r = self.modify(kind, v, wide);
        if wide {
            bus.write(ea.next().addr(), (r >> 8) as u8);
        }
        bus.write(ea.addr(), r as u8);
    }

    fn set_a(&mut self, v: u16) {
        if self.m8() {
            self.a = (self.a & 0xFF00) | (v & 0xFF);
            self.set_nz8(v as u8);
        } else {
            self.a = v;
            self.set_nz16(v);
        }
    }

    fn set_index(&mut self, which_y: bool, v: u16) {
        let v = if self.x8() { v & 0xFF } else { v };
        if which_y {
            self.y = v;
        } else {
            self.x = v;
        }
        self.set_nz(v, !self.x8());
    }

    // ---- interrupts ----------------------------------------------------------------------

    fn interrupt(&mut self, bus: &mut impl Bus65816, vectors: (u16, u16), brk: bool) {
        if !self.e {
            self.push(bus, self.pbr);
        }
        let [hi, lo] = self.pc.to_be_bytes();
        self.push(bus, hi);
        self.push(bus, lo);
        let p = if self.e && !brk { self.p & !X } else { self.p };
        self.push(bus, p);
        self.p = (self.p | I) & !D;
        self.pbr = 0;
        let v = if self.e { vectors.1 } else { vectors.0 };
        let lo = bus.read(v as u32);
        let hi = bus.read(v as u32 + 1);
        self.pc = u16::from_le_bytes([lo, hi]);
    }

    fn branch(&mut self, bus: &mut impl Bus65816, taken: bool) {
        let o = self.fetch(bus) as i8;
        if taken {
            bus.idle();
            let target = self.pc.wrapping_add(o as u16);
            if self.e && target & 0xFF00 != self.pc & 0xFF00 {
                bus.idle();
            }
            self.pc = target;
        }
    }

    /// Runs one instruction, or takes a pending interrupt.
    pub fn step(&mut self, bus: &mut impl Bus65816) {
        if self.stopped {
            bus.idle();
            return;
        }
        if self.waiting {
            if self.nmi || self.irq {
                self.waiting = false;
                bus.idle();
            } else {
                bus.idle();
                return;
            }
        }
        if self.nmi {
            self.nmi = false;
            bus.read((self.pbr as u32) << 16 | self.pc as u32);
            bus.idle();
            self.interrupt(bus, VEC_NMI, false);
            return;
        }
        if self.irq && self.p & I == 0 {
            bus.read((self.pbr as u32) << 16 | self.pc as u32);
            bus.idle();
            self.interrupt(bus, VEC_IRQ, false);
            return;
        }
        let op = self.fetch(bus);
        self.execute(bus, op);
    }

    fn execute(&mut self, bus: &mut impl Bus65816, op: u8) {
        let m16 = !self.m8();
        let x16 = !self.x8();
        match op {
            // ---- group 1: ORA AND EOR ADC STA LDA CMP SBC ----
            _ if op != 0x89
                && matches!(
                    op & 0x1F,
                    0x01 | 0x03
                        | 0x05
                        | 0x07
                        | 0x09
                        | 0x0D
                        | 0x0F
                        | 0x11
                        | 0x12
                        | 0x13
                        | 0x15
                        | 0x17
                        | 0x19
                        | 0x1D
                        | 0x1F
                ) =>
            {
                let kind = op >> 5;
                if kind == 4 {
                    // STA
                    let ea = self.ea_group1(bus, op, true);
                    self.write_ea(bus, ea, m16, self.a);
                    return;
                }
                let ea = if op & 0x1F == 0x09 {
                    None
                } else {
                    Some(self.ea_group1(bus, op, false))
                };
                let v = self.operand(bus, ea, m16);
                match kind {
                    0 => self.set_a(self.a | v),
                    1 => self.set_a(self.a & v),
                    2 => self.set_a(self.a ^ v),
                    3 => self.adc(v),
                    5 => self.set_a(v),
                    6 => self.compare(self.a, v, m16),
                    _ => self.sbc(v),
                }
            }

            // ---- loads and stores of X, Y and zero ----
            0xA0 | 0xA2 | 0xA4 | 0xA6 | 0xAC | 0xAE | 0xB4 | 0xB6 | 0xBC | 0xBE => {
                let is_y = op & 0x02 == 0;
                let ea = match op {
                    0xA0 | 0xA2 => None,
                    0xA4 | 0xA6 => Some(self.ea_dp(bus)),
                    0xAC | 0xAE => Some(self.ea_abs(bus)),
                    0xB4 => {
                        let x = self.x;
                        Some(self.ea_dp_indexed(bus, x))
                    }
                    0xB6 => {
                        let y = self.y;
                        Some(self.ea_dp_indexed(bus, y))
                    }
                    0xBC => {
                        let x = self.x;
                        Some(self.ea_abs_indexed(bus, x, false))
                    }
                    _ => {
                        let y = self.y;
                        Some(self.ea_abs_indexed(bus, y, false))
                    }
                };
                let v = self.operand(bus, ea, x16);
                self.set_index(is_y, v);
            }
            0x84 | 0x86 | 0x8C | 0x8E | 0x94 | 0x96 => {
                let v = if op & 0x02 == 0 { self.y } else { self.x };
                let ea = match op {
                    0x84 | 0x86 => self.ea_dp(bus),
                    0x8C | 0x8E => self.ea_abs(bus),
                    0x94 => {
                        let x = self.x;
                        self.ea_dp_indexed(bus, x)
                    }
                    _ => {
                        let y = self.y;
                        self.ea_dp_indexed(bus, y)
                    }
                };
                self.write_ea(bus, ea, x16, v);
            }
            0x64 | 0x74 | 0x9C | 0x9E => {
                let ea = match op {
                    0x64 => self.ea_dp(bus),
                    0x74 => {
                        let x = self.x;
                        self.ea_dp_indexed(bus, x)
                    }
                    0x9C => self.ea_abs(bus),
                    _ => {
                        let x = self.x;
                        self.ea_abs_indexed(bus, x, true)
                    }
                };
                self.write_ea(bus, ea, m16, 0);
            }

            // ---- compares of X and Y ----
            0xC0 | 0xC4 | 0xCC | 0xE0 | 0xE4 | 0xEC => {
                let ea = match op & 0x0F {
                    0x00 => None,
                    0x04 => Some(self.ea_dp(bus)),
                    _ => Some(self.ea_abs(bus)),
                };
                let v = self.operand(bus, ea, x16);
                let reg = if op < 0xE0 { self.y } else { self.x };
                self.compare(reg, v, x16);
            }

            // ---- BIT ----
            0x89 => {
                let v = self.operand(bus, None, m16);
                let mask = if m16 { 0xFFFF } else { 0xFF };
                self.flag(Z, self.a & v & mask == 0);
            }
            0x24 | 0x2C | 0x34 | 0x3C => {
                let ea = match op {
                    0x24 => self.ea_dp(bus),
                    0x2C => self.ea_abs(bus),
                    0x34 => {
                        let x = self.x;
                        self.ea_dp_indexed(bus, x)
                    }
                    _ => {
                        let x = self.x;
                        self.ea_abs_indexed(bus, x, false)
                    }
                };
                let v = self.read_ea(bus, ea, m16);
                let (mask, top) = if m16 { (0xFFFF, 0x8000) } else { (0xFF, 0x80) };
                self.flag(Z, self.a & v & mask == 0);
                self.flag(N, v & top != 0);
                self.flag(V, v & (top >> 1) != 0);
            }

            // ---- read-modify-write: ASL ROL LSR ROR on A and memory, INC DEC, TSB TRB ----
            0x0A | 0x2A | 0x4A | 0x6A | 0x1A | 0x3A => {
                bus.idle();
                let kind = match op {
                    0x0A => 0,
                    0x2A => 1,
                    0x4A => 2,
                    0x6A => 3,
                    0x1A => 4,
                    _ => 5,
                };
                let r = self.modify(kind, self.a, m16);
                if m16 {
                    self.a = r;
                } else {
                    self.a = (self.a & 0xFF00) | r;
                }
            }
            0x06 | 0x0E | 0x16 | 0x1E | 0x26 | 0x2E | 0x36 | 0x3E | 0x46 | 0x4E | 0x56 | 0x5E
            | 0x66 | 0x6E | 0x76 | 0x7E | 0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => {
                let kind = match op >> 5 {
                    0 => 0,
                    1 => 1,
                    2 => 2,
                    3 => 3,
                    6 => 5,
                    _ => 4,
                };
                let ea = match op & 0x1F {
                    0x06 => self.ea_dp(bus),
                    0x0E => self.ea_abs(bus),
                    0x16 => {
                        let x = self.x;
                        self.ea_dp_indexed(bus, x)
                    }
                    _ => {
                        let x = self.x;
                        self.ea_abs_indexed(bus, x, true)
                    }
                };
                self.rmw(bus, ea, kind);
            }
            0x04 | 0x0C | 0x14 | 0x1C => {
                let ea = if op & 0x08 == 0 {
                    self.ea_dp(bus)
                } else {
                    self.ea_abs(bus)
                };
                self.rmw(bus, ea, if op & 0x10 == 0 { 6 } else { 7 });
            }

            // ---- index register increments ----
            0xE8 | 0xC8 | 0xCA | 0x88 => {
                bus.idle();
                let is_y = op == 0xC8 || op == 0x88;
                let reg = if is_y { self.y } else { self.x };
                let v = if op == 0xE8 || op == 0xC8 {
                    reg.wrapping_add(1)
                } else {
                    reg.wrapping_sub(1)
                };
                self.set_index(is_y, v);
            }

            // ---- transfers ----
            0xAA | 0xA8 | 0x8A | 0x98 | 0x9B | 0xBB | 0xBA | 0x9A | 0x5B | 0x7B | 0x1B | 0x3B => {
                bus.idle();
                match op {
                    0xAA => self.set_index(false, self.a),
                    0xA8 => self.set_index(true, self.a),
                    0x8A => self.set_a(self.x),
                    0x98 => self.set_a(self.y),
                    0x9B => self.set_index(true, self.x),
                    0xBB => self.set_index(false, self.y),
                    0xBA => self.set_index(false, self.s),
                    0x9A => {
                        self.s = if self.e {
                            0x0100 | (self.x & 0xFF)
                        } else {
                            self.x
                        }
                    }
                    0x5B => {
                        self.d = self.a;
                        self.set_nz16(self.a);
                    }
                    0x7B => {
                        self.a = self.d;
                        self.set_nz16(self.d);
                    }
                    0x1B => {
                        self.s = if self.e {
                            0x0100 | (self.a & 0xFF)
                        } else {
                            self.a
                        }
                    }
                    _ => {
                        self.a = self.s;
                        self.set_nz16(self.s);
                    }
                }
            }
            0xEB => {
                // XBA
                bus.idle();
                bus.idle();
                self.a = self.a.rotate_left(8);
                self.set_nz8(self.a as u8);
            }

            // ---- flags ----
            0x18 | 0x38 | 0x58 | 0x78 | 0xB8 | 0xD8 | 0xF8 => {
                bus.idle();
                match op {
                    0x18 => self.p &= !C,
                    0x38 => self.p |= C,
                    0x58 => self.p &= !I,
                    0x78 => self.p |= I,
                    0xB8 => self.p &= !V,
                    0xD8 => self.p &= !D,
                    _ => self.p |= D,
                }
            }
            0xC2 | 0xE2 => {
                let v = self.fetch(bus);
                bus.idle();
                let p = if op == 0xC2 { self.p & !v } else { self.p | v };
                self.set_p(p);
            }
            0xFB => {
                // XCE
                bus.idle();
                let c = self.p & C != 0;
                self.flag(C, self.e);
                self.e = c;
                if self.e {
                    self.set_p(self.p | M | X);
                    self.s = 0x0100 | (self.s & 0xFF);
                }
            }

            // ---- stack ----
            0x48 | 0xDA | 0x5A => {
                bus.idle();
                let (v, wide) = match op {
                    0x48 => (self.a, m16),
                    0xDA => (self.x, x16),
                    _ => (self.y, x16),
                };
                if wide {
                    self.push(bus, (v >> 8) as u8);
                }
                self.push(bus, v as u8);
            }
            0x68 | 0xFA | 0x7A => {
                bus.idle();
                bus.idle();
                let wide = if op == 0x68 { m16 } else { x16 };
                let lo = self.pull(bus) as u16;
                let v = if wide {
                    lo | (self.pull(bus) as u16) << 8
                } else {
                    lo
                };
                match op {
                    0x68 => self.set_a(v),
                    0xFA => self.set_index(false, v),
                    _ => self.set_index(true, v),
                }
            }
            0x08 => {
                bus.idle();
                let p = self.p;
                self.push(bus, p);
            }
            0x28 => {
                bus.idle();
                bus.idle();
                let p = self.pull(bus);
                self.set_p(p);
            }
            0x8B => {
                bus.idle();
                let b = self.dbr;
                self.push(bus, b);
            }
            0x4B => {
                bus.idle();
                let k = self.pbr;
                self.push(bus, k);
            }
            0xAB => {
                bus.idle();
                bus.idle();
                self.dbr = self.pull_n(bus);
                self.fix_s();
                self.set_nz8(self.dbr);
            }
            0x0B => {
                bus.idle();
                let [hi, lo] = self.d.to_be_bytes();
                self.push_n(bus, hi);
                self.push_n(bus, lo);
                self.fix_s();
            }
            0x2B => {
                bus.idle();
                bus.idle();
                let lo = self.pull_n(bus);
                let hi = self.pull_n(bus);
                self.fix_s();
                self.d = u16::from_le_bytes([lo, hi]);
                self.set_nz16(self.d);
            }
            0xF4 => {
                // PEA
                let v = self.fetch16(bus);
                self.push_n(bus, (v >> 8) as u8);
                self.push_n(bus, v as u8);
                self.fix_s();
            }
            0xD4 => {
                // PEI
                let o = self.fetch(bus) as u16;
                self.dp_penalty(bus);
                let v = self.dp_pointer(bus, o, false);
                self.push_n(bus, (v >> 8) as u8);
                self.push_n(bus, v as u8);
                self.fix_s();
            }
            0x62 => {
                // PER
                let o = self.fetch16(bus);
                bus.idle();
                let v = self.pc.wrapping_add(o);
                self.push_n(bus, (v >> 8) as u8);
                self.push_n(bus, v as u8);
                self.fix_s();
            }

            // ---- jumps and calls ----
            0x4C => self.pc = self.fetch16(bus),
            0x5C => {
                let a = self.fetch16(bus);
                self.pbr = self.fetch(bus);
                self.pc = a;
            }
            0x6C => {
                let a = self.fetch16(bus);
                let lo = bus.read(a as u32);
                let hi = bus.read(a.wrapping_add(1) as u32);
                self.pc = u16::from_le_bytes([lo, hi]);
            }
            0x7C => {
                let a = self.fetch16(bus).wrapping_add(self.x);
                bus.idle();
                let bank = (self.pbr as u32) << 16;
                let lo = bus.read(bank | a as u32);
                let hi = bus.read(bank | a.wrapping_add(1) as u32);
                self.pc = u16::from_le_bytes([lo, hi]);
            }
            0xDC => {
                let a = self.fetch16(bus);
                let lo = bus.read(a as u32);
                let hi = bus.read(a.wrapping_add(1) as u32);
                self.pbr = bus.read(a.wrapping_add(2) as u32);
                self.pc = u16::from_le_bytes([lo, hi]);
            }
            0x20 => {
                let a = self.fetch16(bus);
                bus.idle();
                let [hi, lo] = self.pc.wrapping_sub(1).to_be_bytes();
                self.push(bus, hi);
                self.push(bus, lo);
                self.pc = a;
            }
            0xFC => {
                // JSR (abs,X)
                let lo = self.fetch(bus);
                let [rh, rl] = self.pc.to_be_bytes();
                self.push_n(bus, rh);
                self.push_n(bus, rl);
                let hi = self.fetch(bus);
                bus.idle();
                let a = u16::from_le_bytes([lo, hi]).wrapping_add(self.x);
                let bank = (self.pbr as u32) << 16;
                let tlo = bus.read(bank | a as u32);
                let thi = bus.read(bank | a.wrapping_add(1) as u32);
                self.fix_s();
                self.pc = u16::from_le_bytes([tlo, thi]);
            }
            0x22 => {
                // JSL
                let a = self.fetch16(bus);
                let k = self.pbr;
                self.push_n(bus, k);
                bus.idle();
                let bank = self.fetch(bus);
                let [hi, lo] = self.pc.wrapping_sub(1).to_be_bytes();
                self.push_n(bus, hi);
                self.push_n(bus, lo);
                self.fix_s();
                self.pbr = bank;
                self.pc = a;
            }
            0x60 => {
                bus.idle();
                bus.idle();
                let lo = self.pull(bus);
                let hi = self.pull(bus);
                bus.idle();
                self.pc = u16::from_le_bytes([lo, hi]).wrapping_add(1);
            }
            0x6B => {
                bus.idle();
                bus.idle();
                let lo = self.pull_n(bus);
                let hi = self.pull_n(bus);
                self.pbr = self.pull_n(bus);
                self.fix_s();
                self.pc = u16::from_le_bytes([lo, hi]).wrapping_add(1);
            }
            0x40 => {
                bus.idle();
                bus.idle();
                let p = self.pull(bus);
                self.set_p(p);
                let lo = self.pull(bus);
                let hi = self.pull(bus);
                self.pc = u16::from_le_bytes([lo, hi]);
                if !self.e {
                    self.pbr = self.pull(bus);
                }
            }

            // ---- branches ----
            0x10 => self.branch(bus, self.p & N == 0),
            0x30 => self.branch(bus, self.p & N != 0),
            0x50 => self.branch(bus, self.p & V == 0),
            0x70 => self.branch(bus, self.p & V != 0),
            0x90 => self.branch(bus, self.p & C == 0),
            0xB0 => self.branch(bus, self.p & C != 0),
            0xD0 => self.branch(bus, self.p & Z == 0),
            0xF0 => self.branch(bus, self.p & Z != 0),
            0x80 => self.branch(bus, true),
            0x82 => {
                let o = self.fetch16(bus);
                bus.idle();
                self.pc = self.pc.wrapping_add(o);
            }

            // ---- interrupts and misc ----
            0x00 | 0x02 => {
                self.fetch(bus);
                let v = if op == 0x00 { VEC_BRK } else { VEC_COP };
                self.interrupt(bus, v, true);
            }
            0x42 => {
                // WDM: a reserved 2-byte no-op; its second byte is skipped, not read
                bus.idle();
                self.pc = self.pc.wrapping_add(1);
            }
            0xEA => bus.idle(),
            0xCB => {
                bus.idle();
                bus.idle();
                bus.idle();
                self.waiting = true;
            }
            0xDB => {
                bus.idle();
                bus.idle();
                bus.idle();
                self.stopped = true;
            }
            0x44 | 0x54 => {
                // MVP / MVN: one byte per run; repeats until A wraps to $FFFF
                let dst = self.fetch(bus);
                let src = self.fetch(bus);
                self.dbr = dst;
                let v = bus.read((src as u32) << 16 | self.x as u32);
                bus.write((dst as u32) << 16 | self.y as u32, v);
                bus.idle();
                bus.idle();
                let step = |r: u16, x8: bool| -> u16 {
                    let r = if op == 0x54 {
                        r.wrapping_add(1)
                    } else {
                        r.wrapping_sub(1)
                    };
                    if x8 { r & 0xFF } else { r }
                };
                let x8 = self.x8();
                self.x = step(self.x, x8);
                self.y = step(self.y, x8);
                self.a = self.a.wrapping_sub(1);
                if self.a != 0xFFFF {
                    self.pc = self.pc.wrapping_sub(3);
                }
            }
            _ => unreachable!("unhandled opcode {op:02X}"),
        }
    }
}
