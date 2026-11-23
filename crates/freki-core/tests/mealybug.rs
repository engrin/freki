//! Runs Matt Currie's mealybug-tearoom-tests and compares the screen with DMG references.
//! They also run on the Game Boy Color, after its real boot ROM (which picks the colours
//! the CGB references show) if one is at `bios/gbc_bios.bin` (not included).
//!
//! These tests change PPU registers in the middle of a scanline, so they check that the
//! PPU draws pixel by pixel with the right timing. Each ROM signals that its screen is
//! ready by executing `LD B,B`; the next complete frame is compared with the reference.
//! On a mismatch, freki's screen is written to `target/<test>-actual.png`.

mod common;

use freki_core::{BootRom, GameBoy, Screen, rgb555_to_rgb888};

/// Frames a ROM may run before `LD B,B` counts as never reached.
const MAX_FRAMES: usize = 600;
const LD_B_B: u8 = 0x40;

fn run_mealybug(name: &str) {
    let dir = common::test_dir().join("mealybug");
    let (Ok(rom), true) = (
        std::fs::read(dir.join(format!("{name}.gb"))),
        dir.join(format!("{name}.png")).exists(),
    ) else {
        eprintln!("skipping {name}: not found (run scripts/fetch-test-roms.sh)");
        return;
    };
    let mut gb = GameBoy::new(rom).expect("ROM loads");

    let budget = MAX_FRAMES as u64 * freki_core::CYCLES_PER_FRAME as u64;
    let mut cycles = 0u64;
    while gb.peek(gb.pc()) != LD_B_B {
        cycles += gb.step().unwrap_or_else(|e| panic!("{name}: {e}")) as u64;
        assert!(cycles < budget, "{name}: never reached LD B,B");
    }
    gb.run_frame().expect("runs");
    gb.run_frame().expect("runs");

    let expected = common::load_reference(&dir.join(format!("{name}.png")));
    if let Err(msg) = common::compare(name, gb.framebuffer(), &expected) {
        panic!("{msg}");
    }
}

/// Runs a test on the CGB (DMG compatibility mode) and compares with a CGB C reference.
fn run_mealybug_cgb(name: &str) {
    let dir = common::test_dir().join("mealybug");
    let reference = dir.join("cgb").join(format!("{name}.png"));
    let (Ok(rom), true) = (
        std::fs::read(dir.join(format!("{name}.gb"))),
        reference.exists(),
    ) else {
        eprintln!("skipping {name}: not found (run scripts/fetch-test-roms.sh)");
        return;
    };
    let boot = common::test_dir().join("../../bios/gbc_bios.bin");
    let Ok(boot) = std::fs::read(&boot) else {
        eprintln!("skipping {name}: no CGB boot ROM at {}", boot.display());
        return;
    };
    let boot = BootRom::new(&boot).expect("CGB boot ROM");
    let mut gb = GameBoy::with_boot_rom(rom, boot).expect("ROM loads");

    let budget = (MAX_FRAMES as u64 + 300) * freki_core::CYCLES_PER_FRAME as u64;
    let mut cycles = 0u64;
    while gb.in_boot_rom() || gb.peek(gb.pc()) != LD_B_B {
        cycles += gb.step().unwrap_or_else(|e| panic!("{name}: {e}")) as u64;
        assert!(cycles < budget, "{name}: never reached LD B,B");
    }
    gb.run_frame().expect("runs");
    gb.run_frame().expect("runs");

    let Screen::Rgb555(screen) = gb.screen() else {
        panic!("expected a colour screen");
    };
    let actual: Vec<[u8; 3]> = screen.iter().map(|&c| rgb555_to_rgb888(c)).collect();
    let expected = common::load_rgb_reference(&reference);
    if let Err(msg) = common::compare_rgb(&format!("{name}-cgb"), &actual, &expected) {
        panic!("{msg}");
    }
}

macro_rules! mealybug_cgb {
    ($($(#[$attr:meta])* $test:ident: $name:ident,)*) => {
        $(
            #[test]
            $(#[$attr])*
            fn $test() {
                run_mealybug_cgb(stringify!($name));
            }
        )*
    };
}

macro_rules! mealybug {
    ($($(#[$attr:meta])* $test:ident,)*) => {
        $(
            #[test]
            $(#[$attr])*
            fn $test() {
                run_mealybug(stringify!($test));
            }
        )*
    };
}

mealybug! {
    m2_win_en_toggle,
    m3_bgp_change,
    m3_bgp_change_sprites,
    #[ignore = "4 pixels off: BG-off write at pixel 0 during a sprite stall"]
    m3_lcdc_bg_en_change,
    m3_lcdc_bg_map_change,
    m3_lcdc_obj_en_change,
    m3_lcdc_obj_en_change_variant,
    m3_lcdc_obj_size_change,
    m3_lcdc_obj_size_change_scx,
    m3_lcdc_tile_sel_change,
    m3_lcdc_tile_sel_win_change,
    m3_lcdc_win_en_change_multiple,
    #[ignore = "3 pixels off on 2 rows: window start edge cases (unit-specific on DMG)"]
    m3_lcdc_win_en_change_multiple_wx,
    m3_lcdc_win_map_change,
    m3_obp0_change,
    m3_scx_high_5_bits,
    m3_scx_low_3_bits,
    m3_scy_change,
    m3_window_timing,
    m3_window_timing_wx_0,
    m3_wx_4_change,
    m3_wx_4_change_sprites,
    m3_wx_5_change,
    m3_wx_6_change,
}

// The CGB references of m3_lcdc_win_en_change_multiple_wx and m3_wx_{4,5,6}_change are
// placeholders ("expected result not currently available"), so those tests are left out.
mealybug_cgb! {
    cgb_m2_win_en_toggle: m2_win_en_toggle,
    cgb_m3_bgp_change: m3_bgp_change,
    cgb_m3_bgp_change_sprites: m3_bgp_change_sprites,
    cgb_m3_lcdc_bg_en_change: m3_lcdc_bg_en_change,
    cgb_m3_lcdc_bg_en_change2: m3_lcdc_bg_en_change2,
    cgb_m3_lcdc_bg_map_change: m3_lcdc_bg_map_change,
    cgb_m3_lcdc_bg_map_change2: m3_lcdc_bg_map_change2,
    cgb_m3_lcdc_obj_en_change: m3_lcdc_obj_en_change,
    #[ignore = "64 pixels off: OBJ enable toggled during a sprite fetch"]
    cgb_m3_lcdc_obj_en_change_variant: m3_lcdc_obj_en_change_variant,
    cgb_m3_lcdc_obj_size_change: m3_lcdc_obj_size_change,
    #[ignore = "15 pixels off: OBJ size changed during a sprite fetch with fine scroll"]
    cgb_m3_lcdc_obj_size_change_scx: m3_lcdc_obj_size_change_scx,
    #[ignore = "tile data select changed mid-fetch mixes tile sets on the CGB"]
    cgb_m3_lcdc_tile_sel_change: m3_lcdc_tile_sel_change,
    #[ignore = "tile data select changed mid-fetch mixes tile sets on the CGB"]
    cgb_m3_lcdc_tile_sel_change2: m3_lcdc_tile_sel_change2,
    #[ignore = "tile data select changed mid-fetch mixes tile sets on the CGB"]
    cgb_m3_lcdc_tile_sel_win_change: m3_lcdc_tile_sel_win_change,
    #[ignore = "tile data select changed mid-fetch mixes tile sets on the CGB"]
    cgb_m3_lcdc_tile_sel_win_change2: m3_lcdc_tile_sel_win_change2,
    cgb_m3_lcdc_win_en_change_multiple: m3_lcdc_win_en_change_multiple,
    cgb_m3_lcdc_win_map_change: m3_lcdc_win_map_change,
    cgb_m3_lcdc_win_map_change2: m3_lcdc_win_map_change2,
    cgb_m3_obp0_change: m3_obp0_change,
    cgb_m3_scx_high_5_bits: m3_scx_high_5_bits,
    cgb_m3_scx_high_5_bits_change2: m3_scx_high_5_bits_change2,
    cgb_m3_scx_low_3_bits: m3_scx_low_3_bits,
    cgb_m3_scy_change: m3_scy_change,
    cgb_m3_scy_change2: m3_scy_change2,
    cgb_m3_window_timing: m3_window_timing,
    cgb_m3_window_timing_wx_0: m3_window_timing_wx_0,
    #[ignore = "10 pixels off: WX change next to sprites"]
    cgb_m3_wx_4_change_sprites: m3_wx_4_change_sprites,
}
