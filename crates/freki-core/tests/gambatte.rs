//! Runs Gambatte's hardware tests (`test/hwtests` of gambatte-core): about 3,500 small
//! ROMs, each verified on a DMG (DMG-CPU-08) and/or a Game Boy Color (CPU CGB 04C).
//!
//! A test's file name says what it shows after 15 frames: `dmg08_cgb04c_outXY` (both
//! models), `dmg08_outXY_cgb04c_outZW` (each model) or `_outXY` (CGB only), where `XY` are
//! hex digits drawn as 8×8 tiles from the top-left corner. `outaudio0`/`outaudio1` tests
//! (sound off/on) and tests checked against a PNG screenshot are not run yet.
//!
//! `scripts/fetch-test-roms.sh` downloads the sources; they are assembled here (see
//! `gambatte/asm.rs`). If they are missing, the test passes. Tests freki is known to
//! fail are listed in `tests/gambatte_failures.txt`; the test fails if any other one
//! fails. Run with `FREKI_UPDATE_GAMBATTE=1` to rewrite that list, or with
//! `FREKI_GAMBATTE=<part of a path>` to run only matching tests and print what they show.

#[path = "gambatte/asm.rs"]
mod asm;
mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use freki_core::{GameBoy, Model, SCREEN_WIDTH, Screen};

/// Gambatte runs 15 frames without a boot ROM, then compares the screen.
const CYCLES: u64 = 15 * freki_core::CYCLES_PER_FRAME as u64;

const FAILURES: &str = "tests/gambatte_failures.txt";

/// One run: a ROM on one model, with the digits it should show.
struct Case {
    rom: PathBuf,
    model: Model,
    expected: String,
}

/// What a test name expects from `model`, if it is a digit test for that model.
fn expected(stem: &str, model: Model) -> Option<String> {
    let marker = match model {
        Model::Dmg if stem.contains("dmg08_cgb04c_out") => "dmg08_cgb04c_out",
        Model::Dmg if stem.contains("dmg08_out") => "dmg08_out",
        Model::Dmg => return None,
        Model::Cgb if stem.contains("dmg08_cgb04c_out") => "dmg08_cgb04c_out",
        Model::Cgb if stem.contains("cgb04c_out") => "cgb04c_out",
        Model::Cgb if stem.contains("dmg08_out") => return None,
        Model::Cgb if stem.contains("_out") => "_out",
        Model::Cgb => return None,
    };
    let rest = &stem[stem.find(marker)? + marker.len()..];
    if rest.starts_with("audio") {
        return None;
    }
    let digits: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    (!digits.is_empty()).then_some(digits)
}

fn find_roms(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_roms(&path, out);
        } else if path.extension().is_some_and(|e| e == "asm") {
            out.push(path);
        }
    }
}

/// The 8×8 digit glyphs the tests draw: `#` black, `.` white (row 0 is blank).
const GLYPHS: [[&str; 7]; 16] = [
    [
        ".#######", ".#.....#", ".#.....#", ".#.....#", ".#.....#", ".#.....#", ".#######",
    ],
    [
        "....#...", "....#...", "....#...", "....#...", "....#...", "....#...", "....#...",
    ],
    [
        ".#######", ".......#", ".......#", ".#######", ".#......", ".#......", ".#######",
    ],
    [
        ".#######", ".......#", ".......#", "..######", ".......#", ".......#", ".#######",
    ],
    [
        ".#.....#", ".#.....#", ".#.....#", ".#######", ".......#", ".......#", ".......#",
    ],
    [
        ".#######", ".#......", ".#......", ".######.", ".......#", ".......#", ".######.",
    ],
    [
        ".#######", ".#......", ".#......", ".#######", ".#.....#", ".#.....#", ".#######",
    ],
    [
        ".#######", ".......#", "......#.", ".....#..", "....#...", "...#....", "...#....",
    ],
    [
        "..#####.", ".#.....#", ".#.....#", "..#####.", ".#.....#", ".#.....#", "..#####.",
    ],
    [
        ".#######", ".#.....#", ".#.....#", ".#######", ".......#", ".......#", ".#######",
    ],
    [
        "....#...", "..#...#.", ".#.....#", ".#######", ".#.....#", ".#.....#", ".#.....#",
    ],
    [
        ".######.", ".#.....#", ".#.....#", ".######.", ".#.....#", ".#.....#", ".######.",
    ],
    [
        "..#####.", ".#.....#", ".#......", ".#......", ".#......", ".#.....#", "..#####.",
    ],
    [
        ".######.", ".#.....#", ".#.....#", ".#.....#", ".#.....#", ".#.....#", ".######.",
    ],
    [
        ".#######", ".#......", ".#......", ".#######", ".#......", ".#......", ".#######",
    ],
    [
        ".#######", ".#......", ".#......", ".#######", ".#......", ".#......", ".#......",
    ],
];

/// Each pixel of the screen as white (`Some(false)`), black (`Some(true)`) or neither.
fn black_and_white(gb: &GameBoy) -> Vec<Option<bool>> {
    match gb.screen() {
        Screen::Shades(shades) => shades
            .iter()
            .map(|&s| match s {
                0 => Some(false),
                3 => Some(true),
                _ => None,
            })
            .collect(),
        Screen::Rgb555(colors) => colors
            .iter()
            .map(|&c| match c & 0x7FFF {
                0x7FFF => Some(false),
                0 => Some(true),
                _ => None,
            })
            .collect(),
    }
}

/// Whether the screen shows `digits` from the top-left corner.
fn shows(pixels: &[Option<bool>], digits: &str) -> bool {
    digits
        .chars()
        .enumerate()
        .all(|(i, c)| shows_at(pixels, i, c.to_digit(16).expect("hex digit")))
}

/// Whether tile `i` of the top row shows hex digit `digit`.
fn shows_at(pixels: &[Option<bool>], i: usize, digit: u32) -> bool {
    let glyph = &GLYPHS[digit as usize];
    (0..8).all(|y| {
        (0..8).all(|x| {
            let black = y > 0 && glyph[y - 1].as_bytes()[x] == b'#';
            pixels[y * SCREEN_WIDTH + i * 8 + x] == Some(black)
        })
    })
}

/// The digits the screen shows from the top-left corner (`?` for an unknown tile).
fn read_digits(pixels: &[Option<bool>], count: usize) -> String {
    (0..count)
        .map(|i| {
            (0..16u32)
                .find(|&d| shows_at(pixels, i, d))
                .map_or('?', |d| {
                    char::from_digit(d, 16).unwrap().to_ascii_uppercase()
                })
        })
        .collect()
}

/// Runs a case; returns `true` if the screen shows the expected digits.
fn run(case: &Case) -> bool {
    let source = std::fs::read_to_string(&case.rom).expect("read test source");
    let rom = asm::assemble(&source).unwrap_or_else(|e| panic!("{}: {e}", case.rom.display()));
    let mut gb = GameBoy::with_model(rom, case.model).expect("ROM loads");
    let mut cycles = 0u64;
    let mut locked = false;
    while cycles < CYCLES {
        if locked {
            // An illegal opcode locked up the CPU; the rest of the hardware runs on.
            cycles += 4;
            gb.run_without_cpu(4);
            continue;
        }
        match gb.step() {
            Ok(c) => cycles += c as u64,
            Err(_) => locked = true,
        }
    }
    let pixels = black_and_white(&gb);
    let pass = shows(&pixels, &case.expected);
    if std::env::var_os("FREKI_GAMBATTE").is_some() {
        let shown = read_digits(&pixels, case.expected.len());
        println!(
            "{} {} expected {} shows {shown}",
            if pass { "ok  " } else { "FAIL" },
            case.rom.display(),
            case.expected
        );
    }
    pass
}

/// The name a case has in the failure list: ROM path below `hwtests/` and model.
fn case_name(case: &Case, root: &Path) -> String {
    let model = match case.model {
        Model::Dmg => "dmg",
        Model::Cgb => "cgb",
    };
    let rel = case.rom.strip_prefix(root).unwrap_or(&case.rom);
    format!("{model} {}", rel.display())
}

#[test]
fn gambatte_hwtests() {
    let root = common::test_dir().join("gambatte/hwtests");
    let mut roms = Vec::new();
    find_roms(&root, &mut roms);
    if roms.is_empty() {
        eprintln!("skipping: no Gambatte hwtests (run scripts/fetch-test-roms.sh)");
        return;
    }
    roms.sort();
    if let Ok(filter) = std::env::var("FREKI_GAMBATTE") {
        roms.retain(|r| r.to_string_lossy().contains(&filter));
    }
    let cases: Vec<Case> = roms
        .iter()
        .flat_map(|rom| {
            let stem = rom.file_stem().unwrap().to_string_lossy().to_string();
            [Model::Dmg, Model::Cgb]
                .into_iter()
                .filter_map(move |model| {
                    expected(&stem, model).map(|expected| Case {
                        rom: rom.clone(),
                        model,
                        expected,
                    })
                })
        })
        .collect();

    // Run the cases on all cores.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = cases.len().div_ceil(threads);
    let failed: BTreeSet<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = cases
            .chunks(chunk)
            .map(|part| {
                let root = &root;
                scope.spawn(move || {
                    part.iter()
                        .filter(|case| !run(case))
                        .map(|case| case_name(case, root))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("runner thread"))
            .collect()
    });

    let dmg = cases.iter().filter(|c| c.model == Model::Dmg).count();
    let cgb = cases.len() - dmg;
    let failed_dmg = failed.iter().filter(|n| n.starts_with("dmg")).count();
    println!(
        "Gambatte hwtests: DMG {}/{dmg}, CGB {}/{cgb} passed",
        dmg - failed_dmg,
        cgb - (failed.len() - failed_dmg)
    );

    if std::env::var_os("FREKI_GAMBATTE").is_some() {
        return;
    }
    let list = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FAILURES);
    if std::env::var_os("FREKI_UPDATE_GAMBATTE").is_some() {
        let text: String = failed.iter().map(|n| format!("{n}\n")).collect();
        std::fs::write(&list, text).expect("write failure list");
        return;
    }
    let known: BTreeSet<String> = std::fs::read_to_string(&list)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let new: Vec<&String> = failed.difference(&known).collect();
    let fixed = known.difference(&failed).count();
    if fixed > 0 {
        println!("{fixed} tests in {FAILURES} pass now (update it with FREKI_UPDATE_GAMBATTE=1)");
    }
    assert!(
        new.is_empty(),
        "{} tests fail that used to pass: {new:#?}",
        new.len()
    );
}
