//! Codes for Blaze's Xploder GB (Xplorer GB), as cheat sites list them: `$0D22A281`.
//!
//! An Xploder code is four bytes, `TT VV LL HH` like a GameShark code (type, value,
//! address), with the value and address encrypted. Bits 2–3 of the type pick the
//! encryption; the high nibble is a flag (0 or 8, the same RAM write either way). How it
//! works comes from the device's firmware (the routine at $41xx in bank 6 that decodes
//! the code list):
//!
//! | Type bits 2–3 | e.g.         | value    | low address byte  | high address byte |
//! |---------------|--------------|----------|-------------------|-------------------|
//! | 0             | `01`         | `VV`     | `LL`              | `HH`              |
//! | 1             | `05`, `84`   | `VV`     | `LL ^ $19`        | `HH ^ $73`        |
//! | 2             | `09`         | `VV-$1B` | `HH ^ $15`        | `LL ^ $3B`        |
//! | 3             | `0C`, `0D`   | `VV-$21` | `TABLE⁻¹[LL]`     | `TABLE⁻¹[HH]`     |
//!
//! `TABLE` is a permutation of 0–255 the firmware shuffles at start-up with a fixed
//! pseudo-random sequence; [`TABLE`] repeats that. Nearly all published codes are type
//! `0D`; all of them decrypt to RAM addresses.

use crate::cheats::{Code, Location};

/// The firmware's byte permutation: 0–255 in order, then 299 swaps of two entries chosen
/// by a linear congruential generator (`seed = seed × 245 + 123`, 16 bits, starting at
/// $2C; each pick is `seed / 255`).
pub const TABLE: [u8; 256] = shuffled();

const fn shuffled() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        table[i] = i as u8;
        i += 1;
    }
    let mut seed: u16 = 0x2C;
    let mut round = 0;
    while round < 299 {
        seed = seed.wrapping_mul(245).wrapping_add(123);
        let a = (seed / 255) as u8 as usize;
        seed = seed.wrapping_mul(245).wrapping_add(123);
        let b = (seed / 255) as u8 as usize;
        table.swap(a, b);
        round += 1;
    }
    table
}

/// The position of `byte` in [`TABLE`]: the decrypted byte.
const fn untable(byte: u8) -> u8 {
    let mut i = 0;
    while i < 256 {
        if TABLE[i] == byte {
            return i as u8;
        }
        i += 1;
    }
    unreachable!()
}

/// [`untable`] for every byte, so decrypting is a lookup.
const UNTABLE: [u8; 256] = {
    let mut out = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        out[i] = untable(i as u8);
        i += 1;
    }
    out
};

/// Decrypts an Xploder code given as 8 hex digits (without the `$`). `None` for a type
/// freki does not know or an address outside RAM.
pub fn decode(hex: &str) -> Option<Code> {
    if hex.len() != 8 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok();
    let (kind, value, lo, hi) = (byte(0)?, byte(1)?, byte(2)?, byte(3)?);
    if !matches!(kind & 0xF0, 0x00 | 0x80) {
        return None;
    }
    let (value, lo, hi) = match (kind >> 2) & 3 {
        0 => (value, lo, hi),
        1 => (value, lo ^ 0x19, hi ^ 0x73),
        2 => (value.wrapping_sub(0x1B), hi ^ 0x15, lo ^ 0x3B),
        _ => (
            value.wrapping_sub(0x21),
            UNTABLE[usize::from(lo)],
            UNTABLE[usize::from(hi)],
        ),
    };
    let address = u16::from_le_bytes([lo, hi]);
    (address >= 0x8000).then_some(Code::Xploder {
        location: Location::new(address),
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(code: &str) -> (u16, u8) {
        match Code::parse(code) {
            Ok(Code::Xploder { location, value }) => (location.address, value),
            other => panic!("{code}: {other:?}"),
        }
    }

    #[test]
    fn the_table_is_a_permutation() {
        let mut seen = [false; 256];
        for &b in &TABLE {
            seen[usize::from(b)] = true;
        }
        assert!(seen.iter().all(|&s| s));
        assert!((0..=255u8).all(|b| TABLE[usize::from(UNTABLE[usize::from(b)])] == b));
    }

    /// Codes from cheat sites, checked against GameShark codes for the same games.
    #[test]
    fn decrypts_published_codes() {
        // Super Mario Bros. Deluxe: lives at $C17F and time at $C17D, as GameShark's
        // "Infinite Lives" (90097FC1) and "Infinite Time" (01907DC1) codes.
        assert_eq!(write("$0D26A081"), (0xC17F, 0x05));
        assert_eq!(write("$0D1DC381"), (0xC17D, 0xFC));
        // Pokémon Gold: money at $D573; the encounter at $D0ED, where the firmware's own
        // GameShark code for Lugia (01F9EDD0) writes too.
        assert_eq!(write("$0DBA7396"), (0xD573, 0x99));
        assert_eq!(write("$0D22E764"), (0xD0ED, 0x01)); // Bulbasaur
        assert_eq!(write("$0D14E764"), (0xD0ED, 0xF3)); // Raikou
        // A Bug's Life: the same cheat as type $8C (cheat site) and $84 (firmware list).
        assert_eq!(write("$8C243F2A"), (0xC62F, 0x03));
        assert_eq!(write("$840336B5"), (0xC62F, 0x03));
        // Without the `$` too.
        assert_eq!(write("0D26A081"), (0xC17F, 0x05));
    }

    #[test]
    fn unknown_types_are_refused() {
        assert_eq!(decode("E0123456"), None);
        assert!(Code::parse("$E0123456").is_err());
    }
}
