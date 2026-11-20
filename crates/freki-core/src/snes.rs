//! The SNES side of a Super Game Boy, emulated as hardware: the 65816 ([`crate::w65816`])
//! runs the SGB cartridge's SNES program from its ROM, with the SNES's memory map, timing,
//! DMA/HDMA, interrupts, multiplier/divider and joypad auto-read, the picture processor
//! ([`crate::snes_ppu`]), the sound chip ([`crate::spc700`]) behind the four APU ports, and
//! the ICD2 ([`crate::icd2`]) that links it with the Game Boy. After nocash's "fullsnes".
//!
//! The APU's 64-byte boot ROM is Nintendo's and not included: its upload protocol is done
//! here instead ([`Ipl`]), with the same behaviour at the ports.

use serde::{Deserialize, Serialize};

use crate::icd2::Icd2;
use crate::snes_ppu::SnesPpu;
use crate::spc700::{self, Smp};
use crate::w65816::{Bus65816, W65816};

/// NTSC master clock.
pub const MASTER_HZ: u64 = 21_477_272;
/// Master clocks per scanline and scanlines per frame.
pub const LINE_CLOCKS: u32 = 1364;
pub const LINES: u16 = 262;
/// The Game Boy runs at the master clock divided by this ($6003 speed 1, the default).
pub const GB_DIVIDER: u32 = 5;

/// The APU boot protocol, done for the boot ROM (fullsnes, "SNES APU Main CPU
/// Communication Port").
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
enum Ipl {
    /// Waiting for $CC in port 0 (ports 0/1 show $AA/$BB).
    WaitKick,
    /// Receiving a block: the next byte goes to `addr + index`.
    Transfer { addr: u16, index: u8 },
    /// The uploaded program runs.
    Running,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default)]
struct DmaChannel {
    /// $43x0 parameters, $43x1 B-bus address.
    control: u8,
    b_addr: u8,
    /// A-bus address and bank ($43x2–$43x4).
    a_addr: u16,
    a_bank: u8,
    /// Byte count / HDMA indirect address ($43x5–$43x6), indirect bank ($43x7).
    count: u16,
    ind_bank: u8,
    /// HDMA table address ($43x8–$43x9) and line counter ($43xA).
    table: u16,
    line_counter: u8,
    unused: u8,
    /// HDMA: still running this frame, and transferring on this line.
    hdma_active: bool,
    hdma_do: bool,
}

/// Everything the 65816 sees.
pub struct SnesBus {
    pub rom: Vec<u8>,
    pub wram: Vec<u8>,
    pub ppu: SnesPpu,
    pub icd2: Icd2,
    pub smp: Smp,
    ipl: Ipl,
    /// Master clocks since power-on, and the position in the frame.
    pub clock: u64,
    pub h: u32,
    pub v: u16,
    /// Master clocks the APU has not caught up with, in SPC700 clock × master units.
    apu_debt: u64,
    pub audio: Vec<(i16, i16)>,
    open_bus: u8,
    nmitimen: u8,
    nmi_flag: bool,
    nmi_line: bool,
    irq_flag: bool,
    htime: u16,
    vtime: u16,
    wrmpya: u8,
    wrdiv: u16,
    rddiv: u16,
    rdmpy: u16,
    memsel: u8,
    wrio: u8,
    wram_addr: u32,
    dma: [DmaChannel; 8],
    hdma_enable: u8,
    /// $4218–$421F: joypads 1–4 as auto-read; `pads` are the SNES joypads' current
    /// buttons (SNES order, bit 15 = B).
    joy_auto: [u16; 4],
    pub pads: [u16; 4],
    joy_latch: bool,
    joy_shift: [u16; 2],
    /// A frame was finished (VBlank started).
    pub frame_done: bool,
}

impl SnesBus {
    fn new(rom: Vec<u8>) -> Self {
        let mut smp = Smp::new();
        // The boot ROM's "ready" signal.
        smp.output = [0xAA, 0xBB, 0, 0];
        SnesBus {
            rom,
            wram: vec![0; 0x20000],
            ppu: SnesPpu::new(),
            icd2: Icd2::new(),
            smp,
            ipl: Ipl::WaitKick,
            clock: 0,
            h: 0,
            v: 0,
            apu_debt: 0,
            audio: Vec::new(),
            open_bus: 0,
            nmitimen: 0,
            nmi_flag: false,
            nmi_line: false,
            irq_flag: false,
            htime: 0x1FF,
            vtime: 0x1FF,
            wrmpya: 0xFF,
            wrdiv: 0xFFFF,
            rddiv: 0,
            rdmpy: 0,
            memsel: 0,
            wrio: 0xFF,
            wram_addr: 0,
            dma: [DmaChannel::default(); 8],
            hdma_enable: 0,
            joy_auto: [0; 4],
            pads: [0; 4],
            joy_latch: false,
            joy_shift: [0; 2],
            frame_done: false,
        }
    }

    fn rom_byte(&self, bank: u8, addr: u16) -> u8 {
        if self.rom.is_empty() {
            return self.open_bus;
        }
        let offset = ((bank & 0x7F) as usize) << 15 | (addr & 0x7FFF) as usize;
        self.rom[offset % self.rom.len()]
    }

    /// Master clocks of an access to `addr`.
    fn speed(&self, addr: u32) -> u32 {
        let bank = (addr >> 16) as u8;
        let a = addr as u16;
        if bank & 0x40 == 0 {
            match a {
                0x0000..=0x1FFF => 8,
                0x2000..=0x3FFF => 6,
                0x4000..=0x41FF => 12,
                0x4200..=0x5FFF => 6,
                0x6000..=0x7FFF => 8,
                _ if bank & 0x80 != 0 && self.memsel & 1 != 0 => 6,
                _ => 8,
            }
        } else if bank >= 0xC0 && self.memsel & 1 != 0 {
            6
        } else {
            8
        }
    }

    /// Runs the rest of the machine for `clocks` master clocks.
    fn advance(&mut self, clocks: u32) {
        self.clock += clocks as u64;
        self.apu_debt += clocks as u64 * spc700::CLOCK_HZ as u64;
        let mut left = clocks;
        while left > 0 {
            // Step to the next point of interest in the line.
            let next = [HDMA_H, LINE_CLOCKS]
                .into_iter()
                .chain((self.nmitimen & 0x10 != 0).then_some(self.htime as u32 * 4))
                .filter(|&p| p > self.h)
                .min()
                .unwrap_or(LINE_CLOCKS);
            let step = left.min(next - self.h);
            let before = self.h;
            self.h += step;
            left -= step;
            self.check_irq(before, self.h);
            if before < HDMA_H && self.h >= HDMA_H && self.v < 225 {
                self.hdma_line();
            }
            if self.h >= LINE_CLOCKS {
                self.h -= LINE_CLOCKS;
                self.new_line();
            }
        }
    }

    fn check_irq(&mut self, from: u32, to: u32) {
        let mode = (self.nmitimen >> 4) & 3;
        let hit = match mode {
            0 => false,
            // H-IRQ: every line at HTIME.
            1 => (from..to).contains(&(self.htime as u32 * 4)) || (from == 0 && self.htime == 0),
            // V-IRQ: at the start of line VTIME.
            2 => self.v == self.vtime && from == 0,
            _ => self.v == self.vtime && (from..to).contains(&(self.htime as u32 * 4)),
        };
        if hit {
            self.irq_flag = true;
        }
    }

    fn new_line(&mut self) {
        self.v += 1;
        if self.v == LINES {
            self.v = 0;
            self.nmi_flag = false;
            self.hdma_init();
        }
        if self.v == 225 {
            self.nmi_flag = true;
            if self.nmitimen & 0x80 != 0 {
                self.nmi_line = true;
            }
            self.ppu.vblank_start();
            self.frame_done = true;
            if self.nmitimen & 1 != 0 {
                self.auto_joypad();
            }
        }
        if self.nmitimen & 0x30 == 0x20 && self.v == self.vtime {
            self.irq_flag = true;
        }
    }

    /// Joypad auto-read: latches the joypads and shifts all 16 bits of each out, so later
    /// manual reads see what follows (1s from a standard joypad). Only port 1 has one.
    fn auto_joypad(&mut self) {
        self.joy_auto = [self.pads[0], 0, 0, 0];
        self.joy_shift = [0xFFFF, 0];
    }

    /// Catches the sound chip up with the CPU.
    fn sync_apu(&mut self) {
        let cycles = (self.apu_debt / MASTER_HZ) as u32;
        if cycles == 0 {
            return;
        }
        self.apu_debt -= cycles as u64 * MASTER_HZ;
        self.ipl_poll();
        if self.ipl == Ipl::Running {
            self.smp.run(cycles, &mut self.audio);
            if self.smp.halted() {
                // The program jumped back into the boot ROM.
                self.ipl = Ipl::WaitKick;
                self.smp.output[0] = 0xAA;
                self.smp.output[1] = 0xBB;
            }
        } else {
            // While in the boot ROM the DSP keeps running (silence or the echo).
            self.smp.run(cycles, &mut self.audio);
        }
    }

    /// The boot ROM's side of the upload protocol, after a port write.
    fn ipl_poll(&mut self) {
        let input = self.smp.input;
        match self.ipl {
            Ipl::WaitKick => {
                self.smp.output[0] = 0xAA;
                self.smp.output[1] = 0xBB;
                if input[0] == 0xCC {
                    self.ipl_command();
                }
            }
            Ipl::Transfer { addr, index } => {
                let diff = input[0].wrapping_sub(index) as i8;
                if diff == 0 {
                    self.smp.ram[addr.wrapping_add(index as u16) as usize] = input[1];
                    self.smp.output[0] = index;
                    self.ipl = Ipl::Transfer {
                        addr: if index == 0xFF {
                            addr.wrapping_add(0x100)
                        } else {
                            addr
                        },
                        index: index.wrapping_add(1),
                    };
                } else if diff > 0 {
                    self.ipl_command();
                }
            }
            Ipl::Running => {}
        }
    }

    fn ipl_command(&mut self) {
        let input = self.smp.input;
        let addr = u16::from_le_bytes([input[2], input[3]]);
        self.smp.output[0] = input[0];
        if input[1] == 0 {
            self.smp.jump(addr);
            self.ipl = Ipl::Running;
        } else {
            self.ipl = Ipl::Transfer { addr, index: 0 };
        }
    }

    // ---- DMA ---------------------------------------------------------------------------

    fn b_bus_read(&mut self, reg: u8) -> u8 {
        self.read(0x2100 | reg as u32)
    }

    fn b_bus_write(&mut self, reg: u8, v: u8) {
        self.write_io(0x2100 | reg as u16, v);
    }

    /// Bytes of one transfer unit in each mode: B-bus register offsets.
    fn unit(mode: u8) -> &'static [u8] {
        match mode & 7 {
            0 => &[0],
            1 => &[0, 1],
            2 | 6 => &[0, 0],
            3 | 7 => &[0, 0, 1, 1],
            4 => &[0, 1, 2, 3],
            _ => &[0, 1, 0, 1],
        }
    }

    fn general_dma(&mut self, channels: u8) {
        self.advance(8);
        for n in 0..8 {
            if channels & (1 << n) == 0 {
                continue;
            }
            self.advance(8);
            let mut i = 0;
            loop {
                let ch = self.dma[n];
                let unit = Self::unit(ch.control);
                let reg = ch.b_addr.wrapping_add(unit[i % unit.len()]);
                let a = (ch.a_bank as u32) << 16 | ch.a_addr as u32;
                if ch.control & 0x80 == 0 {
                    let v = self.read_a_bus(a);
                    self.b_bus_write(reg, v);
                } else {
                    let v = self.b_bus_read(reg);
                    self.write_a_bus(a, v);
                }
                self.advance(8);
                let ch = &mut self.dma[n];
                match (ch.control >> 3) & 3 {
                    0 => ch.a_addr = ch.a_addr.wrapping_add(1),
                    2 => ch.a_addr = ch.a_addr.wrapping_sub(1),
                    _ => {}
                }
                ch.count = ch.count.wrapping_sub(1);
                i += 1;
                if ch.count == 0 {
                    break;
                }
            }
        }
    }

    /// A-bus side of DMA: the B-bus registers and DMA registers are not reachable.
    fn read_a_bus(&mut self, a: u32) -> u8 {
        let low = a as u16;
        if a & 0x40_0000 == 0 && (low & 0xFF00 == 0x2100 || (0x4300..0x4380).contains(&low)) {
            return self.open_bus;
        }
        self.read(a)
    }

    fn write_a_bus(&mut self, a: u32, v: u8) {
        let low = a as u16;
        if a & 0x40_0000 == 0 && (low & 0xFF00 == 0x2100 || (0x4300..0x4380).contains(&low)) {
            return;
        }
        self.write(a, v);
    }

    fn hdma_init(&mut self) {
        for n in 0..8 {
            let ch = &mut self.dma[n];
            ch.hdma_do = false;
            ch.hdma_active = self.hdma_enable & (1 << n) != 0;
            if !ch.hdma_active {
                continue;
            }
            ch.table = ch.a_addr;
            self.hdma_load(n);
        }
    }

    /// Reads the next line counter (and indirect address) of channel `n`.
    fn hdma_load(&mut self, n: usize) {
        let ch = self.dma[n];
        let base = (ch.a_bank as u32) << 16;
        let counter = self.read(base | ch.table as u32);
        let mut table = ch.table.wrapping_add(1);
        let mut count = ch.count;
        if counter != 0 && ch.control & 0x40 != 0 {
            let lo = self.read(base | table as u32);
            let hi = self.read(base | table.wrapping_add(1) as u32);
            count = u16::from_le_bytes([lo, hi]);
            table = table.wrapping_add(2);
        }
        let ch = &mut self.dma[n];
        ch.line_counter = counter;
        ch.table = table;
        ch.count = count;
        ch.hdma_do = counter != 0;
        if counter == 0 {
            ch.hdma_active = false;
        }
    }

    fn hdma_line(&mut self) {
        if self.hdma_enable == 0 {
            return;
        }
        for n in 0..8 {
            let ch = self.dma[n];
            if !ch.hdma_active || self.hdma_enable & (1 << n) == 0 {
                continue;
            }
            if ch.hdma_do {
                let unit = Self::unit(ch.control);
                for &off in unit {
                    let ch = self.dma[n];
                    let reg = ch.b_addr.wrapping_add(off);
                    let v = if ch.control & 0x40 != 0 {
                        let v = self.read((ch.ind_bank as u32) << 16 | ch.count as u32);
                        self.dma[n].count = ch.count.wrapping_add(1);
                        v
                    } else {
                        let v = self.read((ch.a_bank as u32) << 16 | ch.table as u32);
                        self.dma[n].table = ch.table.wrapping_add(1);
                        v
                    };
                    self.b_bus_write(reg, v);
                }
            }
            let ch = &mut self.dma[n];
            ch.line_counter = ch.line_counter.wrapping_sub(1);
            ch.hdma_do = ch.line_counter & 0x80 != 0;
            if ch.line_counter & 0x7F == 0 {
                self.hdma_load(n);
            }
        }
    }

    // ---- registers -----------------------------------------------------------------------

    fn read_io(&mut self, a: u16) -> u8 {
        match a {
            0x2134..=0x213F => {
                let h = (self.h / 4) as u16;
                let v = self.v;
                self.ppu.read((a & 0xFF) as u8, h, v, self.open_bus)
            }
            0x2140..=0x217F => {
                self.sync_apu();
                self.smp.output[(a & 3) as usize]
            }
            0x2180 => {
                let v = self.wram[self.wram_addr as usize];
                self.wram_addr = (self.wram_addr + 1) & 0x1FFFF;
                v
            }
            0x4016 | 0x4017 => {
                // Port 1 has a standard joypad (1s after its 16 bits), port 2 nothing.
                let i = (a & 1) as usize;
                let bit = (self.joy_shift[i] >> 15) as u8;
                if !self.joy_latch {
                    self.joy_shift[i] = self.joy_shift[i] << 1 | u16::from(i == 0);
                }
                if i == 0 {
                    (self.open_bus & 0xFC) | bit
                } else {
                    (self.open_bus & 0xE0) | 0x1C | bit
                }
            }
            0x4210 => {
                let v = u8::from(self.nmi_flag) << 7 | 0x02 | (self.open_bus & 0x70);
                self.nmi_flag = false;
                v
            }
            0x4211 => {
                let v = u8::from(self.irq_flag) << 7 | (self.open_bus & 0x7F);
                self.irq_flag = false;
                v
            }
            0x4212 => {
                let vblank = self.v >= 225;
                let hblank = self.h < 4 || self.h >= 274 * 4;
                let auto = self.nmitimen & 1 != 0 && (225..228).contains(&self.v);
                u8::from(vblank) << 7
                    | u8::from(hblank) << 6
                    | u8::from(auto)
                    | (self.open_bus & 0x3E)
            }
            0x4213 => self.wrio,
            0x4214 => self.rddiv as u8,
            0x4215 => (self.rddiv >> 8) as u8,
            0x4216 => self.rdmpy as u8,
            0x4217 => (self.rdmpy >> 8) as u8,
            0x4218..=0x421F => {
                let pad = self.joy_auto[((a - 0x4218) / 2) as usize];
                if a & 1 == 0 {
                    pad as u8
                } else {
                    (pad >> 8) as u8
                }
            }
            0x4300..=0x437F => {
                let ch = &self.dma[((a >> 4) & 7) as usize];
                match a & 0x0F {
                    0x0 => ch.control,
                    0x1 => ch.b_addr,
                    0x2 => ch.a_addr as u8,
                    0x3 => (ch.a_addr >> 8) as u8,
                    0x4 => ch.a_bank,
                    0x5 => ch.count as u8,
                    0x6 => (ch.count >> 8) as u8,
                    0x7 => ch.ind_bank,
                    0x8 => ch.table as u8,
                    0x9 => (ch.table >> 8) as u8,
                    0xA => ch.line_counter,
                    _ => ch.unused,
                }
            }
            0x6000..=0x7FFF => self.icd2.read(a, self.open_bus),
            _ => self.open_bus,
        }
    }

    fn write_io(&mut self, a: u16, v: u8) {
        match a {
            0x2100..=0x2133 => self.ppu.write((a & 0xFF) as u8, v),
            0x2140..=0x217F => {
                self.sync_apu();
                self.smp.input[(a & 3) as usize] = v;
                self.ipl_poll();
            }
            0x2180 => {
                self.wram[self.wram_addr as usize] = v;
                self.wram_addr = (self.wram_addr + 1) & 0x1FFFF;
            }
            0x2181 => self.wram_addr = (self.wram_addr & 0x1FF00) | v as u32,
            0x2182 => self.wram_addr = (self.wram_addr & 0x100FF) | (v as u32) << 8,
            0x2183 => self.wram_addr = (self.wram_addr & 0x0FFFF) | ((v as u32 & 1) << 16),
            0x4016 => {
                let latch = v & 1 != 0;
                if latch {
                    self.joy_shift = [self.pads[0], 0];
                }
                self.joy_latch = latch;
            }
            0x4200 => {
                let enable_nmi = v & 0x80 != 0 && self.nmitimen & 0x80 == 0;
                self.nmitimen = v;
                if enable_nmi && self.nmi_flag {
                    self.nmi_line = true;
                }
                if v & 0x30 == 0 {
                    self.irq_flag = false;
                }
            }
            0x4201 => self.wrio = v,
            0x4202 => self.wrmpya = v,
            0x4203 => {
                self.rdmpy = self.wrmpya as u16 * v as u16;
                self.rddiv = v as u16;
            }
            0x4204 => self.wrdiv = (self.wrdiv & 0xFF00) | v as u16,
            0x4205 => self.wrdiv = (self.wrdiv & 0x00FF) | (v as u16) << 8,
            0x4206 => {
                if v == 0 {
                    self.rddiv = 0xFFFF;
                    self.rdmpy = self.wrdiv;
                } else {
                    self.rddiv = self.wrdiv / v as u16;
                    self.rdmpy = self.wrdiv % v as u16;
                }
            }
            0x4207 => self.htime = (self.htime & 0x100) | v as u16,
            0x4208 => self.htime = (self.htime & 0xFF) | (v as u16 & 1) << 8,
            0x4209 => self.vtime = (self.vtime & 0x100) | v as u16,
            0x420A => self.vtime = (self.vtime & 0xFF) | (v as u16 & 1) << 8,
            0x420B => self.general_dma(v),
            0x420C => self.hdma_enable = v,
            0x420D => self.memsel = v,
            0x4300..=0x437F => {
                let ch = &mut self.dma[((a >> 4) & 7) as usize];
                match a & 0x0F {
                    0x0 => ch.control = v,
                    0x1 => ch.b_addr = v,
                    0x2 => ch.a_addr = (ch.a_addr & 0xFF00) | v as u16,
                    0x3 => ch.a_addr = (ch.a_addr & 0x00FF) | (v as u16) << 8,
                    0x4 => ch.a_bank = v,
                    0x5 => ch.count = (ch.count & 0xFF00) | v as u16,
                    0x6 => ch.count = (ch.count & 0x00FF) | (v as u16) << 8,
                    0x7 => ch.ind_bank = v,
                    0x8 => ch.table = (ch.table & 0xFF00) | v as u16,
                    0x9 => ch.table = (ch.table & 0x00FF) | (v as u16) << 8,
                    0xA => ch.line_counter = v,
                    _ => ch.unused = v,
                }
            }
            0x6000..=0x7FFF => self.icd2.write(a, v),
            _ => {}
        }
    }
}

/// Where in a line HDMA transfers happen.
const HDMA_H: u32 = 1104;

impl Bus65816 for SnesBus {
    fn read(&mut self, addr: u32) -> u8 {
        let bank = (addr >> 16) as u8;
        let a = addr as u16;
        let v = if bank & 0xFE == 0x7E {
            self.wram[(addr & 0x1FFFF) as usize]
        } else if bank & 0x40 == 0 && a < 0x8000 {
            if a < 0x2000 {
                self.wram[a as usize]
            } else {
                self.read_io(a)
            }
        } else if a >= 0x8000 {
            self.rom_byte(bank, a)
        } else {
            self.open_bus
        };
        self.open_bus = v;
        v
    }

    fn write(&mut self, addr: u32, value: u8) {
        self.open_bus = value;
        let bank = (addr >> 16) as u8;
        let a = addr as u16;
        if bank & 0xFE == 0x7E {
            self.wram[(addr & 0x1FFFF) as usize] = value;
        } else if bank & 0x40 == 0 && a < 0x8000 {
            if a < 0x2000 {
                self.wram[a as usize] = value;
            } else {
                self.write_io(a, value);
            }
        }
    }

    fn idle(&mut self) {
        self.advance(6);
    }
}

/// A bus wrapper that times each access before it happens.
struct Timed<'a>(&'a mut SnesBus);

impl Bus65816 for Timed<'_> {
    fn read(&mut self, addr: u32) -> u8 {
        let clocks = self.0.speed(addr);
        self.0.advance(clocks);
        self.0.read(addr)
    }

    fn write(&mut self, addr: u32, value: u8) {
        let clocks = self.0.speed(addr);
        self.0.advance(clocks);
        self.0.write(addr, value);
    }

    fn idle(&mut self) {
        self.0.advance(6);
    }
}

/// The SNES: CPU and bus.
pub struct Snes {
    pub cpu: W65816,
    pub bus: SnesBus,
}

impl Snes {
    /// A SNES with `rom` (the SGB cartridge's program) in its cartridge slot, reset.
    pub fn new(rom: Vec<u8>) -> Self {
        let mut snes = Snes {
            cpu: W65816::new(),
            bus: SnesBus::new(rom),
        };
        snes.cpu.reset(&mut snes.bus);
        snes
    }

    /// Runs one instruction (or interrupt); returns the master clocks it took.
    pub fn step(&mut self) -> u32 {
        let start = self.bus.clock;
        if std::mem::take(&mut self.bus.nmi_line) {
            self.cpu.nmi = true;
        }
        self.cpu.irq = self.bus.irq_flag;
        self.cpu.step(&mut Timed(&mut self.bus));
        (self.bus.clock - start) as u32
    }

    /// Runs until `clocks` master clocks have passed (or a little more).
    pub fn run(&mut self, clocks: u64) {
        let end = self.bus.clock + clocks;
        while self.bus.clock < end {
            self.step();
        }
        self.bus.sync_apu();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes an APU port as the 65816 would (`$2140 + port`).
    fn port(bus: &mut SnesBus, n: u16, v: u8) {
        bus.write(0x2140 + n as u32, v);
    }

    #[test]
    fn apu_boot_protocol_uploads_and_starts_a_program() {
        let mut bus = SnesBus::new(Vec::new());
        assert_eq!(bus.read(0x2140), 0xAA);
        assert_eq!(bus.read(0x2141), 0xBB);
        // Block to $0300: 3 bytes.
        port(&mut bus, 2, 0x00);
        port(&mut bus, 3, 0x03);
        port(&mut bus, 1, 0x01);
        port(&mut bus, 0, 0xCC);
        assert_eq!(bus.read(0x2140), 0xCC, "kick echoed");
        for (i, b) in [0x8F, 0x55, 0xF4].into_iter().enumerate() {
            port(&mut bus, 1, b);
            port(&mut bus, 0, i as u8);
            assert_eq!(bus.read(0x2140), i as u8, "index echoed");
        }
        assert_eq!(&bus.smp.ram[0x300..0x303], &[0x8F, 0x55, 0xF4]);
        // Start at $0300: command 0, kick bigger than the last index + 1.
        port(&mut bus, 2, 0x00);
        port(&mut bus, 3, 0x03);
        port(&mut bus, 1, 0x00);
        port(&mut bus, 0, 0x05);
        assert!(!bus.smp.halted(), "the program runs");
        assert_eq!(bus.ipl, Ipl::Running);
    }

    #[test]
    fn lorom_mapping_and_wram_mirror() {
        let mut rom = vec![0; 0x10000];
        rom[0x0000] = 0x11; // bank 0, $8000
        rom[0x8000] = 0x22; // bank 1, $8000
        let mut bus = SnesBus::new(rom);
        assert_eq!(bus.read(0x00_8000), 0x11);
        assert_eq!(bus.read(0x81_8000), 0x22);
        bus.write(0x00_0123, 0x5A);
        assert_eq!(bus.read(0x7E_0123), 0x5A, "low WRAM mirror");
        assert_eq!(bus.read(0x80_0123), 0x5A);
    }
}
