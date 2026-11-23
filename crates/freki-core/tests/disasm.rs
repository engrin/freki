//! The disassembler must agree with the CPU on instruction lengths.

mod common;

use freki_core::GameBoy;

const INTERRUPT_VECTORS: [u16; 5] = [0x40, 0x48, 0x50, 0x58, 0x60];

#[test]
fn lengths_match_cpu_execution_on_cpu_instrs() {
    let Ok(rom) = std::fs::read(common::test_dir().join("cpu_instrs.gb")) else {
        eprintln!("skipping: cpu_instrs.gb not found (run scripts/fetch-test-roms.sh)");
        return;
    };
    let mut gb = GameBoy::new(rom).unwrap();
    let mut checked = 0u64;
    let mut seen = [false; 512];
    // Run the whole combined ROM (all 11 sub-tests) so nearly every opcode executes.
    for step in 0..200_000_000u64 {
        if step % 100_000 == 0 && String::from_utf8_lossy(gb.serial_output()).contains("Passed all")
        {
            break;
        }
        let pc = gb.pc();
        let halted = gb.halted();
        let ins = gb.disassemble(pc);
        gb.step().unwrap();
        if halted || ins.is_control_flow() {
            continue;
        }
        let expected = pc.wrapping_add(ins.len as u16);
        if gb.pc() != expected {
            assert!(
                INTERRUPT_VECTORS.contains(&gb.pc()),
                "${pc:04X} {} ({:02X?}): CPU went to ${:04X}, disassembler says ${expected:04X}",
                ins.text,
                ins.bytes,
                gb.pc()
            );
            continue;
        }
        let key = if ins.bytes[0] == 0xCB {
            256 + ins.bytes[1] as usize
        } else {
            ins.bytes[0] as usize
        };
        seen[key] = true;
        checked += 1;
    }
    let distinct = seen.iter().filter(|&&s| s).count();
    eprintln!("checked {checked} instructions, {distinct} distinct opcodes");
    assert!(distinct > 400, "only {distinct} distinct opcodes exercised");
}
