//! Runs Matt Currie's mealybug-tearoom-tests and compares the screen with DMG references.
//!
//! These tests change PPU registers in the middle of a scanline, so they check that the
//! PPU draws pixel by pixel with the right timing. Each ROM signals that its screen is
//! ready by executing `LD B,B`; the next complete frame is compared with the reference.
//! On a mismatch, freki's screen is written to `target/<test>-actual.png`.

mod common;

use freki_core::GameBoy;

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
    #[ignore = "not yet pixel-exact"]
    m2_win_en_toggle,
    #[ignore = "not yet pixel-exact"]
    m3_bgp_change,
    #[ignore = "not yet pixel-exact"]
    m3_bgp_change_sprites,
    #[ignore = "not yet pixel-exact"]
    m3_lcdc_bg_en_change,
    #[ignore = "not yet pixel-exact"]
    m3_lcdc_bg_map_change,
    #[ignore = "not yet pixel-exact"]
    m3_lcdc_obj_en_change,
    #[ignore = "not yet pixel-exact"]
    m3_lcdc_obj_en_change_variant,
    #[ignore = "not yet pixel-exact"]
    m3_lcdc_obj_size_change,
    #[ignore = "not yet pixel-exact"]
    m3_lcdc_obj_size_change_scx,
    #[ignore = "not yet pixel-exact"]
    m3_lcdc_tile_sel_change,
    #[ignore = "not yet pixel-exact"]
    m3_lcdc_tile_sel_win_change,
    m3_lcdc_win_en_change_multiple,
    #[ignore = "not yet pixel-exact"]
    m3_lcdc_win_en_change_multiple_wx,
    #[ignore = "not yet pixel-exact"]
    m3_lcdc_win_map_change,
    #[ignore = "not yet pixel-exact"]
    m3_obp0_change,
    m3_scx_high_5_bits,
    #[ignore = "not yet pixel-exact"]
    m3_scx_low_3_bits,
    #[ignore = "not yet pixel-exact"]
    m3_scy_change,
    #[ignore = "not yet pixel-exact"]
    m3_window_timing,
    #[ignore = "not yet pixel-exact"]
    m3_window_timing_wx_0,
    m3_wx_4_change,
    m3_wx_4_change_sprites,
    m3_wx_5_change,
    m3_wx_6_change,
}
