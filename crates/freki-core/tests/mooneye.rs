//! Runs the Mooneye Test Suite (acceptance and emulator-only MBC tests) headless.
//!
//! A Mooneye ROM finishes by executing `LD B,B` ($40). It passed if B, C, D, E, H, L
//! then hold the Fibonacci numbers 3, 5, 8, 13, 21, 34; it failed if they hold $42.
//!
//! Only tests that apply to a DMG (rev. A–C) are listed. The ROMs are not in the repo;
//! fetch them with `scripts/fetch-test-roms.sh`. If they are missing, the tests pass.

use std::path::PathBuf;

use freki_core::{CLOCK_HZ, GameBoy};

/// Emulated seconds a ROM may run before it counts as hung.
const TIMEOUT_SECONDS: u64 = 20;

fn run(path: &str) {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../roms/test/mooneye")
        .join(path);
    let Ok(rom) = std::fs::read(&full) else {
        eprintln!(
            "skipping: {} is missing (run scripts/fetch-test-roms.sh)",
            full.display()
        );
        return;
    };
    let mut gb = GameBoy::new(rom).expect("load ROM");
    let mut cycles = 0u64;
    while cycles < TIMEOUT_SECONDS * CLOCK_HZ as u64 {
        if gb.peek(gb.pc()) == 0x40 {
            let r = gb.registers();
            let regs = [r.b, r.c, r.d, r.e, r.h, r.l];
            assert_eq!(
                regs,
                [3, 5, 8, 13, 21, 34],
                "{path} failed (registers {regs:02X?})"
            );
            return;
        }
        cycles += gb.step().expect("CPU error") as u64;
    }
    panic!("{path} timed out after {TIMEOUT_SECONDS} emulated seconds");
}

macro_rules! mooneye {
    ($($(#[$attr:meta])* $name:ident: $path:literal,)*) => {
        $(
            #[test]
            $(#[$attr])*
            fn $name() {
                run($path);
            }
        )*
    };
}

mooneye! {
    add_sp_e_timing: "acceptance/add_sp_e_timing.gb",
    bits_mem_oam: "acceptance/bits/mem_oam.gb",
    bits_reg_f: "acceptance/bits/reg_f.gb",
    bits_unused_hwio_gs: "acceptance/bits/unused_hwio-GS.gb",
    boot_div_dmgabcmgb: "acceptance/boot_div-dmgABCmgb.gb",
    boot_hwio_dmgabcmgb: "acceptance/boot_hwio-dmgABCmgb.gb",
    boot_regs_dmgabc: "acceptance/boot_regs-dmgABC.gb",
    call_cc_timing: "acceptance/call_cc_timing.gb",
    call_cc_timing2: "acceptance/call_cc_timing2.gb",
    call_timing: "acceptance/call_timing.gb",
    call_timing2: "acceptance/call_timing2.gb",
    di_timing_gs: "acceptance/di_timing-GS.gb",
    div_timing: "acceptance/div_timing.gb",
    ei_sequence: "acceptance/ei_sequence.gb",
    ei_timing: "acceptance/ei_timing.gb",
    halt_ime0_ei: "acceptance/halt_ime0_ei.gb",
    halt_ime0_nointr_timing: "acceptance/halt_ime0_nointr_timing.gb",
    halt_ime1_timing: "acceptance/halt_ime1_timing.gb",
    halt_ime1_timing2_gs: "acceptance/halt_ime1_timing2-GS.gb",
    if_ie_registers: "acceptance/if_ie_registers.gb",
    instr_daa: "acceptance/instr/daa.gb",
    interrupts_ie_push: "acceptance/interrupts/ie_push.gb",
    intr_timing: "acceptance/intr_timing.gb",
    jp_cc_timing: "acceptance/jp_cc_timing.gb",
    jp_timing: "acceptance/jp_timing.gb",
    ld_hl_sp_e_timing: "acceptance/ld_hl_sp_e_timing.gb",
    oam_dma_basic: "acceptance/oam_dma/basic.gb",
    oam_dma_reg_read: "acceptance/oam_dma/reg_read.gb",
    oam_dma_sources_gs: "acceptance/oam_dma/sources-GS.gb",
    oam_dma_restart: "acceptance/oam_dma_restart.gb",
    oam_dma_start: "acceptance/oam_dma_start.gb",
    oam_dma_timing: "acceptance/oam_dma_timing.gb",
    pop_timing: "acceptance/pop_timing.gb",
    ppu_hblank_ly_scx_timing_gs: "acceptance/ppu/hblank_ly_scx_timing-GS.gb",
    ppu_intr_1_2_timing_gs: "acceptance/ppu/intr_1_2_timing-GS.gb",
    ppu_intr_2_0_timing: "acceptance/ppu/intr_2_0_timing.gb",
    ppu_intr_2_mode0_timing: "acceptance/ppu/intr_2_mode0_timing.gb",
    #[ignore = "not passing yet"]
    ppu_intr_2_mode0_timing_sprites: "acceptance/ppu/intr_2_mode0_timing_sprites.gb",
    ppu_intr_2_mode3_timing: "acceptance/ppu/intr_2_mode3_timing.gb",
    ppu_intr_2_oam_ok_timing: "acceptance/ppu/intr_2_oam_ok_timing.gb",
    ppu_lcdon_timing_gs: "acceptance/ppu/lcdon_timing-GS.gb",
    ppu_lcdon_write_timing_gs: "acceptance/ppu/lcdon_write_timing-GS.gb",
    ppu_stat_irq_blocking: "acceptance/ppu/stat_irq_blocking.gb",
    ppu_stat_lyc_onoff: "acceptance/ppu/stat_lyc_onoff.gb",
    ppu_vblank_stat_intr_gs: "acceptance/ppu/vblank_stat_intr-GS.gb",
    push_timing: "acceptance/push_timing.gb",
    rapid_di_ei: "acceptance/rapid_di_ei.gb",
    ret_cc_timing: "acceptance/ret_cc_timing.gb",
    ret_timing: "acceptance/ret_timing.gb",
    reti_intr_timing: "acceptance/reti_intr_timing.gb",
    reti_timing: "acceptance/reti_timing.gb",
    rst_timing: "acceptance/rst_timing.gb",
    serial_boot_sclk_align_dmgabcmgb: "acceptance/serial/boot_sclk_align-dmgABCmgb.gb",
    timer_div_write: "acceptance/timer/div_write.gb",
    timer_rapid_toggle: "acceptance/timer/rapid_toggle.gb",
    timer_tim00: "acceptance/timer/tim00.gb",
    timer_tim00_div_trigger: "acceptance/timer/tim00_div_trigger.gb",
    timer_tim01: "acceptance/timer/tim01.gb",
    timer_tim01_div_trigger: "acceptance/timer/tim01_div_trigger.gb",
    timer_tim10: "acceptance/timer/tim10.gb",
    timer_tim10_div_trigger: "acceptance/timer/tim10_div_trigger.gb",
    timer_tim11: "acceptance/timer/tim11.gb",
    timer_tim11_div_trigger: "acceptance/timer/tim11_div_trigger.gb",
    timer_tima_reload: "acceptance/timer/tima_reload.gb",
    timer_tima_write_reloading: "acceptance/timer/tima_write_reloading.gb",
    timer_tma_write_reloading: "acceptance/timer/tma_write_reloading.gb",
    mbc1_bits_bank1: "emulator-only/mbc1/bits_bank1.gb",
    mbc1_bits_bank2: "emulator-only/mbc1/bits_bank2.gb",
    mbc1_bits_mode: "emulator-only/mbc1/bits_mode.gb",
    mbc1_bits_ramg: "emulator-only/mbc1/bits_ramg.gb",
    mbc1_multicart_rom_8mb: "emulator-only/mbc1/multicart_rom_8Mb.gb",
    mbc1_ram_256kb: "emulator-only/mbc1/ram_256kb.gb",
    mbc1_ram_64kb: "emulator-only/mbc1/ram_64kb.gb",
    mbc1_rom_16mb: "emulator-only/mbc1/rom_16Mb.gb",
    mbc1_rom_1mb: "emulator-only/mbc1/rom_1Mb.gb",
    mbc1_rom_2mb: "emulator-only/mbc1/rom_2Mb.gb",
    mbc1_rom_4mb: "emulator-only/mbc1/rom_4Mb.gb",
    mbc1_rom_512kb: "emulator-only/mbc1/rom_512kb.gb",
    mbc1_rom_8mb: "emulator-only/mbc1/rom_8Mb.gb",
    mbc2_bits_ramg: "emulator-only/mbc2/bits_ramg.gb",
    mbc2_bits_romb: "emulator-only/mbc2/bits_romb.gb",
    mbc2_bits_unused: "emulator-only/mbc2/bits_unused.gb",
    mbc2_ram: "emulator-only/mbc2/ram.gb",
    mbc2_rom_1mb: "emulator-only/mbc2/rom_1Mb.gb",
    mbc2_rom_2mb: "emulator-only/mbc2/rom_2Mb.gb",
    mbc2_rom_512kb: "emulator-only/mbc2/rom_512kb.gb",
    mbc5_rom_16mb: "emulator-only/mbc5/rom_16Mb.gb",
    mbc5_rom_1mb: "emulator-only/mbc5/rom_1Mb.gb",
    mbc5_rom_2mb: "emulator-only/mbc5/rom_2Mb.gb",
    mbc5_rom_32mb: "emulator-only/mbc5/rom_32Mb.gb",
    mbc5_rom_4mb: "emulator-only/mbc5/rom_4Mb.gb",
    mbc5_rom_512kb: "emulator-only/mbc5/rom_512kb.gb",
    mbc5_rom_64mb: "emulator-only/mbc5/rom_64Mb.gb",
    mbc5_rom_8mb: "emulator-only/mbc5/rom_8Mb.gb",
}
