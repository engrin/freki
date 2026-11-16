//! Runs Blargg's `cpu_instrs` test ROMs headless and checks their serial output.
//!
//! The ROMs are not in the repo. Fetch them with `scripts/fetch-test-roms.sh`;
//! if they are missing, these tests print a note and pass.

use std::path::PathBuf;

use freki_core::{CLOCK_HZ, GameBoy};

/// Emulated seconds a ROM may run before it counts as hung.
const TIMEOUT_SECONDS: u64 = 60;

fn rom_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../roms/test")
        .join(name)
}

fn run_blargg(name: &str) {
    let path = rom_path(name);
    let Ok(rom) = std::fs::read(&path) else {
        eprintln!("skipping {name}: not found (run scripts/fetch-test-roms.sh)");
        return;
    };
    let mut gb = GameBoy::new(rom).expect("ROM loads");

    let budget = CLOCK_HZ as u64 * TIMEOUT_SECONDS;
    let mut cycles = 0u64;
    let output = loop {
        cycles += gb.step().unwrap_or_else(|e| panic!("{name}: {e}")) as u64;
        // Checking every ~1/60 s is plenty; the output only grows a few bytes at a time.
        if cycles % 70_224 < 24 || cycles >= budget {
            let out = String::from_utf8_lossy(gb.serial_output()).into_owned();
            if out.contains("Passed") || out.contains("Failed") || cycles >= budget {
                break out;
            }
        }
    };

    assert!(
        output.contains("Passed"),
        "{name} did not pass after {:.1} emulated s. Serial output:\n{output}",
        cycles as f64 / CLOCK_HZ as f64
    );
}

macro_rules! blargg {
    ($($test:ident => $file:literal,)*) => {
        $(
            #[test]
            fn $test() {
                run_blargg(concat!("cpu_instrs/", $file));
            }
        )*
    };
}

blargg! {
    cpu_instrs_01_special => "01-special.gb",
    cpu_instrs_02_interrupts => "02-interrupts.gb",
    cpu_instrs_03_op_sp_hl => "03-op sp,hl.gb",
    cpu_instrs_04_op_r_imm => "04-op r,imm.gb",
    cpu_instrs_05_op_rp => "05-op rp.gb",
    cpu_instrs_06_ld_r_r => "06-ld r,r.gb",
    cpu_instrs_07_jr_jp_call_ret_rst => "07-jr,jp,call,ret,rst.gb",
    cpu_instrs_08_misc_instrs => "08-misc instrs.gb",
    cpu_instrs_09_op_r_r => "09-op r,r.gb",
    cpu_instrs_10_bit_ops => "10-bit ops.gb",
    cpu_instrs_11_op_a_hl => "11-op a,(hl).gb",
}

/// The combined ROM runs all 11 tests in a row (takes ~1 emulated minute).
#[test]
#[ignore = "slow; run with --ignored"]
fn cpu_instrs_all() {
    run_blargg("cpu_instrs.gb");
}
