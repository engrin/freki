//! Cheat codes and a memory search for finding new ones.
//!
//! - **GameShark** codes (`01VVLLHH`) write value `VV` to address `HHLL` once per frame.
//!   On the Game Boy Color, `9xVVLLHH` writes to work RAM bank `x` at $D000–$DFFF, whichever
//!   bank the game has selected.
//! - **Game Genie** codes (`ABC-DEF` or `ABC-DEF-GHI`) patch what the CPU reads from ROM.
//!   The 9-digit form only applies when the original byte equals the compare value, which
//!   keeps a patch from hitting other ROM banks mapped at the same address.
//!
//! One cheat can hold several codes joined with `+` (or, as in RetroArch's files, 8-digit
//! GameShark codes separated by spaces). Cheat files are read in freki's own format or in
//! RetroArch's (`cheats = N`, `cheatN_desc`, `cheatN_code`, `cheatN_enable`), and saved
//! back in the format they were read in.

use std::fmt;

/// A RAM location: an address, plus the work RAM bank for $D000–$DFFF on the Game Boy
/// Color (`None`: whichever bank is selected, or not banked).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    pub address: u16,
    pub wram_bank: Option<u8>,
}

impl Location {
    pub fn new(address: u16) -> Self {
        Self {
            address,
            wram_bank: None,
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad` honours width and alignment, e.g. `{location:>7}`.
        f.pad(&match self.wram_bank {
            Some(bank) => format!("{bank}:${:04X}", self.address),
            None => format!("${:04X}", self.address),
        })
    }
}

/// A parsed cheat code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    GameShark {
        location: Location,
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
    /// GameShark code type other than `01` and `90`–`97`.
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
                let wram_bank = match kind {
                    0x01 => None,
                    // Bank 0 cannot be mapped at $D000: selecting it gives bank 1.
                    0x90..=0x97 => Some((kind & 7).max(1)),
                    _ => return Err(CheatError::UnsupportedType(kind)),
                };
                Ok(Self::GameShark {
                    value: byte(2),
                    location: Location {
                        address: u16::from_le_bytes([byte(4), byte(6)]),
                        wram_bank,
                    },
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

    /// The GameShark code that writes `value` to `location`.
    pub fn gameshark(location: Location, value: u8) -> String {
        let [lo, hi] = location.address.to_le_bytes();
        let kind = location.wram_bank.map_or(0x01, |bank| 0x90 | bank);
        format!("{kind:02X}{value:02X}{lo:02X}{hi:02X}")
    }
}

/// A cheat as entered by the user.
#[derive(Debug, Clone)]
pub struct Cheat {
    /// One or more codes, applied together.
    pub codes: Vec<Code>,
    /// The codes as typed (joined with `+`), for display and saving.
    pub text: String,
    pub name: String,
    pub enabled: bool,
}

/// Parses a cheat's codes: `+`-separated, and within each part either one code or several
/// 8-digit GameShark codes separated by spaces. Returns the codes and normalised text.
pub fn parse_codes(text: &str) -> Result<(Vec<Code>, String), CheatError> {
    let mut codes = Vec::new();
    let mut parts = Vec::new();
    for part in text.split('+').map(str::trim) {
        let part = part.to_uppercase();
        match Code::parse(&part) {
            Ok(code) => {
                codes.push(code);
                parts.push(part);
            }
            Err(e) => {
                let words: Vec<&str> = part.split_whitespace().collect();
                if words.len() < 2 {
                    return Err(e);
                }
                for word in words {
                    codes.push(Code::parse(word).map_err(|_| e.clone())?);
                    parts.push(word.to_string());
                }
            }
        }
    }
    Ok((codes, parts.join("+")))
}

/// The file format the cheat list was read from, and is saved in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileFormat {
    /// One cheat per line: `[!]CODE [name]`.
    #[default]
    Freki,
    /// RetroArch's `.cht`: `cheats = N` and `cheatN_desc/code/enable = ...`.
    RetroArch,
}

/// A cheat whose code has a value left to fill in: `X` or `?` for a hex digit, `XX` or
/// `??` for a byte (e.g. `01??B6D7`, "MP Modifier"). [`Cheats::fill_template`] makes a
/// cheat from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    /// The code as written, with the placeholder.
    pub code: String,
    /// Hex digits to fill in: 1 (0–15) or 2 (0–255).
    pub digits: usize,
}

impl Template {
    /// `code` as a template: one run of 1 or 2 placeholder characters, and a valid code
    /// once filled.
    fn parse(code: &str, name: &str) -> Option<Self> {
        let code = code.trim().to_uppercase();
        let placeholder = |c: char| c == 'X' || c == '?';
        let start = code.find(placeholder)?;
        let digits = code[start..]
            .chars()
            .take_while(|&c| placeholder(c))
            .count();
        let rest = &code[start + digits..];
        if !(1..=2).contains(&digits) || rest.contains(placeholder) {
            return None;
        }
        let template = Self {
            name: name.trim().to_string(),
            code,
            digits,
        };
        parse_codes(&template.filled(0)).ok()?;
        Some(template)
    }

    /// The code with `value` in place of the placeholder.
    pub fn filled(&self, value: u8) -> String {
        let start = self.code.find(['X', '?']).unwrap_or(0);
        let hex = format!("{value:0width$X}", width = self.digits);
        format!(
            "{}{}{}",
            &self.code[..start],
            &hex[hex.len() - self.digits..],
            &self.code[start + self.digits..]
        )
    }

    /// The largest value that fits.
    pub fn max(&self) -> u8 {
        if self.digits == 1 { 0x0F } else { 0xFF }
    }
}

/// Whether a cheat file's code field holds no code at all but a note ("N/A", empty).
fn is_note(code: &str) -> bool {
    let code = code.trim();
    code.is_empty() || code.eq_ignore_ascii_case("n/a")
}

/// An entry of a RetroArch file whose code freki cannot use, kept so saving keeps it.
#[derive(Debug, Clone)]
struct Unparsed {
    name: String,
    code: String,
    enabled: bool,
}

/// The active cheat list. Not part of save states.
#[derive(Debug, Clone)]
pub struct Cheats {
    list: Vec<Cheat>,
    /// Master switch for all cheats.
    pub enabled: bool,
    format: FileFormat,
    unparsed: Vec<Unparsed>,
    /// Set when a cheat is added, removed or switched; see [`Cheats::take_changed`].
    changed: bool,
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
            format: FileFormat::Freki,
            unparsed: Vec::new(),
            changed: false,
        }
    }

    /// Parses and adds a cheat (enabled). Returns its index.
    pub fn add(&mut self, text: &str, name: &str) -> Result<usize, CheatError> {
        let (codes, text) = parse_codes(text)?;
        self.list.push(Cheat {
            codes,
            text,
            name: name.trim().to_string(),
            enabled: true,
        });
        self.changed = true;
        Ok(self.list.len() - 1)
    }

    pub fn remove(&mut self, index: usize) -> Option<Cheat> {
        let removed = (index < self.list.len()).then(|| self.list.remove(index));
        self.changed |= removed.is_some();
        removed
    }

    pub fn set_enabled(&mut self, index: usize, enabled: bool) -> bool {
        match self.list.get_mut(index) {
            Some(cheat) => {
                self.changed |= cheat.enabled != enabled;
                cheat.enabled = enabled;
                true
            }
            None => false,
        }
    }

    pub fn list(&self) -> &[Cheat] {
        &self.list
    }

    /// The cheats of the file that need a value filled in (see [`Template`]).
    pub fn templates(&self) -> Vec<Template> {
        self.unparsed
            .iter()
            .filter_map(|u| Template::parse(&u.code, &u.name))
            .collect()
    }

    /// Adds the cheat that template `index` (of [`Cheats::templates`]) makes with
    /// `value`, enabled and named "name = value". Returns its index in the list.
    pub fn fill_template(&mut self, index: usize, value: u8) -> Result<usize, CheatError> {
        let template = self
            .templates()
            .into_iter()
            .nth(index)
            .ok_or_else(|| CheatError::Format(format!("no template {index}")))?;
        if value > template.max() {
            return Err(CheatError::Format(format!(
                "{value} is too big for {} (0–{})",
                template.code,
                template.max()
            )));
        }
        let name = format!("{} = {value}", template.name);
        self.add(&template.filled(value), &name)
    }

    /// Removes all cheats and forgets the file format.
    pub fn clear(&mut self) {
        self.list.clear();
        self.unparsed.clear();
        self.format = FileFormat::Freki;
        self.changed = false;
    }

    /// Whether the list changed since the last call (or since it was loaded), so the
    /// frontend can write the cheat file.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    pub fn format(&self) -> FileFormat {
        self.format
    }

    fn active(&self) -> impl Iterator<Item = &Code> {
        self.list
            .iter()
            .filter(move |c| self.enabled && c.enabled)
            .flat_map(|c| &c.codes)
    }

    /// GameShark writes to apply this frame.
    pub fn frame_writes(&self) -> impl Iterator<Item = (Location, u8)> + '_ {
        self.active().filter_map(|c| match *c {
            Code::GameShark { location, value } => Some((location, value)),
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

    /// Loads cheats from text in freki's or RetroArch's format (see the module docs).
    /// In freki's: one code per line, optionally followed by a name; lines starting with
    /// `#` are comments, a leading `!` adds the cheat disabled.
    /// Returns the lines that could not be parsed.
    pub fn load_text(&mut self, text: &str) -> Vec<(usize, CheatError)> {
        let errors = self.load_any(text);
        self.changed = false;
        errors
    }

    fn load_any(&mut self, text: &str) -> Vec<(usize, CheatError)> {
        if text
            .lines()
            .any(|l| l.split_once('=').is_some_and(|(k, _)| k.trim() == "cheats"))
        {
            return self.load_retroarch(text);
        }
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

    /// Loads RetroArch's format. Entries are numbered from 0; each needs a `code`.
    fn load_retroarch(&mut self, text: &str) -> Vec<(usize, CheatError)> {
        use std::collections::BTreeMap;
        self.format = FileFormat::RetroArch;
        // index -> (desc, (line, code), enable)
        type Entry = (String, Option<(usize, String)>, bool);
        let mut entries: BTreeMap<usize, Entry> = BTreeMap::new();
        for (n, line) in text.lines().enumerate() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            let Some((index, field)) = key
                .trim()
                .strip_prefix("cheat")
                .and_then(|k| k.split_once('_'))
                .and_then(|(i, f)| Some((i.parse::<usize>().ok()?, f)))
            else {
                continue;
            };
            let entry = entries.entry(index).or_default();
            match field {
                "desc" => entry.0 = value.to_string(),
                "code" => entry.1 = Some((n + 1, value.to_string())),
                "enable" => entry.2 = value == "true",
                _ => {}
            }
        }
        let mut errors = Vec::new();
        for (name, code, enabled) in entries.into_values() {
            let Some((line, code)) = code else { continue };
            match self.add(&code, &name) {
                Ok(i) => self.list[i].enabled = enabled,
                Err(e) => {
                    // Templates and notes are kept without complaint.
                    if Template::parse(&code, &name).is_none() && !is_note(&code) {
                        errors.push((line, e));
                    }
                    self.unparsed.push(Unparsed {
                        name,
                        code,
                        enabled,
                    });
                }
            }
        }
        errors
    }

    /// The cheat list as text, in the format it was loaded from (see [`Cheats::load_text`]).
    pub fn to_text(&self) -> String {
        match self.format {
            FileFormat::Freki => self.to_freki_text(),
            FileFormat::RetroArch => self.to_retroarch_text(),
        }
    }

    fn to_retroarch_text(&self) -> String {
        let entries = self
            .list
            .iter()
            .map(|c| (c.name.as_str(), c.text.as_str(), c.enabled))
            .chain(
                self.unparsed
                    .iter()
                    .map(|u| (u.name.as_str(), u.code.as_str(), u.enabled)),
            );
        let mut out = format!("cheats = {}\n", self.list.len() + self.unparsed.len());
        for (i, (name, code, enabled)) in entries.enumerate() {
            let quote = |s: &str| s.replace('"', "'");
            out.push_str(&format!(
                "\ncheat{i}_desc = \"{}\"\ncheat{i}_code = \"{}\"\ncheat{i}_enable = {enabled}\n",
                quote(name),
                quote(code)
            ));
        }
        out
    }

    fn to_freki_text(&self) -> String {
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
    /// Candidate locations with the value seen at the last filter.
    candidates: Vec<(Location, u8)>,
}

/// RAM a search covers: cartridge RAM, work RAM and high RAM.
pub const SEARCH_RANGES: [std::ops::RangeInclusive<u16>; 3] =
    [0xA000..=0xBFFF, 0xC000..=0xDFFF, 0xFF80..=0xFFFE];

impl MemorySearch {
    /// Starts a search over all searchable RAM (unbanked), using `read` for current values.
    pub fn start(read: impl Fn(Location) -> u8) -> Self {
        Self::start_in(&SEARCH_RANGES, read)
    }

    /// Starts a search over the given address ranges (unbanked).
    pub fn start_in(
        ranges: &[std::ops::RangeInclusive<u16>],
        read: impl Fn(Location) -> u8,
    ) -> Self {
        Self::start_at(
            ranges.iter().flat_map(|r| r.clone()).map(Location::new),
            read,
        )
    }

    /// Starts a search over the given locations.
    pub fn start_at(
        locations: impl IntoIterator<Item = Location>,
        read: impl Fn(Location) -> u8,
    ) -> Self {
        let candidates = locations.into_iter().map(|l| (l, read(l))).collect();
        Self { candidates }
    }

    /// Keeps the candidates whose current value passes `filter`. Returns how many remain.
    pub fn filter(&mut self, filter: Filter, read: impl Fn(Location) -> u8) -> usize {
        self.candidates.retain_mut(|(location, last)| {
            let now = read(*location);
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

    pub fn candidates(&self) -> &[(Location, u8)] {
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
                location: Location::new(0xC110),
                value: 0x0A
            })
        );
        assert_eq!(Code::gameshark(Location::new(0xC110), 0x0A), "010A10C1");
        // 9x: work RAM bank x (GBC), as in 934204D2 = $42 to $D204 in bank 3.
        let banked = Location {
            address: 0xD204,
            wram_bank: Some(3),
        };
        assert_eq!(
            Code::parse("934204D2"),
            Ok(Code::GameShark {
                location: banked,
                value: 0x42
            })
        );
        assert_eq!(Code::gameshark(banked, 0x42), "934204D2");
        assert_eq!(banked.to_string(), "3:$D204");
        assert_eq!(
            Code::parse("820A10C1"),
            Err(CheatError::UnsupportedType(0x82))
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
    fn several_codes_in_one_cheat() {
        let (codes, text) = parse_codes("01ff4bca 01FF4CCA+00A-17B-C49").unwrap();
        assert_eq!(codes.len(), 3);
        assert_eq!(text, "01FF4BCA+01FF4CCA+00A-17B-C49");
        assert!(parse_codes("01FF4BCA 0XFF4CCA").is_err());
        let mut cheats = Cheats::new();
        cheats.add("014200C0+014301C0", "two").unwrap();
        assert_eq!(cheats.frame_writes().count(), 2);
        assert!(cheats.take_changed());
        assert!(!cheats.take_changed());
        cheats.set_enabled(0, true);
        assert!(!cheats.take_changed(), "already enabled");
        cheats.set_enabled(0, false);
        assert!(cheats.take_changed());
        cheats.load_text("010A10C1");
        assert!(!cheats.take_changed(), "loading is not a change");
    }

    #[test]
    fn retroarch_file_round_trip() {
        let file = "cheats = 3\n\ncheat0_desc = \"Infinite Money\"\ncheat0_code = \"01FF4BCA 01FF4CCA\"\ncheat0_enable = false\n\ncheat1_desc = \"Mario\"\ncheat1_code = \"ZZ-NOPE\"\ncheat1_enable = false\n\ncheat2_desc = \"Lives\"\ncheat2_code = \"3EA-17B\"\ncheat2_enable = true\n";
        let mut cheats = Cheats::new();
        let errors = cheats.load_text(file);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, 8, "line of the bad code");
        assert_eq!(cheats.format(), FileFormat::RetroArch);
        let list = cheats.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Infinite Money");
        assert_eq!(list[0].codes.len(), 2);
        assert!(!list[0].enabled);
        assert!(list[1].enabled);
        // Saved back in RetroArch's format, keeping the code freki cannot use.
        let saved = cheats.to_text();
        assert!(saved.starts_with("cheats = 3\n"), "{saved}");
        assert!(saved.contains("cheat2_code = \"ZZ-NOPE\""), "{saved}");
        let mut again = Cheats::new();
        assert_eq!(again.load_text(&saved).len(), 1);
        assert_eq!(again.list()[0].text, "01FF4BCA+01FF4CCA");
        assert_eq!(again.list()[1].name, "Lives");
    }

    #[test]
    fn templates_and_notes_are_kept_quietly() {
        let file = "cheats = 4\ncheat0_desc = \"MP Modifier\"\ncheat0_code = \"01??B6D7\"\n\
                    cheat1_desc = \"Items\"\ncheat1_code = \"0X0-64B-219\"\n\
                    cheat2_desc = \"Effects list\"\ncheat2_code = \"N/A\"\n\
                    cheat3_desc = \"Stage\"\ncheat3_code = \"3E4-F3D-081+XX4-F4D-4CE\"\n";
        let mut cheats = Cheats::new();
        assert!(
            cheats.load_text(file).is_empty(),
            "no errors for templates and notes"
        );
        let templates = cheats.templates();
        assert_eq!(templates.len(), 3);
        assert_eq!((templates[0].digits, templates[0].max()), (2, 255));
        assert_eq!((templates[1].digits, templates[1].max()), (1, 15));
        assert_eq!(templates[0].filled(99), "0163B6D7");
        assert_eq!(templates[1].filled(10), "0A0-64B-219");
        assert_eq!(templates[2].filled(0x21), "3E4-F3D-081+214-F4D-4CE");

        let i = cheats.fill_template(0, 99).unwrap();
        let cheat = &cheats.list()[i];
        assert_eq!(cheat.name, "MP Modifier = 99");
        assert!(cheat.enabled);
        assert_eq!(
            cheat.codes,
            [Code::GameShark {
                location: Location::new(0xD7B6),
                value: 99
            }]
        );
        assert!(cheats.fill_template(1, 16).is_err(), "one digit holds 0–15");
        // The templates stay in the file, the filled cheat is added.
        let saved = cheats.to_text();
        assert!(
            saved.contains("01??B6D7") && saved.contains("0163B6D7"),
            "{saved}"
        );
        assert!(saved.contains("N/A"), "{saved}");
    }

    #[test]
    fn memory_search_narrows_down() {
        let mut ram = [0u8; 0x10000];
        ram[0xC123] = 3; // lives
        ram[0xC200] = 3; // something else that stays 3
        let mut s = MemorySearch::start(|l| ram[l.address as usize]);
        assert_eq!(s.filter(Filter::Equal(3), |l| ram[l.address as usize]), 2);
        ram[0xC123] = 2;
        assert_eq!(s.filter(Filter::Decreased, |l| ram[l.address as usize]), 1);
        assert_eq!(s.candidates(), &[(Location::new(0xC123), 2)]);
    }
}
