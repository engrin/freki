//! Cheat codes and a memory search for finding new ones.
//!
//! - **GameShark** codes (`01VVLLHH`) write value `VV` to address `HHLL` once per frame.
//! - **Game Genie** codes (`ABC-DEF` or `ABC-DEF-GHI`) patch what the CPU reads from ROM.
//!   The 9-digit form only applies when the original byte equals the compare value, which
//!   keeps a patch from hitting other ROM banks mapped at the same address.

use std::fmt;

/// A parsed cheat code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    GameShark {
        address: u16,
        value: u8,
    },
    GameGenie {
        address: u16,
        value: u8,
        compare: Option<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheatError {
    /// Not 8 hex digits (GameShark) or 6/9 hex digits (Game Genie).
    Format(String),
    /// GameShark code type other than `01`.
    UnsupportedType(u8),
    /// Game Genie address outside ROM ($0000–$7FFF).
    NotRomAddress(u16),
}

impl fmt::Display for CheatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(code) => write!(
                f,
                "{code:?} is not a GameShark (01VVLLHH) or Game Genie (ABC-DEF[-GHI]) code"
            ),
            Self::UnsupportedType(t) => write!(f, "GameShark code type {t:02X} is not supported"),
            Self::NotRomAddress(a) => write!(f, "Game Genie address ${a:04X} is not in ROM"),
        }
    }
}

impl std::error::Error for CheatError {}

impl Code {
    /// Parses a GameShark or Game Genie code. Dashes and spaces are ignored.
    pub fn parse(code: &str) -> Result<Self, CheatError> {
        let digits: Vec<u8> = code
            .chars()
            .filter(|c| !matches!(c, '-' | ' '))
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()
            .ok_or_else(|| CheatError::Format(code.to_string()))?;
        let byte = |i: usize| (digits[i] << 4) | digits[i + 1];
        match digits.len() {
            8 => {
                let kind = byte(0);
                if kind != 0x01 {
                    return Err(CheatError::UnsupportedType(kind));
                }
                Ok(Self::GameShark {
                    value: byte(2),
                    address: u16::from_le_bytes([byte(4), byte(6)]),
                })
            }
            6 | 9 => {
                // ABC-DEF-GHI: AB = value; address = (F ^ $F)CDE; compare = GI ror 2 ^ $BA.
                let address = (((digits[5] ^ 0xF) as u16) << 12)
                    | ((digits[2] as u16) << 8)
                    | ((digits[3] as u16) << 4)
                    | digits[4] as u16;
                if address > 0x7FFF {
                    return Err(CheatError::NotRomAddress(address));
                }
                let compare = (digits.len() == 9)
                    .then(|| ((digits[6] << 4) | digits[8]).rotate_right(2) ^ 0xBA);
                Ok(Self::GameGenie {
                    address,
                    value: byte(0),
                    compare,
                })
            }
            _ => Err(CheatError::Format(code.to_string())),
        }
    }

    /// The GameShark code that writes `value` to `address`.
    pub fn gameshark(address: u16, value: u8) -> String {
        let [lo, hi] = address.to_le_bytes();
        format!("01{value:02X}{lo:02X}{hi:02X}")
    }
}

/// A cheat as entered by the user.
#[derive(Debug, Clone)]
pub struct Cheat {
    pub code: Code,
    /// The code as typed, for display and saving.
    pub text: String,
    pub name: String,
    pub enabled: bool,
}

/// The active cheat list. Not part of save states.
#[derive(Debug, Clone)]
pub struct Cheats {
    list: Vec<Cheat>,
    /// Master switch for all cheats.
    pub enabled: bool,
}

impl Default for Cheats {
    fn default() -> Self {
        Self::new()
    }
}

impl Cheats {
    pub fn new() -> Self {
        Self {
            list: Vec::new(),
            enabled: true,
        }
    }

    /// Parses and adds a cheat (enabled). Returns its index.
    pub fn add(&mut self, text: &str, name: &str) -> Result<usize, CheatError> {
        let code = Code::parse(text)?;
        self.list.push(Cheat {
            code,
            text: text.trim().to_uppercase(),
            name: name.trim().to_string(),
            enabled: true,
        });
        Ok(self.list.len() - 1)
    }

    pub fn remove(&mut self, index: usize) -> Option<Cheat> {
        (index < self.list.len()).then(|| self.list.remove(index))
    }

    pub fn set_enabled(&mut self, index: usize, enabled: bool) -> bool {
        match self.list.get_mut(index) {
            Some(cheat) => {
                cheat.enabled = enabled;
                true
            }
            None => false,
        }
    }

    pub fn list(&self) -> &[Cheat] {
        &self.list
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }

    fn active(&self) -> impl Iterator<Item = &Code> {
        self.list
            .iter()
            .filter(move |c| self.enabled && c.enabled)
            .map(|c| &c.code)
    }

    /// GameShark writes to apply this frame.
    pub fn frame_writes(&self) -> impl Iterator<Item = (u16, u8)> + '_ {
        self.active().filter_map(|c| match *c {
            Code::GameShark { address, value } => Some((address, value)),
            Code::GameGenie { .. } => None,
        })
    }

    /// Applies Game Genie patches to a ROM read of `original` at `address`.
    pub fn patch_rom_read(&self, address: u16, original: u8) -> u8 {
        for code in self.active() {
            if let Code::GameGenie {
                address: a,
                value,
                compare,
            } = *code
                && a == address
                && compare.is_none_or(|c| c == original)
            {
                return value;
            }
        }
        original
    }

    /// Whether any Game Genie patch is active (fast path for ROM reads).
    pub fn has_rom_patches(&self) -> bool {
        self.active().any(|c| matches!(c, Code::GameGenie { .. }))
    }

    /// Loads cheats from text: one code per line, optionally followed by a name;
    /// lines starting with `#` are comments, a leading `!` adds the cheat disabled.
    /// Returns the lines that could not be parsed.
    pub fn load_text(&mut self, text: &str) -> Vec<(usize, CheatError)> {
        let mut errors = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (disabled, line) = match line.strip_prefix('!') {
                Some(rest) => (true, rest.trim_start()),
                None => (false, line),
            };
            let (code, name) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            match self.add(code, name) {
                Ok(i) => {
                    self.list[i].enabled = !disabled;
                }
                Err(e) => errors.push((n + 1, e)),
            }
        }
        errors
    }

    /// The cheat list in the text format read by [`Cheats::load_text`].
    pub fn to_text(&self) -> String {
        let mut out = String::from("# freki cheats: CODE [name]; '!' = disabled\n");
        for c in &self.list {
            let flag = if c.enabled { "" } else { "! " };
            out.push_str(&format!("{flag}{} {}\n", c.text, c.name).replace(" \n", "\n"));
        }
        out
    }
}

/// Comparison used to narrow a [`MemorySearch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Equal(u8),
    NotEqual(u8),
    Changed,
    Unchanged,
    Increased,
    Decreased,
}

/// Finds the RAM address holding a value (lives, money, …) by repeated filtering.
///
/// Start a search, play until the value changes, filter (e.g. [`Filter::Decreased`]),
/// repeat until few candidates remain, then turn one into a GameShark code.
#[derive(Debug, Clone, Default)]
pub struct MemorySearch {
    /// Candidate addresses with the value seen at the last filter.
    candidates: Vec<(u16, u8)>,
}

/// RAM a search covers: cartridge RAM, work RAM and high RAM.
pub const SEARCH_RANGES: [std::ops::RangeInclusive<u16>; 3] =
    [0xA000..=0xBFFF, 0xC000..=0xDFFF, 0xFF80..=0xFFFE];

impl MemorySearch {
    /// Starts a search over all searchable RAM, using `read` to get current values.
    pub fn start(read: impl Fn(u16) -> u8) -> Self {
        let candidates = SEARCH_RANGES
            .iter()
            .flat_map(|r| r.clone())
            .map(|a| (a, read(a)))
            .collect();
        Self { candidates }
    }

    /// Keeps the candidates whose current value passes `filter`. Returns how many remain.
    pub fn filter(&mut self, filter: Filter, read: impl Fn(u16) -> u8) -> usize {
        self.candidates.retain_mut(|(addr, last)| {
            let now = read(*addr);
            let keep = match filter {
                Filter::Equal(v) => now == v,
                Filter::NotEqual(v) => now != v,
                Filter::Changed => now != *last,
                Filter::Unchanged => now == *last,
                Filter::Increased => now > *last,
                Filter::Decreased => now < *last,
            };
            *last = now;
            keep
        });
        self.candidates.len()
    }

    pub fn candidates(&self) -> &[(u16, u8)] {
        &self.candidates
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gameshark() {
        assert_eq!(
            Code::parse("010A10C1"),
            Ok(Code::GameShark {
                address: 0xC110,
                value: 0x0A
            })
        );
        assert_eq!(Code::gameshark(0xC110, 0x0A), "010A10C1");
        assert_eq!(
            Code::parse("910A10C1"),
            Err(CheatError::UnsupportedType(0x91))
        );
    }

    #[test]
    fn parses_game_genie() {
        // 00A-17B-C49: value $00, address (B ^ F)A17 = $4A17, compare (C9 ror 2) ^ BA = $C8.
        assert_eq!(
            Code::parse("00A-17B-C49"),
            Ok(Code::GameGenie {
                address: 0x4A17,
                value: 0x00,
                compare: Some(0xC8)
            })
        );
        assert_eq!(
            Code::parse("3EA 17B"),
            Ok(Code::GameGenie {
                address: 0x4A17,
                value: 0x3E,
                compare: None
            })
        );
        assert_eq!(
            Code::parse("00A-170"),
            Err(CheatError::NotRomAddress(0xFA17))
        );
        assert!(matches!(Code::parse("XYZ"), Err(CheatError::Format(_))));
    }

    /// Game Genie decoding as SameBoy implements it (Core/cheats.c), ported literally:
    /// strip dashes, drop the 8th digit, read "VVAAAA[OO]" as hex, rotate the address
    /// right by 4 and XOR $F000, rotate the compare byte right by 2 and XOR $BA.
    #[allow(clippy::manual_rotate)] // kept as literal shifts, like the C original
    fn sameboy_game_genie(code: &str) -> Option<(u16, u8, Option<u8>)> {
        let mut digits: String = code.chars().filter(|&c| c != '-').collect();
        if digits.len() == 9 {
            digits.remove(7);
        }
        if digits.len() != 8 && digits.len() != 6 {
            return None;
        }
        let value = u8::from_str_radix(&digits[0..2], 16).ok()?;
        let raw = u16::from_str_radix(&digits[2..6], 16).ok()?;
        let address = ((raw >> 4) | (raw << 12)) ^ 0xF000;
        if address > 0x7FFF {
            return None;
        }
        let compare = if digits.len() == 8 {
            let old = u8::from_str_radix(&digits[6..8], 16).ok()?;
            Some(((old >> 2) | (old << 6)) ^ 0xBA)
        } else {
            None
        };
        Some((address, value, compare))
    }

    #[test]
    fn game_genie_matches_sameboy_reference() {
        // Deterministic pseudo-random codes covering all digit values and positions.
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut checked = 0;
        for _ in 0..10_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let d: Vec<char> = (0..9)
                .map(|i| char::from_digit(((seed >> (i * 4)) & 0xF) as u32, 16).unwrap())
                .map(|c| c.to_ascii_uppercase())
                .collect();
            let long = format!(
                "{}{}{}-{}{}{}-{}{}{}",
                d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7], d[8]
            );
            let short: String = long[..7].to_string();
            for code in [long.as_str(), short.as_str(), &long.replace('-', "")] {
                let ours = Code::parse(code).ok().map(|c| match c {
                    Code::GameGenie {
                        address,
                        value,
                        compare,
                    } => (address, value, compare),
                    Code::GameShark { .. } => panic!("{code} parsed as GameShark"),
                });
                assert_eq!(ours, sameboy_game_genie(code), "code {code}");
                checked += 1;
            }
        }
        assert_eq!(checked, 30_000);
    }

    #[test]
    fn game_genie_compare_protects_other_banks() {
        let mut cheats = Cheats::new();
        cheats.add("00A-17B-C49", "").unwrap();
        assert_eq!(cheats.patch_rom_read(0x4A17, 0xC8), 0x00);
        assert_eq!(
            cheats.patch_rom_read(0x4A17, 0xC9),
            0xC9,
            "compare mismatch"
        );
        cheats.enabled = false;
        assert_eq!(
            cheats.patch_rom_read(0x4A17, 0xC8),
            0xC8,
            "master switch off"
        );
    }

    #[test]
    fn cheat_file_round_trip() {
        let mut cheats = Cheats::new();
        let errors = cheats.load_text("# comment\n010A10C1 Infinite lives\n! 00A-17B-C49\nnope\n");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, 4);
        assert_eq!(cheats.list().len(), 2);
        assert!(!cheats.list()[1].enabled);
        let mut again = Cheats::new();
        assert!(again.load_text(&cheats.to_text()).is_empty());
        assert_eq!(again.list()[0].name, "Infinite lives");
        assert!(!again.list()[1].enabled);
    }

    #[test]
    fn memory_search_narrows_down() {
        let mut ram = [0u8; 0x10000];
        ram[0xC123] = 3; // lives
        ram[0xC200] = 3; // something else that stays 3
        let mut s = MemorySearch::start(|a| ram[a as usize]);
        assert_eq!(s.filter(Filter::Equal(3), |a| ram[a as usize]), 2);
        ram[0xC123] = 2;
        assert_eq!(s.filter(Filter::Decreased, |a| ram[a as usize]), 1);
        assert_eq!(s.candidates(), &[(0xC123, 2)]);
    }
}
