//! The SNES sound CPU (Sony SPC700 in the S-SMP) with its 64 KiB of RAM, I/O ports and
//! timers, for the Super Game Boy's sound (see [`crate::sgb_sound`]). Written from the
//! "fullsnes" hardware documentation (nocash): opcodes, flags and cycle counts per
//! instruction, the CONTROL/timer/port registers, and the DSP behind $F2/$F3.
//!
//! The 64-byte boot ROM at $FFC0 is not emulated: freki loads programs into RAM itself
//! (as the ROM's upload protocol would) and starts them; a program that jumps into the
//! ROM waits there until the next upload.

use crate::snes_dsp::Dsp;

/// SPC700 cycles per second, and per DSP sample (32 kHz).
pub const CLOCK_HZ: u32 = 1_024_000;
pub const CYCLES_PER_SAMPLE: u32 = 32;

/// Cycles per opcode (fullsnes); taken branches add 2.
const CYCLES: [u8; 256] = [
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 5, 4, 5, 4, 6, 8, //
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 6, 5, 2, 2, 4, 6, //
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 5, 4, 5, 4, 5, 4, //
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 6, 5, 2, 2, 3, 8, //
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 4, 4, 5, 4, 6, 6, //
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 4, 5, 2, 2, 4, 3, //
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 4, 4, 5, 4, 5, 5, //
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 5, 5, 2, 2, 3, 6, //
    2, 8, 4, 5, 3, 4, 3, 6, 2, 6, 5, 4, 5, 2, 4, 5, //
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 5, 5, 2, 2, 12, 5, //
    3, 8, 4, 5, 3, 4, 3, 6, 2, 6, 4, 4, 5, 2, 4, 4, //
    2, 8, 4, 5, 4, 5, 5, 6, 5, 5, 5, 5, 2, 2, 3, 4, //
    3, 8, 4, 5, 4, 5, 4, 7, 2, 5, 6, 4, 5, 2, 4, 9, //
    2, 8, 4, 5, 5, 6, 6, 7, 4, 5, 5, 5, 2, 2, 6, 3, //
    2, 8, 4, 5, 3, 4, 3, 6, 2, 4, 5, 3, 4, 3, 4, 2, //
    2, 8, 4, 5, 4, 5, 5, 6, 3, 4, 5, 4, 2, 2, 4, 2, //
];

const N: u8 = 0x80;
const V: u8 = 0x40;
const P: u8 = 0x20;
const B: u8 = 0x10;
const H: u8 = 0x08;
const I: u8 = 0x04;
const Z: u8 = 0x02;
const C: u8 = 0x01;

/// A timer: counts its clock (8 kHz or 64 kHz) up to the divider, then bumps a 4-bit
/// output that reads back (and clears) at $FD–$FF.
#[derive(Clone, Default)]
struct Timer {
    enabled: bool,
    /// $FA–$FC; 0 divides by 256.
    divider: u8,
    stage: u8,
    out: u8,
}

impl Timer {
    fn clock(&mut self) {
        if !self.enabled {
            return;
        }
        self.stage = self.stage.wrapping_add(1);
        if self.stage == self.divider {
            self.stage = 0;
            self.out = (self.out + 1) & 0x0F;
        }
    }
}

/// The S-SMP: CPU, RAM, ports, timers, and the DSP it talks to.
pub struct Smp {
    pub ram: Box<[u8; 0x10000]>,
    pub dsp: Dsp,
    a: u8,
    x: u8,
    y: u8,
    sp: u8,
    pc: u16,
    psw: u8,
    /// Ports $F4–$F7 as the main CPU (the SNES side of the SGB) wrote them.
    pub input: [u8; 4],
    /// Ports $F4–$F7 as the SPC700 wrote them, for the main CPU.
    pub output: [u8; 4],
    dsp_addr: u8,
    rom_enabled: bool,
    timers: [Timer; 3],
    /// Cycles towards the next 64 kHz tick, 64 kHz ticks towards the next 8 kHz one, and
    /// cycles towards the next DSP sample.
    timer_cycles: u32,
    ticks_64k: u32,
    sample_cycles: u32,
    /// Stopped by SLEEP/STOP, or waiting in the boot ROM for an upload.
    halted: bool,
}

impl Default for Smp {
    fn default() -> Self {
        Self::new()
    }
}

impl Smp {
    pub fn new() -> Self {
        Self {
            ram: Box::new([0; 0x10000]),
            dsp: Dsp::new(),
            a: 0,
            x: 0,
            y: 0,
            sp: 0xEF,
            pc: 0xFFC0,
            psw: Z,
            input: [0; 4],
            output: [0; 4],
            dsp_addr: 0,
            rom_enabled: true,
            timers: Default::default(),
            timer_cycles: 0,
            ticks_64k: 0,
            sample_cycles: 0,
            halted: true,
        }
    }

    /// Copies `data` to RAM at `addr`, as the boot ROM's upload protocol does.
    pub fn upload(&mut self, addr: u16, data: &[u8]) {
        for (i, &b) in data.iter().enumerate() {
            self.ram[usize::from(addr.wrapping_add(i as u16))] = b;
        }
    }

    /// Starts the program at `entry` the way the boot ROM hands over after an upload:
    /// direct page cleared ($0002–$00EF), A=X=Y=0, SP=$EF, PSW=$02.
    pub fn start(&mut self, entry: u16) {
        self.ram[0x0002..0x00F0].fill(0);
        self.ram[0..2].copy_from_slice(&entry.to_le_bytes());
        self.a = 0;
        self.x = 0;
        self.y = 0;
        self.sp = 0xEF;
        self.psw = Z;
        self.pc = entry;
        self.halted = false;
    }

    pub fn halted(&self) -> bool {
        self.halted
    }

    /// Runs `cycles` SPC700 cycles; pushes each 32 kHz stereo sample to `out`.
    pub fn run(&mut self, cycles: u32, out: &mut Vec<(i16, i16)>) {
        let mut left = i64::from(cycles);
        while left > 0 {
            let used = if self.halted { 2 } else { self.step() };
            left -= i64::from(used);
            self.advance(used, out);
        }
    }

    fn advance(&mut self, cycles: u32, out: &mut Vec<(i16, i16)>) {
        self.timer_cycles += cycles;
        while self.timer_cycles >= 16 {
            self.timer_cycles -= 16;
            self.timers[2].clock();
            self.ticks_64k += 1;
            if self.ticks_64k == 8 {
                self.ticks_64k = 0;
                self.timers[0].clock();
                self.timers[1].clock();
            }
        }
        self.sample_cycles += cycles;
        while self.sample_cycles >= CYCLES_PER_SAMPLE {
            self.sample_cycles -= CYCLES_PER_SAMPLE;
            out.push(self.dsp.sample(&mut self.ram));
        }
    }

    // ---- memory ------------------------------------------------------------------------

    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0x00F0..=0x00FF => self.read_io(addr),
            _ => self.ram[usize::from(addr)],
        }
    }

    fn read_io(&mut self, addr: u16) -> u8 {
        match addr {
            0xF2 => self.dsp_addr,
            0xF3 => self.dsp.read(self.dsp_addr & 0x7F),
            0xF4..=0xF7 => self.input[usize::from(addr - 0xF4)],
            0xF8 | 0xF9 => self.ram[usize::from(addr)],
            0xFD..=0xFF => {
                let timer = &mut self.timers[usize::from(addr - 0xFD)];
                std::mem::take(&mut timer.out)
            }
            _ => 0x00,
        }
    }

    fn write(&mut self, addr: u16, value: u8) {
        if (0x00F0..=0x00FF).contains(&addr) {
            self.write_io(addr, value);
        }
        // Writes always reach RAM, also under the I/O ports and the boot ROM.
        self.ram[usize::from(addr)] = value;
    }

    fn write_io(&mut self, addr: u16, value: u8) {
        match addr {
            0xF1 => {
                for (n, timer) in self.timers.iter_mut().enumerate() {
                    let enable = value & (1 << n) != 0;
                    if enable && !timer.enabled {
                        timer.stage = 0;
                        timer.out = 0;
                    }
                    timer.enabled = enable;
                }
                if value & 0x10 != 0 {
                    self.input[0] = 0;
                    self.input[1] = 0;
                }
                if value & 0x20 != 0 {
                    self.input[2] = 0;
                    self.input[3] = 0;
                }
                self.rom_enabled = value & 0x80 != 0;
            }
            0xF2 => self.dsp_addr = value,
            0xF3 => {
                if self.dsp_addr < 0x80 {
                    self.dsp.write(self.dsp_addr, value);
                }
            }
            0xF4..=0xF7 => self.output[usize::from(addr - 0xF4)] = value,
            0xFA..=0xFC => self.timers[usize::from(addr - 0xFA)].divider = value,
            _ => {}
        }
    }

    fn read16(&mut self, addr: u16) -> u16 {
        let lo = self.read(addr);
        let hi = self.read(addr.wrapping_add(1));
        u16::from_le_bytes([lo, hi])
    }

    /// A word in the direct page: the high byte wraps within the page.
    fn read_dp16(&mut self, dp: u8) -> u16 {
        let lo = self.read(self.dp(dp));
        let hi = self.read(self.dp(dp.wrapping_add(1)));
        u16::from_le_bytes([lo, hi])
    }

    fn write_dp16(&mut self, dp: u8, value: u16) {
        let [lo, hi] = value.to_le_bytes();
        let a = self.dp(dp);
        self.write(a, lo);
        let a = self.dp(dp.wrapping_add(1));
        self.write(a, hi);
    }

    fn dp(&self, offset: u8) -> u16 {
        (if self.psw & P != 0 { 0x100 } else { 0 }) | u16::from(offset)
    }

    fn fetch(&mut self) -> u8 {
        let b = self.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        b
    }

    fn fetch16(&mut self) -> u16 {
        let lo = self.fetch();
        let hi = self.fetch();
        u16::from_le_bytes([lo, hi])
    }

    fn push(&mut self, value: u8) {
        self.write(0x100 | u16::from(self.sp), value);
        self.sp = self.sp.wrapping_sub(1);
    }

    fn pop(&mut self) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        self.read(0x100 | u16::from(self.sp))
    }

    fn push16(&mut self, value: u16) {
        let [lo, hi] = value.to_le_bytes();
        self.push(hi);
        self.push(lo);
    }

    fn pop16(&mut self) -> u16 {
        let lo = self.pop();
        let hi = self.pop();
        u16::from_le_bytes([lo, hi])
    }

    // ---- flags -------------------------------------------------------------------------

    fn set(&mut self, flag: u8, on: bool) {
        if on {
            self.psw |= flag;
        } else {
            self.psw &= !flag;
        }
    }

    fn nz(&mut self, value: u8) -> u8 {
        self.set(N, value & 0x80 != 0);
        self.set(Z, value == 0);
        value
    }

    fn nz16(&mut self, value: u16) {
        self.set(N, value & 0x8000 != 0);
        self.set(Z, value == 0);
    }

    fn ya(&self) -> u16 {
        u16::from_le_bytes([self.a, self.y])
    }

    fn set_ya(&mut self, value: u16) {
        [self.a, self.y] = value.to_le_bytes();
    }

    // ---- ALU ---------------------------------------------------------------------------

    fn adc(&mut self, a: u8, b: u8) -> u8 {
        let carry = u16::from(self.psw & C);
        let r = u16::from(a) + u16::from(b) + carry;
        let r8 = r as u8;
        self.set(C, r > 0xFF);
        self.set(H, (a ^ b ^ r8) & 0x10 != 0);
        self.set(V, !(a ^ b) & (a ^ r8) & 0x80 != 0);
        self.nz(r8)
    }

    fn sbc(&mut self, a: u8, b: u8) -> u8 {
        self.adc(a, !b)
    }

    fn cmp(&mut self, a: u8, b: u8) {
        let r = a.wrapping_sub(b);
        self.set(C, a >= b);
        self.nz(r);
    }

    /// The eight-way ALU group: OR, AND, EOR, CMP, ADC, SBC (by opcode bits 5–7).
    fn alu(&mut self, op: u8, a: u8, b: u8) -> Option<u8> {
        match op >> 5 {
            0 => Some(self.nz(a | b)),
            1 => Some(self.nz(a & b)),
            2 => Some(self.nz(a ^ b)),
            3 => {
                self.cmp(a, b);
                None
            }
            4 => Some(self.adc(a, b)),
            _ => Some(self.sbc(a, b)),
        }
    }

    /// The shift/rotate/inc/dec group: ASL, ROL, LSR, ROR, DEC, INC (by bits 5–7).
    fn shift(&mut self, op: u8, v: u8) -> u8 {
        let carry = self.psw & C;
        let r = match op >> 5 {
            0 => {
                self.set(C, v & 0x80 != 0);
                v << 1
            }
            1 => {
                self.set(C, v & 0x80 != 0);
                (v << 1) | carry
            }
            2 => {
                self.set(C, v & 1 != 0);
                v >> 1
            }
            3 => {
                self.set(C, v & 1 != 0);
                (v >> 1) | (carry << 7)
            }
            4 => v.wrapping_sub(1),
            _ => v.wrapping_add(1),
        };
        self.nz(r)
    }

    fn branch(&mut self, taken: bool, cycles: &mut u32) {
        let offset = self.fetch() as i8;
        if taken {
            self.pc = self.pc.wrapping_add_signed(i16::from(offset));
            *cycles += 2;
        }
    }

    /// Address and bit of the `aaa.b` operand of the one-bit opcodes.
    fn mem_bit(&mut self) -> (u16, u8) {
        let w = self.fetch16();
        (w & 0x1FFF, (w >> 13) as u8)
    }

    // ---- execution ---------------------------------------------------------------------

    /// Runs one instruction; returns its cycles.
    fn step(&mut self) -> u32 {
        if self.rom_enabled && self.pc >= 0xFFC0 {
            // Back in the boot ROM: wait for the next upload.
            self.halted = true;
            return 2;
        }
        let op = self.fetch();
        let mut cycles = u32::from(CYCLES[usize::from(op)]);
        let lo = op & 0x1F;
        match op {
            // --- the regular ALU block: op & 0x1F selects the operand, bits 5-7 the op.
            _ if op < 0xC0 && matches!(lo, 0x04..=0x09 | 0x14..=0x19) => {
                self.alu_block(op);
            }
            // --- shifts / inc / dec on memory and A
            _ if op < 0xC0 && matches!(lo, 0x0B | 0x0C | 0x1B | 0x1C) => {
                self.shift_block(op);
            }
            // --- TCALL n (x1), SET1/CLR1 (x2/x12), BBS/BBC (x3/x13)
            _ if lo == 0x01 || lo == 0x11 => {
                let n = u16::from(op >> 4);
                let target = self.read16(0xFFDE - 2 * n);
                self.push16(self.pc);
                self.pc = target;
            }
            _ if lo == 0x02 || lo == 0x12 => {
                let dp = self.fetch();
                let addr = self.dp(dp);
                let bit = 1 << (op >> 5);
                let v = self.read(addr);
                let v = if lo == 0x02 { v | bit } else { v & !bit };
                self.write(addr, v);
            }
            _ if lo == 0x03 || lo == 0x13 => {
                let dp = self.fetch();
                let addr = self.dp(dp);
                let v = self.read(addr);
                let set = v & (1 << (op >> 5)) != 0;
                self.branch(set == (lo == 0x03), &mut cycles);
            }
            // --- conditional branches (x0 with bit 4 set)
            0x10 => self.branch(self.psw & N == 0, &mut cycles),
            0x30 => self.branch(self.psw & N != 0, &mut cycles),
            0x50 => self.branch(self.psw & V == 0, &mut cycles),
            0x70 => self.branch(self.psw & V != 0, &mut cycles),
            0x90 => self.branch(self.psw & C == 0, &mut cycles),
            0xB0 => self.branch(self.psw & C != 0, &mut cycles),
            0xD0 => self.branch(self.psw & Z == 0, &mut cycles),
            0xF0 => self.branch(self.psw & Z != 0, &mut cycles),
            0x2F => self.branch(true, &mut cycles),
            // --- flag and misc
            0x00 => {}
            0x20 => self.psw &= !P,
            0x40 => self.psw |= P,
            0x60 => self.psw &= !C,
            0x80 => self.psw |= C,
            0xA0 => self.psw |= I,
            0xC0 => self.psw &= !I,
            0xE0 => self.psw &= !(V | H),
            0xED => self.psw ^= C,
            0xEF | 0xFF => self.halted = true,
            // --- one-bit operations on aaa.b
            0x0A | 0x2A | 0x4A | 0x6A | 0x8A | 0xAA => {
                let (addr, bit) = self.mem_bit();
                let m = self.read(addr) >> bit & 1 != 0;
                let c = self.psw & C != 0;
                let r = match op {
                    0x0A => c | m,
                    0x2A => c | !m,
                    0x4A => c & m,
                    0x6A => c & !m,
                    0x8A => c ^ m,
                    _ => m,
                };
                self.set(C, r);
            }
            0xCA => {
                let (addr, bit) = self.mem_bit();
                let v = self.read(addr);
                let v = (v & !(1 << bit)) | ((self.psw & C) << bit);
                self.write(addr, v);
            }
            0xEA => {
                let (addr, bit) = self.mem_bit();
                let v = self.read(addr) ^ (1 << bit);
                self.write(addr, v);
            }
            // --- push / pop
            0x2D => self.push(self.a),
            0x4D => self.push(self.x),
            0x6D => self.push(self.y),
            0x0D => self.push(self.psw),
            0xAE => self.a = self.pop(),
            0xCE => self.x = self.pop(),
            0xEE => self.y = self.pop(),
            0x8E => self.psw = self.pop(),
            // --- TSET1 / TCLR1
            0x0E | 0x4E => {
                let addr = self.fetch16();
                let v = self.read(addr);
                let r = self.a.wrapping_sub(v);
                self.nz(r);
                let v = if op == 0x0E { v | self.a } else { v & !self.a };
                self.write(addr, v);
            }
            // --- compares with X and Y
            0xC8 => {
                let v = self.fetch();
                self.cmp(self.x, v);
            }
            0x3E => {
                let dp = self.fetch();
                let v = self.read(self.dp(dp));
                self.cmp(self.x, v);
            }
            0x1E => {
                let addr = self.fetch16();
                let v = self.read(addr);
                self.cmp(self.x, v);
            }
            0xAD => {
                let v = self.fetch();
                self.cmp(self.y, v);
            }
            0x7E => {
                let dp = self.fetch();
                let v = self.read(self.dp(dp));
                self.cmp(self.y, v);
            }
            0x5E => {
                let addr = self.fetch16();
                let v = self.read(addr);
                self.cmp(self.y, v);
            }
            // --- 16-bit
            0xBA => {
                let dp = self.fetch();
                let v = self.read_dp16(dp);
                self.set_ya(v);
                self.nz16(v);
            }
            0xDA => {
                let dp = self.fetch();
                self.write_dp16(dp, self.ya());
            }
            0x3A | 0x1A => {
                let dp = self.fetch();
                let v = self.read_dp16(dp);
                let r = if op == 0x3A {
                    v.wrapping_add(1)
                } else {
                    v.wrapping_sub(1)
                };
                self.write_dp16(dp, r);
                self.nz16(r);
            }
            0x7A | 0x9A => {
                let dp = self.fetch();
                let w = self.read_dp16(dp);
                let ya = self.ya();
                let r = if op == 0x7A {
                    let (r, carry) = ya.overflowing_add(w);
                    self.set(C, carry);
                    self.set(V, !(ya ^ w) & (ya ^ r) & 0x8000 != 0);
                    self.set(H, (ya ^ w ^ r) & 0x1000 != 0);
                    r
                } else {
                    let r = ya.wrapping_sub(w);
                    self.set(C, ya >= w);
                    self.set(V, (ya ^ w) & (ya ^ r) & 0x8000 != 0);
                    self.set(H, (ya ^ w ^ r) & 0x1000 == 0);
                    r
                };
                self.set_ya(r);
                self.nz16(r);
            }
            0x5A => {
                let dp = self.fetch();
                let w = self.read_dp16(dp);
                let ya = self.ya();
                self.set(C, ya >= w);
                self.nz16(ya.wrapping_sub(w));
            }
            0xCF => {
                let r = u16::from(self.y) * u16::from(self.a);
                self.set_ya(r);
                self.nz(self.y);
            }
            0x9E => {
                let ya = u32::from(self.ya());
                let x = u32::from(self.x);
                self.set(V, self.y >= self.x);
                self.set(H, (self.y & 0x0F) >= (self.x & 0x0F));
                if u32::from(self.y) < (x << 1) {
                    self.a = (ya / x) as u8;
                    self.y = (ya % x) as u8;
                } else {
                    let rest = ya - (x << 9);
                    self.a = (255 - rest / (256 - x)) as u8;
                    self.y = (x + rest % (256 - x)) as u8;
                }
                self.nz(self.a);
            }
            // --- decimal adjust, nibble swap
            0xDF => {
                if self.psw & C != 0 || self.a > 0x99 {
                    self.a = self.a.wrapping_add(0x60);
                    self.psw |= C;
                }
                if self.psw & H != 0 || self.a & 0x0F > 9 {
                    self.a = self.a.wrapping_add(6);
                }
                self.nz(self.a);
            }
            0xBE => {
                if self.psw & C == 0 || self.a > 0x99 {
                    self.a = self.a.wrapping_sub(0x60);
                    self.psw &= !C;
                }
                if self.psw & H == 0 || self.a & 0x0F > 9 {
                    self.a = self.a.wrapping_sub(6);
                }
                self.nz(self.a);
            }
            0x9F => {
                self.a = self.a.rotate_left(4);
                self.nz(self.a);
            }
            // --- jumps and calls
            0x5F => self.pc = self.fetch16(),
            0x1F => {
                let base = self.fetch16();
                self.pc = self.read16(base.wrapping_add(u16::from(self.x)));
            }
            0x3F => {
                let target = self.fetch16();
                self.push16(self.pc);
                self.pc = target;
            }
            0x4F => {
                let low = self.fetch();
                self.push16(self.pc);
                self.pc = 0xFF00 | u16::from(low);
            }
            0x6F => self.pc = self.pop16(),
            0x7F => {
                self.psw = self.pop();
                self.pc = self.pop16();
            }
            0x0F => {
                self.push16(self.pc);
                self.push(self.psw);
                self.psw = (self.psw | B) & !I;
                self.pc = self.read16(0xFFDE);
            }
            // --- compare-and-branch, decrement-and-branch
            0x2E | 0xDE => {
                let dp = self.fetch();
                let dp = if op == 0xDE {
                    dp.wrapping_add(self.x)
                } else {
                    dp
                };
                let v = self.read(self.dp(dp));
                self.branch(self.a != v, &mut cycles);
            }
            0x6E => {
                let dp = self.fetch();
                let addr = self.dp(dp);
                let v = self.read(addr).wrapping_sub(1);
                self.write(addr, v);
                self.branch(v != 0, &mut cycles);
            }
            0xFE => {
                self.y = self.y.wrapping_sub(1);
                self.branch(self.y != 0, &mut cycles);
            }
            // --- register moves
            0xE8 => {
                let v = self.fetch();
                self.a = self.nz(v);
            }
            0xCD => {
                let v = self.fetch();
                self.x = self.nz(v);
            }
            0x8D => {
                let v = self.fetch();
                self.y = self.nz(v);
            }
            0x7D => self.a = self.nz(self.x),
            0x5D => self.x = self.nz(self.a),
            0xDD => self.a = self.nz(self.y),
            0xFD => self.y = self.nz(self.a),
            0x9D => self.x = self.nz(self.sp),
            0xBD => self.sp = self.x,
            // --- inc/dec X and Y
            0x1D => self.x = self.nz(self.x.wrapping_sub(1)),
            0x3D => self.x = self.nz(self.x.wrapping_add(1)),
            0xDC => self.y = self.nz(self.y.wrapping_sub(1)),
            0xFC => self.y = self.nz(self.y.wrapping_add(1)),
            // --- loads
            0xE4 | 0xF4 | 0xE5 | 0xF5 | 0xF6 | 0xE6 | 0xBF | 0xF7 | 0xE7 => {
                let addr = match op {
                    0xE4 => {
                        let dp = self.fetch();
                        self.dp(dp)
                    }
                    0xF4 => {
                        let dp = self.fetch().wrapping_add(self.x);
                        self.dp(dp)
                    }
                    0xE5 => self.fetch16(),
                    0xF5 => self.fetch16().wrapping_add(u16::from(self.x)),
                    0xF6 => self.fetch16().wrapping_add(u16::from(self.y)),
                    0xE6 | 0xBF => self.dp(self.x),
                    0xF7 => {
                        let dp = self.fetch();
                        self.read_dp16(dp).wrapping_add(u16::from(self.y))
                    }
                    _ => {
                        let dp = self.fetch().wrapping_add(self.x);
                        self.read_dp16(dp)
                    }
                };
                let v = self.read(addr);
                self.a = self.nz(v);
                if op == 0xBF {
                    self.x = self.x.wrapping_add(1);
                }
            }
            0xF8 | 0xF9 | 0xE9 => {
                let addr = match op {
                    0xF8 => {
                        let dp = self.fetch();
                        self.dp(dp)
                    }
                    0xF9 => {
                        let dp = self.fetch().wrapping_add(self.y);
                        self.dp(dp)
                    }
                    _ => self.fetch16(),
                };
                let v = self.read(addr);
                self.x = self.nz(v);
            }
            0xEB | 0xFB | 0xEC => {
                let addr = match op {
                    0xEB => {
                        let dp = self.fetch();
                        self.dp(dp)
                    }
                    0xFB => {
                        let dp = self.fetch().wrapping_add(self.x);
                        self.dp(dp)
                    }
                    _ => self.fetch16(),
                };
                let v = self.read(addr);
                self.y = self.nz(v);
            }
            // --- stores
            0x8F => {
                let v = self.fetch();
                let dp = self.fetch();
                self.write(self.dp(dp), v);
            }
            0xFA => {
                let src = self.fetch();
                let v = self.read(self.dp(src));
                let dst = self.fetch();
                self.write(self.dp(dst), v);
            }
            0xC4 | 0xD4 | 0xC5 | 0xD5 | 0xD6 | 0xC6 | 0xAF | 0xD7 | 0xC7 => {
                let addr = match op {
                    0xC4 => {
                        let dp = self.fetch();
                        self.dp(dp)
                    }
                    0xD4 => {
                        let dp = self.fetch().wrapping_add(self.x);
                        self.dp(dp)
                    }
                    0xC5 => self.fetch16(),
                    0xD5 => self.fetch16().wrapping_add(u16::from(self.x)),
                    0xD6 => self.fetch16().wrapping_add(u16::from(self.y)),
                    0xC6 | 0xAF => self.dp(self.x),
                    0xD7 => {
                        let dp = self.fetch();
                        self.read_dp16(dp).wrapping_add(u16::from(self.y))
                    }
                    _ => {
                        let dp = self.fetch().wrapping_add(self.x);
                        self.read_dp16(dp)
                    }
                };
                self.write(addr, self.a);
                if op == 0xAF {
                    self.x = self.x.wrapping_add(1);
                }
            }
            0xD8 | 0xD9 | 0xC9 => {
                let addr = match op {
                    0xD8 => {
                        let dp = self.fetch();
                        self.dp(dp)
                    }
                    0xD9 => {
                        let dp = self.fetch().wrapping_add(self.y);
                        self.dp(dp)
                    }
                    _ => self.fetch16(),
                };
                self.write(addr, self.x);
            }
            0xCB | 0xDB | 0xCC => {
                let addr = match op {
                    0xCB => {
                        let dp = self.fetch();
                        self.dp(dp)
                    }
                    0xDB => {
                        let dp = self.fetch().wrapping_add(self.x);
                        self.dp(dp)
                    }
                    _ => self.fetch16(),
                };
                self.write(addr, self.y);
            }
            // Every opcode is covered above; this keeps the match exhaustive.
            _ => {}
        }
        cycles
    }

    /// OR/AND/EOR/CMP/ADC/SBC with the operand forms x4–x9 and x14–x19.
    fn alu_block(&mut self, op: u8) {
        match op & 0x1F {
            // A, operand
            0x04..=0x08 | 0x14..=0x17 => {
                let v = match op & 0x1F {
                    0x08 => self.fetch(),
                    0x06 => self.read(self.dp(self.x)),
                    0x04 => {
                        let dp = self.fetch();
                        self.read(self.dp(dp))
                    }
                    0x14 => {
                        let dp = self.fetch().wrapping_add(self.x);
                        self.read(self.dp(dp))
                    }
                    0x05 => {
                        let addr = self.fetch16();
                        self.read(addr)
                    }
                    0x15 => {
                        let addr = self.fetch16().wrapping_add(u16::from(self.x));
                        self.read(addr)
                    }
                    0x16 => {
                        let addr = self.fetch16().wrapping_add(u16::from(self.y));
                        self.read(addr)
                    }
                    0x17 => {
                        let dp = self.fetch();
                        let addr = self.read_dp16(dp).wrapping_add(u16::from(self.y));
                        self.read(addr)
                    }
                    _ => {
                        // 0x07: [aa+X]
                        let dp = self.fetch().wrapping_add(self.x);
                        let addr = self.read_dp16(dp);
                        self.read(addr)
                    }
                };
                if let Some(r) = self.alu(op, self.a, v) {
                    self.a = r;
                }
            }
            // [aa],[bb]  /  [aa],#nn  /  [X],[Y]
            0x09 | 0x18 | 0x19 => {
                let (src, dst) = match op & 0x1F {
                    0x09 => {
                        let bb = self.fetch();
                        let s = self.read(self.dp(bb));
                        let aa = self.fetch();
                        (s, self.dp(aa))
                    }
                    0x18 => {
                        let nn = self.fetch();
                        let aa = self.fetch();
                        (nn, self.dp(aa))
                    }
                    _ => {
                        let s = self.read(self.dp(self.y));
                        (s, self.dp(self.x))
                    }
                };
                let d = self.read(dst);
                if let Some(r) = self.alu(op, d, src) {
                    self.write(dst, r);
                }
            }
            _ => {}
        }
    }

    /// ASL/ROL/LSR/ROR/DEC/INC on [aa], [aa+X], [aaaa] and A.
    fn shift_block(&mut self, op: u8) {
        match op & 0x1F {
            0x1C => {
                self.a = self.shift(op, self.a);
            }
            _ => {
                let addr = match op & 0x1F {
                    0x0B => {
                        let dp = self.fetch();
                        self.dp(dp)
                    }
                    0x1B => {
                        let dp = self.fetch().wrapping_add(self.x);
                        self.dp(dp)
                    }
                    _ => self.fetch16(),
                };
                let v = self.read(addr);
                let r = self.shift(op, v);
                self.write(addr, r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_program(code: &[u8]) -> Smp {
        let mut smp = Smp::new();
        smp.upload(0x0400, code);
        smp.start(0x0400);
        let mut out = Vec::new();
        smp.run(2000, &mut out);
        smp
    }

    #[test]
    fn loads_alu_and_stores() {
        // MOV A,#$12; ADC A,#$34 (C=0); MOV $20,A; MOV X,#$05; MOV Y,#$03; MUL YA;
        // MOV $21,A; STOP
        let smp = run_program(&[
            0xE8, 0x12, 0x88, 0x34, 0xC4, 0x20, 0xCD, 0x05, 0x8D, 0x03, 0xCF, 0xC4, 0x21, 0xFF,
        ]);
        assert_eq!(smp.ram[0x20], 0x46);
        assert_eq!(smp.ram[0x21], 0xD2, "YA = 3 × $46 = $00D2");
    }

    #[test]
    fn calls_branches_and_loops() {
        // MOV Y,#$05; MOV A,#$00; loop: CLRC; ADC A,#$02; DBNZ Y,loop; CALL $0420; STOP
        // $0420: MOV $30,A; RET
        let mut code = vec![
            0x8D, 0x05, 0xE8, 0x00, 0x60, 0x88, 0x02, 0xFE, 0xFB, 0x3F, 0x20, 0x04, 0xFF,
        ];
        code.resize(0x20, 0);
        code.extend([0xC4, 0x30, 0x6F]);
        let smp = run_program(&code);
        assert_eq!(smp.ram[0x30], 10);
    }

    #[test]
    fn division_and_words() {
        // MOV A,#$E8; MOV Y,#$03 (YA=1000); MOV X,#$07; DIV YA,X; MOV $40,A; MOV $41,Y;
        // MOVW $42,YA ... ; STOP
        let smp = run_program(&[
            0xE8, 0xE8, 0x8D, 0x03, 0xCD, 0x07, 0x9E, 0xC4, 0x40, 0xCB, 0x41, 0xFF,
        ]);
        assert_eq!(
            (smp.ram[0x40], smp.ram[0x41]),
            (142, 6),
            "1000 / 7 = 142 rest 6"
        );
    }

    #[test]
    fn timers_count_at_8khz() {
        // MOV $FA,#$10 (divide by 16: 500 Hz); MOV $F1,#$01; loop: BRA loop
        let mut smp = Smp::new();
        smp.upload(0x0400, &[0x8F, 0x10, 0xFA, 0x8F, 0x01, 0xF1, 0x2F, 0xFE]);
        smp.start(0x0400);
        let mut out = Vec::new();
        smp.run(CLOCK_HZ / 100, &mut out); // 10 ms: 5 ticks
        assert_eq!(smp.read(0xFD), 5);
        assert_eq!(smp.read(0xFD), 0, "reading clears the counter");
        assert_eq!(out.len(), 320, "32 kHz samples");
    }

    #[test]
    fn jumping_into_the_boot_rom_waits_for_an_upload() {
        let mut smp = Smp::new();
        smp.upload(0x0400, &[0x5F, 0xC0, 0xFF]); // JMP $FFC0
        smp.start(0x0400);
        smp.write(0xF1, 0x80); // ROM on
        let mut out = Vec::new();
        smp.run(100, &mut out);
        assert!(smp.halted());
    }
}
