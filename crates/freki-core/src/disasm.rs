//! SM83 disassembler, decoding opcodes from their bit fields like the CPU does.

/// One disassembled instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    pub addr: u16,
    /// Length in bytes (1–3).
    pub len: u8,
    pub bytes: Vec<u8>,
    /// Assembly text, e.g. `LD A,($FF44)`.
    pub text: String,
}

impl Instruction {
    /// Whether the instruction can change PC other than by falling through
    /// (jumps, calls, returns, RST, HALT/STOP).
    pub fn is_control_flow(&self) -> bool {
        let op = self.bytes[0];
        matches!(
            op,
            0x10 | 0x18
                | 0x20
                | 0x28
                | 0x30
                | 0x38
                | 0x76
                | 0xC0
                | 0xC2
                | 0xC3
                | 0xC4
                | 0xC7
                | 0xC8
                | 0xC9
                | 0xCA
                | 0xCC
                | 0xCD
                | 0xCF
                | 0xD0
                | 0xD2
                | 0xD4
                | 0xD7
                | 0xD8
                | 0xD9
                | 0xDA
                | 0xDC
                | 0xDF
                | 0xE7
                | 0xE9
                | 0xEF
                | 0xF7
                | 0xFF
        )
    }

    /// Address the instruction jumps or calls to, if it is fixed.
    pub fn target(&self) -> Option<u16> {
        let op = self.bytes[0];
        let next = self.addr.wrapping_add(self.len as u16);
        match op {
            0x18 | 0x20 | 0x28 | 0x30 | 0x38 => {
                Some(next.wrapping_add_signed(self.bytes[1] as i8 as i16))
            }
            0xC2 | 0xC3 | 0xC4 | 0xCA | 0xCC | 0xCD | 0xD2 | 0xD4 | 0xDA | 0xDC => {
                Some(u16::from_le_bytes([self.bytes[1], self.bytes[2]]))
            }
            0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => Some((op & 0x38) as u16),
            _ => None,
        }
    }
}

const R8: [&str; 8] = ["B", "C", "D", "E", "H", "L", "(HL)", "A"];
const RP: [&str; 4] = ["BC", "DE", "HL", "SP"];
const RP2: [&str; 4] = ["BC", "DE", "HL", "AF"];
const CC: [&str; 4] = ["NZ", "Z", "NC", "C"];
const ALU: [&str; 8] = [
    "ADD A,", "ADC A,", "SUB ", "SBC A,", "AND ", "XOR ", "OR ", "CP ",
];
const ROT: [&str; 8] = ["RLC", "RRC", "RL", "RR", "SLA", "SRA", "SWAP", "SRL"];

/// Disassembles the instruction at `addr`, reading memory through `read`.
pub fn disassemble(addr: u16, read: impl Fn(u16) -> u8) -> Instruction {
    let op = read(addr);
    let b1 = read(addr.wrapping_add(1));
    let b2 = read(addr.wrapping_add(2));
    let n16 = u16::from_le_bytes([b1, b2]);
    let (x, y, z) = (op >> 6, (op >> 3) & 7, op & 7);
    let (p, q) = (y >> 1, y & 1);
    let jr = |base: u16| base.wrapping_add(2).wrapping_add_signed(b1 as i8 as i16);

    let (len, text): (u8, String) = match (x, z) {
        (0, 0) => match y {
            0 => (1, "NOP".into()),
            1 => (3, format!("LD (${n16:04X}),SP")),
            2 => (2, "STOP".into()),
            3 => (2, format!("JR ${:04X}", jr(addr))),
            _ => (2, format!("JR {},${:04X}", CC[(y - 4) as usize], jr(addr))),
        },
        (0, 1) if q == 0 => (3, format!("LD {},${n16:04X}", RP[p as usize])),
        (0, 1) => (1, format!("ADD HL,{}", RP[p as usize])),
        (0, 2) => {
            let mem = ["(BC)", "(DE)", "(HL+)", "(HL-)"][p as usize];
            if q == 0 {
                (1, format!("LD {mem},A"))
            } else {
                (1, format!("LD A,{mem}"))
            }
        }
        (0, 3) => (
            1,
            format!("{} {}", ["INC", "DEC"][q as usize], RP[p as usize]),
        ),
        (0, 4) => (1, format!("INC {}", R8[y as usize])),
        (0, 5) => (1, format!("DEC {}", R8[y as usize])),
        (0, 6) => (2, format!("LD {},${b1:02X}", R8[y as usize])),
        (0, 7) => (
            1,
            ["RLCA", "RRCA", "RLA", "RRA", "DAA", "CPL", "SCF", "CCF"][y as usize].into(),
        ),
        (1, _) if y == 6 && z == 6 => (1, "HALT".into()),
        (1, _) => (1, format!("LD {},{}", R8[y as usize], R8[z as usize])),
        (2, _) => (1, format!("{}{}", ALU[y as usize], R8[z as usize])),
        (3, 0) => match y {
            0..=3 => (1, format!("RET {}", CC[y as usize])),
            4 => (2, format!("LDH (${:04X}),A", 0xFF00 | b1 as u16)),
            5 => (2, format!("ADD SP,{}", b1 as i8)),
            6 => (2, format!("LDH A,(${:04X})", 0xFF00 | b1 as u16)),
            _ => (2, format!("LD HL,SP{:+}", b1 as i8)),
        },
        (3, 1) if q == 0 => (1, format!("POP {}", RP2[p as usize])),
        (3, 1) => (1, ["RET", "RETI", "JP HL", "LD SP,HL"][p as usize].into()),
        (3, 2) => match y {
            0..=3 => (3, format!("JP {},${n16:04X}", CC[y as usize])),
            4 => (1, "LD ($FF00+C),A".into()),
            5 => (3, format!("LD (${n16:04X}),A")),
            6 => (1, "LD A,($FF00+C)".into()),
            _ => (3, format!("LD A,(${n16:04X})")),
        },
        (3, 3) => match y {
            0 => (3, format!("JP ${n16:04X}")),
            1 => {
                let (cx, cy, cz) = (b1 >> 6, (b1 >> 3) & 7, b1 & 7);
                let reg = R8[cz as usize];
                let text = match cx {
                    0 => format!("{} {reg}", ROT[cy as usize]),
                    1 => format!("BIT {cy},{reg}"),
                    2 => format!("RES {cy},{reg}"),
                    _ => format!("SET {cy},{reg}"),
                };
                (2, text)
            }
            6 => (1, "DI".into()),
            7 => (1, "EI".into()),
            _ => (1, format!("DB ${op:02X} ; illegal")),
        },
        (3, 4) if y <= 3 => (3, format!("CALL {},${n16:04X}", CC[y as usize])),
        (3, 4) => (1, format!("DB ${op:02X} ; illegal")),
        (3, 5) if q == 0 => (1, format!("PUSH {}", RP2[p as usize])),
        (3, 5) if p == 0 => (3, format!("CALL ${n16:04X}")),
        (3, 5) => (1, format!("DB ${op:02X} ; illegal")),
        (3, 6) => (2, format!("{}${b1:02X}", ALU[y as usize])),
        (3, 7) => (1, format!("RST ${:02X}", y * 8)),
        _ => unreachable!("x and z are 2- and 3-bit fields"),
    };
    let bytes = [op, b1, b2][..len as usize].to_vec();
    Instruction {
        addr,
        len,
        bytes,
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dis(bytes: &[u8]) -> String {
        let mem = |a: u16| bytes.get(a as usize).copied().unwrap_or(0);
        disassemble(0, mem).text
    }

    #[test]
    fn decodes_representative_opcodes() {
        assert_eq!(dis(&[0x00]), "NOP");
        assert_eq!(dis(&[0x08, 0x34, 0x12]), "LD ($1234),SP");
        assert_eq!(dis(&[0x18, 0xFE]), "JR $0000");
        assert_eq!(dis(&[0x20, 0x05]), "JR NZ,$0007");
        assert_eq!(dis(&[0x21, 0x00, 0xC0]), "LD HL,$C000");
        assert_eq!(dis(&[0x22]), "LD (HL+),A");
        assert_eq!(dis(&[0x36, 0x42]), "LD (HL),$42");
        assert_eq!(dis(&[0x76]), "HALT");
        assert_eq!(dis(&[0x7E]), "LD A,(HL)");
        assert_eq!(dis(&[0xAF]), "XOR A");
        assert_eq!(dis(&[0xC6, 0x10]), "ADD A,$10");
        assert_eq!(dis(&[0xCB, 0x7C]), "BIT 7,H");
        assert_eq!(dis(&[0xCB, 0x37]), "SWAP A");
        assert_eq!(dis(&[0xE0, 0x44]), "LDH ($FF44),A");
        assert_eq!(dis(&[0xE8, 0xFE]), "ADD SP,-2");
        assert_eq!(dis(&[0xF8, 0x05]), "LD HL,SP+5");
        assert_eq!(dis(&[0xF5]), "PUSH AF");
        assert_eq!(dis(&[0xFF]), "RST $38");
        assert_eq!(dis(&[0xD3]), "DB $D3 ; illegal");
    }

    #[test]
    fn targets_of_jumps_calls_and_rst() {
        let mem = |bytes: &'static [u8]| move |a: u16| bytes.get(a as usize).copied().unwrap_or(0);
        assert_eq!(
            disassemble(0, mem(&[0xCD, 0x00, 0x40])).target(),
            Some(0x4000)
        );
        assert_eq!(disassemble(0, mem(&[0x38, 0xFC])).target(), Some(0xFFFE));
        assert_eq!(disassemble(0, mem(&[0xEF])).target(), Some(0x28));
        assert!(disassemble(0, mem(&[0xE9])).is_control_flow());
        assert!(!disassemble(0, mem(&[0x3E, 0])).is_control_flow());
    }

    #[test]
    fn every_opcode_has_a_length_of_one_to_three() {
        for op in 0..=255u8 {
            for cb in [0u8, 0x46, 0xFF] {
                let i = disassemble(0, |a| if a == 0 { op } else { cb });
                assert!((1..=3).contains(&i.len), "{op:02X}: {}", i.text);
                assert_eq!(i.bytes.len(), i.len as usize);
            }
        }
    }
}
