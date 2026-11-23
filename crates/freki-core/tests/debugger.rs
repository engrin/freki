//! Breakpoints, watchpoints and stepping on a running machine.

use freki_core::{Access, GameBoy, Stop, Watchpoint};

/// $0150: LD A,($C000); INC A; LD ($C000),A; JR -8 (counts in $C000).
fn gb() -> GameBoy {
    let mut rom = vec![0u8; 0x8000];
    rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
    rom[0x150..0x158].copy_from_slice(&[0xFA, 0x00, 0xC0, 0x3C, 0xEA, 0x00, 0xC0, 0x18]);
    rom[0x158] = 0xF6; // JR -10 back to $0150
    GameBoy::new(rom).unwrap()
}

#[test]
fn breakpoint_stops_before_the_instruction_and_resumes_past_it() {
    let mut gb = gb();
    gb.breakpoints_mut().pc.insert(0x0153);
    assert_eq!(
        gb.run_frame_checked().unwrap(),
        Some(Stop::Breakpoint { pc: 0x0153 })
    );
    assert_eq!(gb.pc(), 0x0153, "INC A not executed yet");
    let a = gb.registers().a;
    // Resuming runs INC A, then stops at the same breakpoint one loop later.
    assert_eq!(
        gb.run_frame_checked().unwrap(),
        Some(Stop::Breakpoint { pc: 0x0153 })
    );
    assert_eq!(gb.registers().a, a.wrapping_add(1));
}

#[test]
fn write_watchpoint_reports_instruction_and_value() {
    let mut gb = gb();
    gb.breakpoints_mut()
        .watches
        .push(Watchpoint::new(0xC000, 0xC000, false, true));
    let stop = gb.run_frame_checked().unwrap();
    let Some(Stop::Watch { pc, hit }) = stop else {
        panic!("expected a watch stop, got {stop:?}");
    };
    assert_eq!(pc, 0x0154, "LD ($C000),A");
    assert_eq!(hit.addr, 0xC000);
    assert_eq!(hit.access, Access::Write);
    assert_eq!(hit.value, gb.peek(0xC000));
}

#[test]
fn read_watchpoint_ignores_fetches_debugger_reads_and_cheats() {
    let mut gb = gb();
    // Watching the code itself must not trigger on instruction fetches.
    gb.breakpoints_mut()
        .watches
        .push(Watchpoint::new(0x0150, 0x0158, true, true));
    gb.peek(0x0150);
    gb.poke(0xC000, 5);
    gb.cheats_mut().add("010700C0", "").unwrap();
    assert_eq!(gb.run_frame_checked().unwrap(), None, "frame completes");

    gb.breakpoints_mut().watches.clear();
    gb.breakpoints_mut()
        .watches
        .push(Watchpoint::new(0xC000, 0xC000, true, false));
    let stop = gb.run_frame_checked().unwrap();
    let Some(Stop::Watch { pc, hit }) = stop else {
        panic!("expected a watch stop, got {stop:?}");
    };
    assert_eq!(pc, 0x0150, "LD A,($C000) reads the watched address");
    assert_eq!((hit.addr, hit.access), (0xC000, Access::Read));
    // The reported value is what the instruction loaded.
    assert_eq!(hit.value, gb.registers().a);
    assert_eq!(hit.value, gb.peek(0xC000));
}

#[test]
fn breakpoints_survive_loading_a_state_and_stepping_works() {
    let mut gb = gb();
    let state = gb.save_state();
    gb.breakpoints_mut().pc.insert(0x0150);
    gb.load_state(&state).unwrap();
    assert!(gb.breakpoints().pc.contains(&0x0150));
    // Single-stepping walks the loop instruction by instruction.
    let mut pcs = Vec::new();
    for _ in 0..12 {
        if gb.step_checked().unwrap().is_none() {
            pcs.push(gb.pc());
        }
    }
    assert!(pcs.windows(2).any(|w| w == [0x0150, 0x0153]), "{pcs:04X?}");
}
