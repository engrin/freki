//! A small assembler for the syntax of Gambatte's hwtests (written for freki, so the
//! tests run without executing the Python assembler that ships with them).
//!
//! A file starts with an optional `.size XXXX` (ROM size in hex, default $8000), followed
//! by sections: `.text@XXXX` (instructions from that address, one per line, until the
//! first line that is not one) and `.data@XXXX` (lines of hex bytes until the first line
//! that is not). Numbers are hex without a prefix: `imm8` is two digits, `imm16` four.
//! Labels are `name:`; jumps take a label or a hex address. Whitespace is optional,
//! except between a jump mnemonic and its target. The Nintendo logo and the header and
//! ROM checksums are filled in like the original assembler does.

use std::collections::HashMap;

/// The instructions the tests use, in the assembler's spelling.
const OPS: &[(&str, u16)] = &[
    ("nop", 0x00),
    ("ld bc, imm16", 0x01),
    ("ld(bc), a", 0x02),
    ("inc bc", 0x03),
    ("inc b", 0x04),
    ("dec b", 0x05),
    ("ld b, imm8", 0x06),
    ("ld a, (bc)", 0x0A),
    ("inc c", 0x0C),
    ("dec c", 0x0D),
    ("ld c, imm8", 0x0E),
    ("rrca", 0x0F),
    ("stop, imm8", 0x10),
    ("ld de, imm16", 0x11),
    ("ld(de), a", 0x12),
    ("inc d", 0x14),
    ("dec d", 0x15),
    ("ld d, imm8", 0x16),
    ("jr target", 0x18),
    ("add hl, de", 0x19),
    ("ld a, (de)", 0x1A),
    ("inc e", 0x1C),
    ("dec e", 0x1D),
    ("ld e, imm8", 0x1E),
    ("jrnz target", 0x20),
    ("ld hl, imm16", 0x21),
    ("ld(hl++), a", 0x22),
    ("inc h", 0x24),
    ("dec h", 0x25),
    ("ld a, (hl++)", 0x2A),
    ("inc l", 0x2C),
    ("dec l", 0x2D),
    ("ld sp, imm16", 0x31),
    ("ld(hl--), a", 0x32),
    ("ld(hl), imm8", 0x36),
    ("add hl, sp", 0x39),
    ("ld a, (hl--)", 0x3A),
    ("inc a", 0x3C),
    ("dec a", 0x3D),
    ("ld a, imm8", 0x3E),
    ("ld b, a", 0x47),
    ("ld c, a", 0x4F),
    ("ld d, a", 0x57),
    ("ld e, a", 0x5F),
    ("halt", 0x76),
    ("ld(hl), a", 0x77),
    ("ld a, b", 0x78),
    ("ld a, c", 0x79),
    ("ld a, d", 0x7A),
    ("ld a, e", 0x7B),
    ("ld a, h", 0x7C),
    ("ld a, l", 0x7D),
    ("ld a, (hl)", 0x7E),
    ("add a, b", 0x80),
    ("add a, e", 0x83),
    ("sub a, b", 0x90),
    ("sub a, c", 0x91),
    ("sub a, d", 0x92),
    ("and a, b", 0xA0),
    ("and a, c", 0xA1),
    ("and a, d", 0xA2),
    ("xor a, a", 0xAF),
    ("or a, a", 0xB7),
    ("cmp a, b", 0xB8),
    ("cmp a, c", 0xB9),
    ("cmp a, d", 0xBA),
    ("cmp a, e", 0xBB),
    ("cmp a, h", 0xBC),
    ("retnz", 0xC0),
    ("pop bc", 0xC1),
    ("jpnz target", 0xC2),
    ("jp target", 0xC3),
    ("push bc", 0xC5),
    ("rst 00", 0xC7),
    ("ret", 0xC9),
    ("call target", 0xCD),
    ("pop de", 0xD1),
    ("push de", 0xD5),
    ("sub a, imm8", 0xD6),
    ("reti", 0xD9),
    ("ldff(imm8), a", 0xE0),
    ("pop hl", 0xE1),
    ("ldff(c), a", 0xE2),
    ("push hl", 0xE5),
    ("and a, imm8", 0xE6),
    ("ld(imm16), a", 0xEA),
    ("xor a, imm8", 0xEE),
    ("ldff a, (imm8)", 0xF0),
    ("pop af", 0xF1),
    ("ldff a, (c)", 0xF2),
    ("di", 0xF3),
    ("push af", 0xF5),
    ("ld a, (imm16)", 0xFA),
    ("ei", 0xFB),
    ("cmp a, imm8", 0xFE),
    ("sra a", 0xCB2F),
    ("swap a", 0xCB37),
    ("srl a", 0xCB3F),
];

const LOGO: [u8; 48] = [
    0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C, 0x00, 0x0D,
    0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD, 0xD9, 0x99,
    0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC, 0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
];

/// The operand of a parsed line.
enum Operand {
    None,
    Imm8(u8),
    Imm16(u16),
    /// A jump target: a label or a hex address.
    Target(String),
}

/// A parsed source line: a label definition, or an instruction.
enum Line {
    Label(String),
    Op(u16, Operand, Kind),
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Plain,
    Jr,
    Jp,
}

impl Line {
    fn size(&self) -> u16 {
        match self {
            Line::Label(_) => 0,
            Line::Op(code, operand, kind) => {
                let opcode = if *code > 0xFF { 2 } else { 1 };
                opcode
                    + match operand {
                        Operand::None => 0,
                        Operand::Imm8(_) => 1,
                        Operand::Imm16(_) => 2,
                        Operand::Target(_) if *kind == Kind::Jr => 1,
                        Operand::Target(_) => 2,
                    }
            }
        }
    }
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_word(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Parses one (trimmed) line, or `None` if it is not an instruction or label.
fn parse(line: &str) -> Option<Line> {
    if let Some(name) = line.strip_suffix(':') {
        let name = name.trim_end();
        if name.is_empty() || is_word(name) {
            return Some(Line::Label(name.to_owned()));
        }
    }
    // Jumps: mnemonic, whitespace, target.
    if let Some((mnemonic, target)) = line.split_once(char::is_whitespace) {
        let target = target.trim_start();
        for &(pattern, code) in OPS {
            if let Some(m) = pattern.strip_suffix(" target")
                && m == mnemonic
                && is_word(target)
            {
                let kind = if pattern.starts_with("jr") {
                    Kind::Jr
                } else {
                    Kind::Jp
                };
                return Some(Line::Op(code, Operand::Target(target.to_owned()), kind));
            }
        }
    }
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    for &(pattern, code) in OPS {
        if pattern.ends_with(" target") {
            continue;
        }
        let pattern: String = pattern.chars().filter(|c| !c.is_whitespace()).collect();
        for (placeholder, len) in [("imm16", 4), ("imm8", 2)] {
            if let Some((before, after)) = pattern.split_once(placeholder)
                && let Some(rest) = compact.strip_prefix(before)
                && let Some(value) = rest.strip_suffix(after)
                && is_hex(value, len)
            {
                let operand = if len == 2 {
                    Operand::Imm8(u8::from_str_radix(value, 16).ok()?)
                } else {
                    Operand::Imm16(u16::from_str_radix(value, 16).ok()?)
                };
                return Some(Line::Op(code, operand, Kind::Plain));
            }
        }
        if compact == pattern {
            return Some(Line::Op(code, Operand::None, Kind::Plain));
        }
    }
    None
}

/// Assembles a test's source into a ROM image.
pub fn assemble(source: &str) -> Result<Vec<u8>, String> {
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    let mut size = 0x8000;
    let mut start = 0;
    if let Some(i) = lines
        .iter()
        .position(|l| l.split_whitespace().next() == Some(".size"))
    {
        let value = lines[i]
            .split_whitespace()
            .nth(1)
            .ok_or(".size without value")?;
        size = usize::from_str_radix(value, 16).map_err(|e| format!(".size: {e}"))?;
        start = i + 1;
    }

    // Sections: (kind, address, parsed lines or data bytes).
    let mut texts: Vec<(usize, Vec<Line>)> = Vec::new();
    let mut data: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.is_empty() {
            continue;
        }
        let (directive, addr) = line
            .split_once('@')
            .map(|(d, a)| (d.trim(), a.trim()))
            .ok_or_else(|| format!("line {i}: {line}"))?;
        let addr = usize::from_str_radix(addr, 16).map_err(|e| format!("line {i}: {e}"))?;
        match directive {
            ".text" => {
                let mut ops = Vec::new();
                while let Some(op) = lines.get(i).and_then(|l| parse(l)) {
                    ops.push(op);
                    i += 1;
                }
                texts.push((addr, ops));
            }
            ".data" => {
                let mut bytes = Vec::new();
                while let Some(line) = lines.get(i) {
                    let values: Result<Vec<u8>, _> = line
                        .split_whitespace()
                        .map(|b| u8::from_str_radix(b, 16))
                        .collect();
                    let Ok(values) = values else { break };
                    bytes.extend(values);
                    i += 1;
                }
                data.push((addr, bytes));
            }
            _ => return Err(format!("line {i}: {line}")),
        }
    }

    // Label addresses.
    let mut labels = HashMap::new();
    for (addr, ops) in &texts {
        let mut pc = *addr;
        for op in ops {
            if let Line::Label(name) = op {
                labels.insert(name.clone(), pc);
            }
            pc += op.size() as usize;
        }
    }

    let mut rom = vec![0u8; size];
    let mut put = |at: usize, byte: u8| -> Result<(), String> {
        *rom.get_mut(at)
            .ok_or_else(|| format!("${at:X} is outside the ROM"))? = byte;
        Ok(())
    };
    for (addr, ops) in &texts {
        let mut pc = *addr;
        for op in ops {
            let Line::Op(code, operand, kind) = op else {
                continue;
            };
            let mut at = pc;
            if *code > 0xFF {
                put(at, (*code >> 8) as u8)?;
                at += 1;
            }
            put(at, *code as u8)?;
            at += 1;
            match operand {
                Operand::None => {}
                Operand::Imm8(v) => put(at, *v)?,
                Operand::Imm16(v) => {
                    put(at, *v as u8)?;
                    put(at + 1, (*v >> 8) as u8)?;
                }
                Operand::Target(name) => {
                    let target = labels.get(name).copied();
                    if *kind == Kind::Jr {
                        let offset = match target {
                            Some(t) => t as i64 - (at as i64 + 1),
                            None => i64::from_str_radix(name, 16).map_err(|e| e.to_string())?,
                        };
                        if !(-0x80..=0x7F).contains(&offset) {
                            return Err(format!("jr to {name} is too far"));
                        }
                        put(at, offset as u8)?;
                    } else {
                        let t = match target {
                            Some(t) if t >= 0x8000 => 0x4000 | (t & 0x3FFF),
                            Some(t) => t,
                            None => usize::from_str_radix(name, 16).map_err(|e| e.to_string())?,
                        };
                        put(at, t as u8)?;
                        put(at + 1, (t >> 8) as u8)?;
                    }
                }
            }
            pc += op.size() as usize;
        }
    }
    for (addr, bytes) in &data {
        for (k, &b) in bytes.iter().enumerate() {
            put(addr + k, b)?;
        }
    }

    rom[0x104..0x134].copy_from_slice(&LOGO);
    rom[0x14D] = rom[0x134..0x14D]
        .iter()
        .fold(0u8, |sum, &b| sum.wrapping_sub(b).wrapping_sub(1));
    let sum = rom.iter().fold(0u16, |sum, &b| sum.wrapping_add(b as u16));
    rom[0x14E] = (sum >> 8) as u8;
    rom[0x14F] = sum as u8;
    Ok(rom)
}
