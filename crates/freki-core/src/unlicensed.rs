//! Mappers of unlicensed and unusual cartridges, which their headers don't announce:
//!
//! - **Wisdom Tree** (Spiritual Warfare, Exodus, King James Bible, …): one 74LS377 latch;
//!   any write to $0000–$7FFF selects the 32 KiB bank in the low 8 bits of the *address*.
//! - **M161** (Mani 4-in-1 with Tetris, Tennis, Alleyway, Yakuman): 32 KiB banks and a
//!   single bank switch per power-on (Pan Docs).
//! - **Sachen MMC1 / MMC2**: MBC1-like banking with a base bank and mask for multicarts,
//!   and a trick against the boot ROM's logo check: for the first $31 reads of $01xx the
//!   cartridge shows a second header at $0180 holding Nintendo's logo, and address lines
//!   A0/A6 and A1/A4 are swapped in $0100–$01FF. MMC2 does this twice (DMG, then CGB boot
//!   ROM). Behaviour as in mGBA, the reference for these cartridges.

use std::cell::Cell;

use serde::{Deserialize, Serialize};

const BANK: usize = 0x4000;
const LOGO: std::ops::Range<usize> = 0x104..0x134;

/// Which of these mappers `rom` needs, if any.
pub(crate) fn detect(rom: &[u8]) -> Option<Kind> {
    if is_wisdom_tree(rom) {
        return Some(Kind::WisdomTree);
    }
    if is_m161(rom) {
        return Some(Kind::M161);
    }
    // The logo's first, 17th and last bytes ($CE, $66, $ED), scrambled into the header.
    if rom.len() >= 0x200 && rom[0x104] == 0xCE && rom[0x144] == 0xED && rom[0x114] == 0x66 {
        return Some(Kind::SachenMmc1);
    }
    if rom.len() >= 0x200 && rom[0x184] == 0xCE && rom[0x1C4] == 0xED && rom[0x194] == 0x66 {
        return Some(Kind::SachenMmc2);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    WisdomTree,
    M161,
    SachenMmc1,
    SachenMmc2,
}

/// Wisdom Tree games: title "WISDOM TREE" (Pan Docs), or an empty header with checksum
/// $E7 and "WISDOM TREE" in the ROM (mGBA), or type $C0 with $014A = $D1 (Pan Docs).
fn is_wisdom_tree(rom: &[u8]) -> bool {
    if rom.len() <= 0x8000 {
        return false;
    }
    let title = &rom[0x134..0x13F];
    let named = title == b"WISDOM TREE" || title == b"WISDOM\0TREE";
    if named && rom[0x147] == 0x00 && rom[0x148] == 0x00 {
        return true;
    }
    if rom[0x147] == 0xC0 && rom[0x14A] == 0xD1 {
        return true;
    }
    let blank =
        rom[0x134..0x14C].iter().all(|&b| b == 0) && rom[0xF0..0x100].iter().all(|&b| b == 0);
    blank
        && rom[0x147] == 0x00
        && rom[0x14D] == 0xE7
        && rom[0x300..]
            .windows(11)
            .any(|w| &w[..6] == b"WISDOM" && &w[7..] == b"TREE")
}

/// The Mani 4-in-1: 256 KiB without a mapper in the header, where every 32 KiB bank
/// starts with a game's own header (the menu, then the games).
fn is_m161(rom: &[u8]) -> bool {
    rom.len() == 0x40000
        && rom[0x147] == 0x00
        && rom[LOGO].iter().any(|&b| b != 0)
        && rom
            .chunks(0x8000)
            .filter(|bank| bank[LOGO] == rom[LOGO])
            .count()
            >= 4
}

/// Sachen's lock against the boot ROM: locked for the DMG boot ROM, then (MMC2 only) for
/// the CGB one, then open.
const LOCKED_DMG: u8 = 0;
const LOCKED_CGB: u8 = 1;
const UNLOCKED: u8 = 2;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Sachen {
    mmc2: bool,
    base: u8,
    mask: u8,
    /// The ROM bank as written ($2000–$3FFF); bits 4–5 set open the base and mask
    /// registers.
    bank: u8,
    /// Reads count towards unlocking, and reading ROM doesn't change the cartridge
    /// otherwise.
    lock: Cell<u8>,
    reads: Cell<u8>,
}

impl Sachen {
    pub(crate) fn new(mmc2: bool) -> Self {
        Self {
            mmc2,
            base: 0,
            mask: 0,
            bank: 1,
            lock: Cell::new(LOCKED_DMG),
            reads: Cell::new(0),
        }
    }

    /// Started without a boot ROM: MMC2 has nothing left to hide (MMC1 keeps counting
    /// the game's own reads, as in mGBA).
    pub(crate) fn skip_boot(&mut self) {
        if self.mmc2 {
            self.lock.set(UNLOCKED);
        }
    }

    pub(crate) fn write(&mut self, addr: u16, value: u8) {
        let open = self.bank & 0x30 == 0x30;
        match addr >> 13 {
            0 if open => self.base = value,
            1 => self.bank = value.max(1),
            2 if open => self.mask = value,
            _ => {}
        }
    }

    /// A CPU access to $C000 and up: on MMC2 it moves the lock on to the CGB stage.
    pub(crate) fn high_access(&self) {
        if self.mmc2 && self.lock.get() == LOCKED_DMG {
            self.lock.set(LOCKED_CGB);
            self.reads.set(0);
        }
    }

    /// The ROM address a read of `addr` really reaches, and the bank there (16 KiB).
    pub(crate) fn map(&self, addr: u16) -> (usize, u16) {
        let mut addr = addr;
        let header = addr & 0xFF00 == 0x0100;
        if self.mmc2 {
            if self.lock.get() != UNLOCKED && addr & 0x8700 == 0x0100 {
                self.count(|s| s.lock.set(s.lock.get() + 1));
            }
            if header && self.lock.get() == LOCKED_CGB {
                addr |= 0x80;
            }
        } else if self.lock.get() != UNLOCKED && header {
            if self.count(|s| s.lock.set(UNLOCKED)) {
                // Unlocked by this read.
            } else {
                addr |= 0x80;
            }
        }
        if addr & 0xFF00 == 0x0100 {
            addr = unscramble(addr);
        }
        let bank = if addr < 0x4000 {
            self.base & self.mask
        } else {
            (self.bank & !self.mask) | (self.base & self.mask)
        };
        (usize::from(bank), addr)
    }

    /// The bank at `addr` without counting a read (for debuggers).
    pub(crate) fn bank_at(&self, addr: u16) -> usize {
        let bank = if addr < 0x4000 {
            self.base & self.mask
        } else {
            (self.bank & !self.mask) | (self.base & self.mask)
        };
        usize::from(bank)
    }

    /// Counts a header read; on the $31st runs `unlock` and returns true.
    fn count(&self, unlock: impl FnOnce(&Self)) -> bool {
        let reads = self.reads.get() + 1;
        if reads == 0x31 {
            self.reads.set(0);
            unlock(self);
            true
        } else {
            self.reads.set(reads);
            false
        }
    }
}

/// Sachen's header wiring: A0↔A6 and A1↔A4.
fn unscramble(addr: u16) -> u16 {
    (addr & 0xFFAC)
        | ((addr & 0x40) >> 6)
        | ((addr & 0x10) >> 3)
        | ((addr & 0x02) << 3)
        | ((addr & 0x01) << 6)
}

/// The 16 KiB bank a 32 KiB-switching mapper (Wisdom Tree, M161) shows at `addr`.
pub(crate) fn bank_of_32k(bank32: u8, addr: u16) -> usize {
    usize::from(bank32) * 2 + usize::from(addr >= BANK as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::Cartridge;

    /// `banks32` banks of 32 KiB; each byte holds its 16 KiB bank number.
    fn rom(banks32: usize) -> Vec<u8> {
        (0..banks32 * 2).flat_map(|b| vec![b as u8; BANK]).collect()
    }

    #[test]
    fn wisdom_tree_switches_32k_by_address() {
        let mut data = rom(4);
        data[0x134..0x13F].copy_from_slice(b"WISDOM TREE");
        data[0x147] = 0;
        data[0x148] = 0;
        assert_eq!(detect(&data), Some(Kind::WisdomTree));
        let mut cart = Cartridge::from_rom(data).unwrap();
        assert_eq!((cart.read_rom(0x0000), cart.read_rom(0x4000)), (0, 1));
        cart.write_rom(0x0002, 0xFF); // bank from the address: 2
        assert_eq!((cart.read_rom(0x0000), cart.read_rom(0x4000)), (4, 5));
        cart.write_rom(0x7F01, 0x00);
        assert_eq!((cart.read_rom(0x0000), cart.read_rom(0x4000)), (2, 3));
    }

    #[test]
    fn m161_switches_once() {
        let mut data = rom(8);
        for bank in 0..8 {
            let at = bank * 0x8000;
            data[at + 0x104..at + 0x134].copy_from_slice(&[0xCE; 48]);
            data[at + 0x147] = 0;
        }
        assert_eq!(detect(&data), Some(Kind::M161));
        let mut cart = Cartridge::from_rom(data).unwrap();
        cart.write_rom(0x2000, 0x03);
        assert_eq!((cart.read_rom(0x0000), cart.read_rom(0x4000)), (6, 7));
        cart.write_rom(0x2000, 0x01);
        assert_eq!(cart.read_rom(0x0000), 6, "only one switch per power-on");
    }

    #[test]
    fn sachen_unscrambles_and_unlocks_after_0x31_header_reads() {
        assert_eq!(unscramble(0x0101), 0x0140);
        assert_eq!(unscramble(0x0140), 0x0101);
        assert_eq!(unscramble(0x0110), 0x0102);
        let sachen = Sachen::new(false);
        // Locked: $0104 reads the second header ($0184, unscrambled).
        assert_eq!(sachen.map(0x0104).1, unscramble(0x0184));
        for _ in 0..0x2F {
            sachen.map(0x0104);
        }
        // The $31st read unlocks: from now on only the scrambling remains.
        assert_eq!(sachen.map(0x0104).1, unscramble(0x0104));
        assert_eq!(sachen.map(0x0104).1, unscramble(0x0104));
    }

    #[test]
    fn sachen_base_and_mask_need_bank_bits_4_and_5() {
        let mut sachen = Sachen::new(false);
        sachen.lock.set(UNLOCKED);
        sachen.write(0x0000, 0x10); // ignored: bank bits 4–5 are not set
        assert_eq!(sachen.base, 0);
        sachen.write(0x2000, 0x30);
        sachen.write(0x0000, 0x08);
        sachen.write(0x4000, 0x0C);
        sachen.write(0x2000, 0x03);
        // Upper area: (3 & !$0C) | (8 & $0C) = $0B; lower area: 8 & $0C = 8.
        assert_eq!(sachen.map(0x4000).0, 0x0B);
        assert_eq!(sachen.map(0x0000).0, 0x08);
    }
}
