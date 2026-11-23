//! Save state round trips: loading a state and replaying must be fully deterministic.

mod common;

use freki_core::{GameBoy, Rewind, StateError};

fn rom(name: &str) -> Option<Vec<u8>> {
    std::fs::read(common::test_dir().join(name)).ok()
}

/// A tiny ROM-only cartridge that counts in a loop, so the test needs no downloads.
fn counting_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    // $0100: JP $0150; $0150: INC A; LD ($C000),A; JR -6
    rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
    rom[0x134..0x139].copy_from_slice(b"COUNT");
    rom[0x150..0x156].copy_from_slice(&[0x3C, 0xEA, 0x00, 0xC0, 0x18, 0xFA]);
    rom
}

fn run_frames(gb: &mut GameBoy, frames: usize) {
    for _ in 0..frames {
        gb.run_frame().expect("runs");
    }
}

/// Save, run N frames, load, run N frames again: the end states must be identical.
fn assert_deterministic_replay(rom: Vec<u8>, warmup: usize, frames: usize) {
    let mut gb = GameBoy::new(rom).expect("ROM loads");
    run_frames(&mut gb, warmup);
    // Save in the middle of a frame so mid-line PPU state is exercised too.
    for _ in 0..1234 {
        gb.step().expect("runs");
    }
    let saved = gb.save_state();

    run_frames(&mut gb, frames);
    let first = gb.save_state();
    let first_screen = gb.framebuffer().to_vec();

    gb.load_state(&saved).expect("state loads");
    run_frames(&mut gb, frames);
    assert_eq!(
        gb.framebuffer().to_vec(),
        first_screen,
        "screen differs after replay"
    );
    assert!(
        gb.save_state() == first,
        "machine state differs after replay"
    );
}

#[test]
fn replay_is_deterministic_without_test_roms() {
    assert_deterministic_replay(counting_rom(), 5, 30);
}

#[test]
fn replay_is_deterministic_on_cpu_instrs() {
    let Some(rom) = rom("cpu_instrs.gb") else {
        eprintln!("skipping: cpu_instrs.gb not found (run scripts/fetch-test-roms.sh)");
        return;
    };
    // cpu_instrs uses MBC1, the timer, interrupts and the serial port.
    assert_deterministic_replay(rom, 300, 300);
}

#[test]
fn replay_is_deterministic_with_sound() {
    let Some(rom) = rom("dmg_sound.gb") else {
        eprintln!("skipping: dmg_sound.gb not found (run scripts/fetch-test-roms.sh)");
        return;
    };
    assert_deterministic_replay(rom, 120, 120);
}

#[test]
fn state_from_another_game_is_rejected_and_nothing_changes() {
    let mut a = GameBoy::new(counting_rom()).unwrap();
    let mut other = counting_rom();
    other[0x134..0x139].copy_from_slice(b"OTHER");
    let b = GameBoy::new(other).unwrap();

    run_frames(&mut a, 3);
    let before = a.save_state();
    assert!(matches!(
        a.load_state(&b.save_state()),
        Err(StateError::OtherGame(title)) if title == "OTHER"
    ));
    assert!(a.save_state() == before);
}

#[test]
fn corrupt_state_is_rejected() {
    let mut gb = GameBoy::new(counting_rom()).unwrap();
    let mut state = gb.save_state();
    assert!(matches!(
        gb.load_state(b"not a state"),
        Err(StateError::NotAState | StateError::Corrupt(_))
    ));
    state.truncate(state.len() / 2);
    assert!(matches!(gb.load_state(&state), Err(StateError::Corrupt(_))));
}

#[test]
fn rewind_steps_back_through_frames() {
    let mut gb = GameBoy::new(counting_rom()).unwrap();
    let mut rewind = Rewind::new(10);
    let mut counters = Vec::new();
    for _ in 0..5 {
        rewind.push(gb.save_state());
        counters.push(gb.peek(0xC000));
        gb.run_frame().unwrap();
    }
    while let Some(state) = rewind.pop() {
        gb.load_state(&state).unwrap();
        assert_eq!(gb.peek(0xC000), counters.pop().unwrap());
    }
}
