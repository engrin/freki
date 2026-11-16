//! Runs Blargg's test ROMs headless and checks their results.
//!
//! Blargg's ROMs report in one of two ways, and both are checked:
//! - `cpu_instrs` prints "Passed"/"Failed" over the serial port.
//! - `dmg_sound` writes to cartridge RAM: signature $DE $B0 $61 at $A001–$A003,
//!   status at $A000 ($80 = running, 0 = passed, otherwise the failure code) and a
//!   zero-terminated text log at $A004.
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

enum Outcome {
    Passed,
    Failed(String),
    Running,
}

/// Result reported through cartridge RAM at $A000, if the ROM has finished.
fn memory_result(gb: &GameBoy) -> Outcome {
    if [gb.peek(0xA001), gb.peek(0xA002), gb.peek(0xA003)] != [0xDE, 0xB0, 0x61] {
        return Outcome::Running;
    }
    match gb.peek(0xA000) {
        0x80 => Outcome::Running,
        0x00 => Outcome::Passed,
        code => {
            let text: Vec<u8> = (0xA004..0xC000u16)
                .map(|a| gb.peek(a))
                .take_while(|&b| b != 0)
                .collect();
            Outcome::Failed(format!(
                "result code {code}\n{}",
                String::from_utf8_lossy(&text)
            ))
        }
    }
}

/// Result reported over the serial port, if the ROM has finished.
fn serial_result(gb: &GameBoy) -> Outcome {
    let out = String::from_utf8_lossy(gb.serial_output());
    if out.contains("Passed") {
        Outcome::Passed
    } else if out.contains("Failed") {
        Outcome::Failed(out.into_owned())
    } else {
        Outcome::Running
    }
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
    while cycles < budget {
        // Results only change a few times per second; check once per frame.
        gb.run_frame().unwrap_or_else(|e| panic!("{name}: {e}"));
        cycles += freki_core::CYCLES_PER_FRAME as u64;
        for outcome in [serial_result(&gb), memory_result(&gb)] {
            match outcome {
                Outcome::Passed => return,
                Outcome::Failed(log) => panic!("{name} failed:\n{log}"),
                Outcome::Running => {}
            }
        }
    }
    panic!(
        "{name} did not finish within {TIMEOUT_SECONDS} emulated s. Serial output:\n{}",
        String::from_utf8_lossy(gb.serial_output())
    );
}

macro_rules! blargg {
    ($dir:literal: $($(#[$attr:meta])* $test:ident => $file:literal,)*) => {
        $(
            #[test]
            $(#[$attr])*
            fn $test() {
                run_blargg(concat!($dir, "/", $file));
            }
        )*
    };
}

blargg! { "cpu_instrs":
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

blargg! { "dmg_sound":
    dmg_sound_01_registers => "01-registers.gb",
    dmg_sound_02_len_ctr => "02-len ctr.gb",
    dmg_sound_03_trigger => "03-trigger.gb",
    dmg_sound_04_sweep => "04-sweep.gb",
    dmg_sound_05_sweep_details => "05-sweep details.gb",
    dmg_sound_06_overflow_on_trigger => "06-overflow on trigger.gb",
    dmg_sound_07_len_sweep_period_sync => "07-len sweep period sync.gb",
    dmg_sound_08_len_ctr_during_power => "08-len ctr during power.gb",
    #[ignore = "needs M-cycle-accurate memory timing (wave RAM access while playing)"]
    dmg_sound_09_wave_read_while_on => "09-wave read while on.gb",
    #[ignore = "needs M-cycle-accurate memory timing (wave RAM access while playing)"]
    dmg_sound_10_wave_trigger_while_on => "10-wave trigger while on.gb",
    dmg_sound_11_regs_after_power => "11-regs after power.gb",
    #[ignore = "needs M-cycle-accurate memory timing (wave RAM access while playing)"]
    dmg_sound_12_wave_write_while_on => "12-wave write while on.gb",
}

/// The combined ROM runs all 11 tests in a row (takes ~1 emulated minute).
#[test]
#[ignore = "slow; run with --ignored"]
fn cpu_instrs_all() {
    run_blargg("cpu_instrs.gb");
}

/// The combined ROM runs all 12 sound tests in a row. Fails until 09, 10 and 12 pass.
#[test]
#[ignore = "includes the three wave-RAM timing tests that need M-cycle accuracy"]
fn dmg_sound_all() {
    run_blargg("dmg_sound.gb");
}
