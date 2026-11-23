//! MBC6 (Net de Get: Minigame @ 100): two independently banked 8 KiB ROM halves, two
//! 4 KiB RAM halves, and a 1 MiB Macronix MX29F008 flash chip that either ROM half can
//! show instead of ROM. The game downloads minigames into the flash. Registers and the
//! flash commands follow Pan Docs.
//!
//! | Range | Register |
//! |---|---|
//! | $0000–$03FF | RAM enable ($0A) |
//! | $0400–$07FF, $0800–$0BFF | RAM bank for $A000–$AFFF, $B000–$BFFF |
//! | $0C00–$0FFF | flash enable (bit 0) |
//! | $1000–$13FF | flash write enable (bit 0): needed for sector 0 and the hidden region |
//! | $2000–$27FF, $3000–$37FF | ROM/flash bank for $4000–$5FFF, $6000–$7FFF |
//! | $2800–$2FFF, $3800–$3FFF | $08 shows the flash in that half, $00 the ROM |
//!
//! Flash commands are written to the flash itself: $AA to $5555, $55 to $2AAA, then the
//! command (flash addresses, i.e. bank × $2000 + offset). See [`Mbc6::write`].

use serde::{Deserialize, Serialize};

/// The flash chip's size; it is saved after the RAM, as mGBA does.
pub const FLASH_BYTES: usize = 0x10_0000;
/// The hidden region (not part of mGBA's save) and the sector 0 protection flag, saved
/// after the flash.
pub const EXTRA_BYTES: usize = HIDDEN_BYTES + 1;
const HIDDEN_BYTES: usize = 256;
const SECTOR_BYTES: usize = 0x2_0000;
const ROM_HALF: usize = 0x2000;
const RAM_HALF: usize = 0x1000;
const PAGE_BYTES: usize = 128;

/// Status read while or after erasing or programming: bit 7 done, bit 1 sector 0
/// protected, bit 4 failed (freki finishes everything at once).
const STATUS_DONE: u8 = 0x80;
const STATUS_FAILED: u8 = 0x10;
const STATUS_PROTECTED: u8 = 0x02;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
enum FlashMode {
    Read,
    /// The JEDEC ID: $C2 (Macronix) at $xxx0, $81 at $xxx1.
    Id,
    /// Reads give the status of the last erase, program or (un)protect.
    Status,
    /// The hidden 256 bytes read in place of the flash.
    ReadHidden,
    /// Collecting a 128-byte page; writing the page's last address again commits it.
    Program {
        hidden: bool,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Mbc6 {
    pub ram_enabled: bool,
    ram_bank: [u8; 2],
    rom_bank: [u8; 2],
    /// Whether each ROM half shows the flash.
    flash_mapped: [bool; 2],
    flash_enabled: bool,
    flash_write_enabled: bool,
    flash: Vec<u8>,
    hidden: Vec<u8>,
    sector0_protected: bool,
    mode: FlashMode,
    /// How far into the $AA/$55 unlock sequence the writes are (0–2).
    unlock: u8,
    /// The first command of a two-part one ($80 erase, $60 special, $77 hidden read).
    prefix: Option<u8>,
    status: u8,
    /// The page being programmed: its first address, its bytes ($FF where not written)
    /// and the last address written.
    page: Option<(usize, Vec<u8>, usize)>,
}

impl Default for Mbc6 {
    fn default() -> Self {
        Self {
            ram_enabled: false,
            ram_bank: [0, 1],
            rom_bank: [2, 3],
            flash_mapped: [false; 2],
            flash_enabled: false,
            flash_write_enabled: false,
            flash: vec![0xFF; FLASH_BYTES],
            hidden: vec![0xFF; HIDDEN_BYTES],
            sector0_protected: false,
            mode: FlashMode::Read,
            unlock: 0,
            prefix: None,
            status: 0,
            page: None,
        }
    }
}

impl Mbc6 {
    /// A register write ($0000–$3FFF), or a write to a flash half ($4000–$7FFF). Returns
    /// `true` if the flash changed.
    pub fn write(&mut self, addr: u16, value: u8) -> bool {
        match addr {
            0x0000..=0x03FF => self.ram_enabled = value & 0x0F == 0x0A,
            0x0400..=0x07FF => self.ram_bank[0] = value,
            0x0800..=0x0BFF => self.ram_bank[1] = value,
            0x0C00..=0x0FFF => self.flash_enabled = value & 1 != 0,
            0x1000..=0x13FF => self.flash_write_enabled = value & 1 != 0,
            0x2000..=0x27FF => self.rom_bank[0] = value & 0x7F,
            0x2800..=0x2FFF => self.flash_mapped[0] = value & 0x08 != 0,
            0x3000..=0x37FF => self.rom_bank[1] = value & 0x7F,
            0x3800..=0x3FFF => self.flash_mapped[1] = value & 0x08 != 0,
            0x4000..=0x7FFF => {
                let half = usize::from(addr >= 0x6000);
                if self.flash_mapped[half] && self.flash_enabled {
                    return self.command(self.flash_addr(half, addr), value);
                }
            }
            _ => {}
        }
        false
    }

    /// What $4000–$7FFF reads when that half shows the flash, or `None` for ROM: the ROM
    /// byte is then at [`Mbc6::rom_offset`].
    pub fn read_flash(&self, addr: u16) -> Option<u8> {
        let half = usize::from(addr >= 0x6000);
        if !self.flash_mapped[half] {
            return None;
        }
        let at = self.flash_addr(half, addr);
        Some(match self.mode {
            FlashMode::Read | FlashMode::Program { .. } => self.flash[at],
            FlashMode::Id => match at & 0x0F {
                0 => 0xC2,
                1 => 0x81,
                _ => self.flash[at],
            },
            FlashMode::Status => self.status,
            FlashMode::ReadHidden => self.hidden[at % HIDDEN_BYTES],
        })
    }

    /// The ROM offset $4000–$7FFF reads from (before wrapping to the ROM's size).
    pub fn rom_offset(&self, addr: u16) -> usize {
        let half = usize::from(addr >= 0x6000);
        usize::from(self.rom_bank[half]) * ROM_HALF + (usize::from(addr) & (ROM_HALF - 1))
    }

    /// The RAM offset $A000–$BFFF reaches (before wrapping to the RAM's size), or `None`
    /// while RAM is off.
    pub fn ram_offset(&self, addr: u16) -> Option<usize> {
        let half = usize::from(addr >= 0xB000);
        self.ram_enabled.then(|| {
            usize::from(self.ram_bank[half]) * RAM_HALF + (usize::from(addr) & (RAM_HALF - 1))
        })
    }

    /// The save file's part after the RAM: the flash (as mGBA saves it), then the hidden
    /// region and the protection flag.
    pub fn to_save(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(FLASH_BYTES + EXTRA_BYTES);
        out.extend(&self.flash);
        out.extend(&self.hidden);
        out.push(u8::from(self.sector0_protected));
        out
    }

    /// Reads [`Mbc6::to_save`]'s block; a part that is missing (a save from mGBA has no
    /// hidden region) stays as it is.
    pub fn load_save(&mut self, data: &[u8]) {
        if let Some(flash) = data.get(..FLASH_BYTES) {
            self.flash.copy_from_slice(flash);
        }
        if let Some(extra) = data.get(FLASH_BYTES..FLASH_BYTES + EXTRA_BYTES) {
            self.hidden.copy_from_slice(&extra[..HIDDEN_BYTES]);
            self.sector0_protected = extra[HIDDEN_BYTES] != 0;
        }
    }

    fn flash_addr(&self, half: usize, addr: u16) -> usize {
        (usize::from(self.rom_bank[half]) * ROM_HALF + (usize::from(addr) & (ROM_HALF - 1)))
            % FLASH_BYTES
    }

    fn sector0_writable(&self) -> bool {
        self.flash_write_enabled && !self.sector0_protected
    }

    /// Finishes an erase, program or (un)protect: reads give the status until $F0.
    fn finish(&mut self, ok: bool) {
        let protected = if self.sector0_protected {
            STATUS_PROTECTED
        } else {
            0
        };
        self.status = STATUS_DONE | protected | if ok { 0 } else { STATUS_FAILED };
        self.mode = FlashMode::Status;
    }

    /// A write to flash address `at`. Returns `true` if the flash changed.
    fn command(&mut self, at: usize, value: u8) -> bool {
        if let FlashMode::Program { hidden } = self.mode {
            return self.program(hidden, at, value);
        }
        if value == 0xF0 {
            self.mode = FlashMode::Read;
            self.unlock = 0;
            self.prefix = None;
            return false;
        }
        let cmd = at & 0x7FFF;
        match (self.unlock, cmd, value) {
            (0, 0x5555, 0xAA) | (1, 0x2AAA, 0x55) => {
                self.unlock += 1;
                return false;
            }
            (2, _, _) => self.unlock = 0,
            _ => {
                self.unlock = 0;
                self.prefix = None;
                return false;
            }
        }
        let at_5555 = cmd == 0x5555;
        match (self.prefix.take(), value) {
            (None, 0x80 | 0x60 | 0x77) if at_5555 => self.prefix = Some(value),
            (None, 0x90) if at_5555 => self.mode = FlashMode::Id,
            (None, 0xA0) if at_5555 => self.mode = FlashMode::Program { hidden: false },
            (Some(0x80), 0x30) => {
                let sector = at / SECTOR_BYTES;
                let ok = sector != 0 || self.sector0_writable();
                if ok {
                    self.flash[sector * SECTOR_BYTES..][..SECTOR_BYTES].fill(0xFF);
                }
                self.finish(ok);
                return ok;
            }
            (Some(0x80), 0x10) if at_5555 => {
                let from = if self.sector0_writable() {
                    0
                } else {
                    SECTOR_BYTES
                };
                self.flash[from..].fill(0xFF);
                self.finish(true);
                return true;
            }
            (Some(0x60), 0x04 | 0x40 | 0x20 | 0xE0) if at_5555 => {
                let ok = self.flash_write_enabled;
                match value {
                    0xE0 if ok => self.mode = FlashMode::Program { hidden: true },
                    0x04 if ok => self.hidden.fill(0xFF),
                    0x40 if ok => self.sector0_protected = false,
                    0x20 if ok => self.sector0_protected = true,
                    _ => {}
                }
                if value != 0xE0 || !ok {
                    self.finish(ok);
                }
                return ok && matches!(value, 0x04 | 0x40 | 0x20);
            }
            (Some(0x77), 0x77) if at_5555 => self.mode = FlashMode::ReadHidden,
            _ => {}
        }
        false
    }

    /// A write in program mode: a byte of the page, or the page's last address again,
    /// which programs it (bits can only go from 1 to 0).
    fn program(&mut self, hidden: bool, at: usize, value: u8) -> bool {
        let at = if hidden { at % HIDDEN_BYTES } else { at };
        let base = at & !(PAGE_BYTES - 1);
        match &mut self.page {
            Some((_, _, last)) if *last == at => {
                let (base, bytes, _) = self.page.take().unwrap();
                let ok = hidden && self.flash_write_enabled
                    || !hidden && (base >= SECTOR_BYTES || self.sector0_writable());
                if ok {
                    let target = if hidden {
                        &mut self.hidden
                    } else {
                        &mut self.flash
                    };
                    for (dst, src) in target[base..][..PAGE_BYTES].iter_mut().zip(bytes) {
                        *dst &= src;
                    }
                }
                self.finish(ok);
                ok
            }
            Some((page, bytes, last)) if *page == base => {
                bytes[at - base] = value;
                *last = at;
                false
            }
            _ => {
                let mut bytes = vec![0xFF; PAGE_BYTES];
                bytes[at - base] = value;
                self.page = Some((base, bytes, at));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shows flash bank `bank` in $4000–$5FFF and turns the flash on.
    fn flash_in_a(bank: u8) -> Mbc6 {
        let mut m = Mbc6::default();
        m.write(0x0C00, 1);
        m.write(0x2800, 0x08);
        m.write(0x2000, bank);
        m
    }

    /// The unlock sequence and `command` at $5555, through half A: flash $5555 is bank
    /// 2 + $1555, $2AAA is bank 1 + $0AAA.
    fn send(m: &mut Mbc6, command: u8) {
        m.write(0x2000, 2);
        m.write(0x5555, 0xAA);
        m.write(0x2000, 1);
        m.write(0x4AAA, 0x55);
        m.write(0x2000, 2);
        m.write(0x5555, command);
    }

    #[test]
    fn banks_and_flash_mapping() {
        let mut m = Mbc6::default();
        assert_eq!(
            (m.rom_offset(0x4000), m.rom_offset(0x6001)),
            (0x4000, 0x6001)
        );
        m.write(0x2000, 0x10);
        m.write(0x3000, 0x7F);
        assert_eq!(m.rom_offset(0x4123), 0x20123);
        assert_eq!(m.rom_offset(0x7FFF), 0xFFFFF);
        assert_eq!(m.read_flash(0x4000), None, "ROM");
        m.write(0x2800, 0x08);
        assert_eq!(m.read_flash(0x4000), Some(0xFF), "blank flash");
        assert_eq!(m.ram_offset(0xA000), None);
        m.write(0x0000, 0x0A);
        m.write(0x0800, 7);
        assert_eq!(m.ram_offset(0xB234), Some(0x7234));
    }

    #[test]
    fn id_erase_and_program() {
        let mut m = flash_in_a(0);
        send(&mut m, 0x90);
        assert_eq!(
            (m.read_flash(0x4000), m.read_flash(0x4001)),
            (Some(0xC2), Some(0x81))
        );
        m.write(0x4000, 0xF0);
        assert_eq!(m.read_flash(0x4000), Some(0xFF));

        // Program a page in sector 1 (bank $10 onwards): 128 bytes, then the last again.
        send(&mut m, 0xA0);
        m.write(0x2000, 0x10);
        for i in 0..128u16 {
            assert!(!m.write(0x4080 + i, i as u8));
        }
        assert!(m.write(0x40FF, 0));
        assert_eq!(m.read_flash(0x4000), Some(STATUS_DONE));
        m.write(0x4000, 0xF0);
        assert_eq!(m.flash[0x20080..0x20084], [0, 1, 2, 3]);
        assert_eq!(m.flash[0x200FF], 127);

        // Erase sector 1 again: the last byte goes anywhere in the sector.
        send(&mut m, 0x80);
        send(&mut m, 0xAA); // not part of the command: starts over
        send(&mut m, 0x80);
        m.write(0x2000, 2);
        m.write(0x5555, 0xAA);
        m.write(0x2000, 1);
        m.write(0x4AAA, 0x55);
        m.write(0x2000, 0x11);
        assert!(m.write(0x4567, 0x30));
        assert_eq!(m.flash[0x20080], 0xFF);
    }

    #[test]
    fn sector_0_and_hidden_region_need_write_enable() {
        let mut m = flash_in_a(0);
        send(&mut m, 0xA0);
        m.write(0x4000, 0x12);
        assert!(!m.write(0x4000, 0x12), "write protected");
        assert_eq!(m.read_flash(0x4000), Some(STATUS_DONE | STATUS_FAILED));
        m.write(0x4000, 0xF0);
        assert_eq!(m.flash[0], 0xFF);

        m.write(0x1000, 1);
        send(&mut m, 0x60);
        send(&mut m, 0x20); // protect sector 0
        m.write(0x4000, 0xF0);
        send(&mut m, 0x60);
        send(&mut m, 0xE0); // program the hidden region
        m.write(0x4005, 0x5A);
        assert!(m.write(0x4005, 0));
        m.write(0x4000, 0xF0);
        send(&mut m, 0x77);
        send(&mut m, 0x77);
        assert_eq!(m.read_flash(0x4005), Some(0x5A));
        m.write(0x4000, 0xF0);

        // Erase chip leaves the protected sector 0 alone; saving keeps all of it.
        m.flash[0] = 0;
        m.flash[SECTOR_BYTES] = 0;
        send(&mut m, 0x80);
        send(&mut m, 0x10);
        assert_eq!((m.flash[0], m.flash[SECTOR_BYTES]), (0, 0xFF));
        let mut loaded = Mbc6::default();
        loaded.load_save(&m.to_save());
        assert_eq!(loaded.flash, m.flash);
        assert_eq!(loaded.hidden[5], 0x5A);
        assert!(loaded.sector0_protected);
    }
}
