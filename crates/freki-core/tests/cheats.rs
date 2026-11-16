//! Cheats applied to a running machine.

use freki_core::{Filter, GameBoy, Location, MemorySearch};

/// ROM-only cartridge: $0150: INC A; LD ($C000),A; JR -6 (counts in $C000).
fn counting_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
    rom[0x150..0x156].copy_from_slice(&[0x3C, 0xEA, 0x00, 0xC0, 0x18, 0xFA]);
    rom
}

/// Game Genie code (no compare) that puts `value` at ROM `address`.
fn game_genie(value: u8, address: u16) -> String {
    let d = |shift: u16| (address >> shift) & 0xF;
    format!(
        "{value:02X}{:X}-{:X}{:X}{:X}",
        d(8),
        d(4),
        d(0),
        (address >> 12) ^ 0xF
    )
}

#[test]
fn gameshark_holds_a_ram_value_every_frame() {
    let mut gb = GameBoy::new(counting_rom()).unwrap();
    gb.cheats_mut()
        .add("014200C0", "hold $C000 at $42")
        .unwrap();
    for _ in 0..3 {
        gb.run_frame().unwrap();
        assert_eq!(gb.peek(0xC000), 0x42);
    }
    gb.cheats_mut().enabled = false;
    gb.run_frame().unwrap();
    assert_ne!(gb.peek(0xC000), 0x42, "master switch off");
}

#[test]
fn game_genie_patches_rom_reads() {
    let mut gb = GameBoy::new(counting_rom()).unwrap();
    // Replace INC A at $0150 with NOP: the counter stays at its start value.
    let code = game_genie(0x00, 0x0150);
    gb.cheats_mut().add(&code, "no counting").unwrap();
    assert_eq!(gb.peek(0x0150), 0x00);
    // INC A never runs, so $C000 keeps receiving A's post-boot value, $01.
    for _ in 0..3 {
        gb.run_frame().unwrap();
        assert_eq!(gb.peek(0xC000), 0x01, "counter frozen by the patch");
    }
    gb.cheats_mut().set_enabled(0, false);
    gb.run_frame().unwrap();
    assert_ne!(gb.peek(0xC000), 0x01, "counts again without the patch");
}

#[test]
fn cheats_survive_loading_a_state() {
    let mut gb = GameBoy::new(counting_rom()).unwrap();
    let state = gb.save_state();
    gb.cheats_mut().add("014200C0", "").unwrap();
    gb.load_state(&state).unwrap();
    assert_eq!(gb.cheats().list().len(), 1);
    gb.run_frame().unwrap();
    assert_eq!(gb.peek(0xC000), 0x42);
}

#[test]
fn memory_search_finds_the_counter() {
    let mut gb = GameBoy::new(counting_rom()).unwrap();
    gb.run_frame().unwrap();
    let mut search = MemorySearch::start(|l| gb.peek_location(l));
    for _ in 0..4 {
        gb.run_frame().unwrap();
        search.filter(Filter::Changed, |l| gb.peek_location(l));
    }
    assert!(
        search
            .candidates()
            .iter()
            .any(|&(l, _)| l == Location::new(0xC000))
    );
    assert!(search.len() < 8, "{} candidates left", search.len());
}
