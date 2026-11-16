//! The Sharp SM83 CPU.
//!
//! Only a handful of opcodes are implemented so far; everything else returns
//! [`CpuError::UnimplementedOpcode`]. See <https://gbdev.io/gb-opcodes/optables/>.

use std::fmt;

use crate::bus::Bus;

pub mod flag {
    pub const Z: u8 = 1 << 7;
    pub const N: u8 = 1 << 6;
    pub const H: u8 = 1 << 5;
    pub const C: u8 = 1 << 4;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpuError {
    UnimplementedOpcode { opcode: u8, pc: u16 },
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnimplementedOpcode { opcode, pc } => {
                write!(f, "unimplemented opcode ${opcode:02X} at ${pc:04X}")
            }
        }
    }
}

impl std::error::Error for CpuError {}

#[derive(Debug, Default, Clone)]
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
}

pub struct Cpu {
    pub regs: Registers,
    /// Interrupt master enable.
    pub ime: bool,
    /// EI enables interrupts after the *following* instruction.
    ime_pending: bool,
    pub halted: bool,
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
        }
    }

    fn fetch(&mut self, bus: &Bus) -> u8 {
        let byte = bus.read(self.regs.pc);
        self.regs.pc = self.regs.pc.wrapping_add(1);
        byte
    }

    fn fetch_u16(&mut self, bus: &Bus) -> u16 {
        let word = bus.read_u16(self.regs.pc);
        self.regs.pc = self.regs.pc.wrapping_add(2);
        word
    }

    fn push(&mut self, bus: &mut Bus, value: u16) {
        let [hi, lo] = value.to_be_bytes();
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        bus.write(self.regs.sp, hi);
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        bus.write(self.regs.sp, lo);
    }

    /// Dispatches a pending interrupt, if any. Returns the T-cycles spent.
    fn handle_interrupts(&mut self, bus: &mut Bus) -> u32 {
        let pending = bus.interrupt_enable & bus.interrupt_flag & 0x1F;
        if pending == 0 {
            return 0;
        }
        self.halted = false;
        if !self.ime {
            return 0;
        }
        let bit = pending.trailing_zeros() as u16;
        bus.interrupt_flag &= !(1 << bit);
        self.ime = false;
        self.push(bus, self.regs.pc);
        self.regs.pc = 0x0040 + bit * 8;
        20
    }

    /// Executes one instruction (or services an interrupt) and returns T-cycles taken.
    pub fn step(&mut self, bus: &mut Bus) -> Result<u32, CpuError> {
        let irq_cycles = self.handle_interrupts(bus);
        if irq_cycles > 0 {
            return Ok(irq_cycles);
        }
        if self.halted {
            return Ok(4);
        }
        if self.ime_pending {
            self.ime_pending = false;
            self.ime = true;
        }

        let pc = self.regs.pc;
        let opcode = self.fetch(bus);
        let cycles = match opcode {
            // NOP
            0x00 => 4,
            // JR e8
            0x18 => {
                let offset = self.fetch(bus) as i8;
                self.regs.pc = self.regs.pc.wrapping_add_signed(offset.into());
                12
            }
            // HALT
            0x76 => {
                self.halted = true;
                4
            }
            // XOR A, A
            0xAF => {
                self.regs.a = 0;
                self.regs.f = flag::Z;
                4
            }
            // JP a16
            0xC3 => {
                self.regs.pc = self.fetch_u16(bus);
                16
            }
            // DI
            0xF3 => {
                self.ime = false;
                self.ime_pending = false;
                4
            }
            // EI
            0xFB => {
                self.ime_pending = true;
                4
            }
            _ => {
                self.regs.pc = pc;
                return Err(CpuError::UnimplementedOpcode { opcode, pc });
            }
        };
        Ok(cycles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
