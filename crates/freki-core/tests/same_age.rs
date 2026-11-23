//! Runs SameSuite (by Lior Halphon) and the AGE test ROMs (by Christoph Sprenger) headless.
//!
//! Both finish with `LD B,B` ($40) and pass when B, C, D, E, H, L hold the Fibonacci numbers
//! 3, 5, 8, 13, 21, 34. Some AGE tests are checked by a screenshot instead: `foo.gb` comes
//! with `foo-<devices>.png`.
//!
//! The AGE file names say which devices a test was verified on: `dmgC` runs on the Game Boy,
//! `cgbBCE` on the Game Boy Color, `ncmBC` on the Game Boy Color in non-CGB mode. A test
//! made for several revisions in separate ROMs (`ly-cgbE`, `ly-dmgC-cgbBC`) can pass only
//! for the revision freki emulates. SameSuite runs on the Game Boy Color, its `sgb/` tests
//! on the Super Game Boy.
//!
//! The ROMs come from `scripts/fetch-test-roms.sh`; without them the test passes. Known
//! failures are listed in `tests/same_age_failures.txt`, and the test fails if any other
//! case fails. `FREKI_UPDATE_SAME_AGE=1` rewrites the list; `FREKI_SAME_AGE=1` prints
//! every result.

mod common;

use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use freki_core::{CLOCK_HZ, GameBoy, Model, Screen, rgb555_to_rgb888};

const FAILURES: &str = "tests/same_age_failures.txt";
/// Emulated seconds a ROM may run before it counts as hung.
const TIMEOUT_SECONDS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Device {
    Dmg,
    Cgb,
    /// A Game Boy Color running a Game Boy game.
    NonCgbMode,
    Sgb,
}

struct Case {
    rom: PathBuf,
    device: Device,
    /// Reference screenshot; `None`: the registers tell.
    screenshot: Option<PathBuf>,
}

fn find(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

/// The devices named in an AGE file name's suffixes (`stat-mode-dmgC-cgbBC` → DMG, CGB).
fn devices(stem: &str) -> Vec<Device> {
    let mut out = Vec::new();
    for part in stem.split('-') {
        let device = if part.starts_with("dmg") {
            Device::Dmg
        } else if part.starts_with("cgb") {
            Device::Cgb
        } else if part.starts_with("ncm") {
            Device::NonCgbMode
        } else {
            continue;
        };
        if !out.contains(&device) {
            out.push(device);
        }
    }
    out
}

fn cases(root: &Path) -> Vec<Case> {
    let mut cases = Vec::new();
    let mut roms = Vec::new();
    find(&root.join("same-suite"), "gb", &mut roms);
    roms.sort();
    for rom in roms {
        let sgb = rom.components().any(|c| c.as_os_str() == "sgb");
        cases.push(Case {
            rom,
            device: if sgb { Device::Sgb } else { Device::Cgb },
            screenshot: None,
        });
    }
    let mut roms = Vec::new();
    find(&root.join("age-test-roms"), "gb", &mut roms);
    roms.sort();
    for rom in roms {
        let stem = rom.file_stem().unwrap().to_string_lossy().into_owned();
        let shots: Vec<PathBuf> = {
            let mut pngs = Vec::new();
            find(rom.parent().unwrap(), "png", &mut pngs);
            pngs.sort();
            pngs.into_iter()
                .filter(|p| {
                    let s = p.file_stem().unwrap().to_string_lossy();
                    s.strip_prefix(stem.as_str())
                        .and_then(|rest| rest.strip_prefix('-'))
                        // `m3-bg-scx-ds-cgbBCE.png` belongs to `m3-bg-scx-ds.gb`, not
                        // to `m3-bg-scx.gb`: the rest must be devices only.
                        .is_some_and(|rest| devices(rest).len() == rest.split('-').count())
                })
                .collect()
        };
        if shots.is_empty() {
            for device in devices(&stem) {
                cases.push(Case {
                    rom: rom.clone(),
                    device,
                    screenshot: None,
                });
            }
        } else {
            for shot in shots {
                let s = shot.file_stem().unwrap().to_string_lossy().into_owned();
                for device in devices(&s[stem.len()..]) {
                    cases.push(Case {
                        rom: rom.clone(),
                        device,
                        screenshot: Some(shot.clone()),
                    });
                }
            }
        }
    }
    cases
}

fn case_name(case: &Case, root: &Path) -> String {
    let device = match case.device {
        Device::Dmg => "dmg",
        Device::Cgb => "cgb",
        Device::NonCgbMode => "ncm",
        Device::Sgb => "sgb",
    };
    let file = case.screenshot.as_ref().unwrap_or(&case.rom);
    let rel = file.strip_prefix(root).unwrap_or(file);
    format!("{device} {}", rel.display())
}

/// The reference screenshot as RGB.
fn load_png(path: &Path) -> Vec<[u8; 3]> {
    let bytes = std::fs::read(path).expect("read reference");
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().expect("PNG header");
    let mut buf = vec![0; reader.output_buffer_size().expect("buffer size")];
    let info = reader.next_frame(&mut buf).expect("PNG data");
    let channels = info.color_type.samples();
    buf[..info.buffer_size()]
        .chunks_exact(channels)
        .map(|px| {
            if channels < 3 {
                [px[0]; 3]
            } else {
                [px[0], px[1], px[2]]
            }
        })
        .collect()
}

/// Darkness 0 (white) … 3 (black) of a colour AGE uses for non-CGB mode, where the boot
/// ROM's colours are `#FFFFFF #7BFF31 #0063C6 #000000` (background) and
/// `#FFFFFF #FF8484 #943939 #000000` (objects). freki shows grey without a boot ROM, so
/// shades are compared.
fn ncm_shade(rgb: [u8; 3]) -> Option<u8> {
    match rgb {
        [0xFF, 0xFF, 0xFF] => Some(0),
        [0x7B, 0xFF, 0x31] | [0xFF, 0x84, 0x84] => Some(1),
        [0x00, 0x63, 0xC6] | [0x94, 0x39, 0x39] => Some(2),
        [0, 0, 0] => Some(3),
        _ => None,
    }
}

/// Whether freki's screen matches the reference.
fn screen_matches(gb: &GameBoy, device: Device, reference: &[[u8; 3]]) -> bool {
    match gb.screen() {
        Screen::Shades(shades) => {
            let grey = |s: u8| [255 - s * 85; 3];
            shades.iter().zip(reference).all(|(&s, &r)| grey(s) == r)
        }
        Screen::Rgb555(colors) if device == Device::NonCgbMode => {
            // Darkness of freki's grey palette by brightness.
            let mut levels: Vec<u16> = colors.iter().map(|&c| c & 0x7FFF).collect();
            levels.sort_unstable_by_key(|&c| std::cmp::Reverse(rgb555_to_rgb888(c)[1]));
            levels.dedup();
            colors.iter().zip(reference).all(|(&c, &r)| {
                let rank = levels.iter().position(|&l| l == c & 0x7FFF).unwrap() as u8;
                // With fewer than four levels on screen, compare brightness order only
                // where the reference is unambiguous.
                ncm_shade(r).is_some_and(|s| levels.len() < 4 || s == rank)
            })
        }
        Screen::Rgb555(colors) => colors
            .iter()
            .zip(reference)
            .all(|(&c, &r)| rgb555_to_rgb888(c) == r),
        Screen::Sgb(_) => unreachable!("AGE runs on a DMG or CGB"),
    }
}

fn run(case: &Case) -> Result<(), String> {
    let rom = std::fs::read(&case.rom).expect("read ROM");
    let model = match case.device {
        Device::Dmg => Model::Dmg,
        Device::Cgb | Device::NonCgbMode => Model::Cgb,
        Device::Sgb => Model::Sgb,
    };
    let mut gb = GameBoy::with_model(rom, model).map_err(|e| e.to_string())?;
    let mut cycles = 0u64;
    let limit = TIMEOUT_SECONDS * CLOCK_HZ as u64;
    while cycles < limit {
        if gb.peek(gb.pc()) == 0x40 {
            let Some(shot) = &case.screenshot else {
                let r = gb.registers();
                let regs = [r.b, r.c, r.d, r.e, r.h, r.l];
                return if regs == [3, 5, 8, 13, 21, 34] {
                    Ok(())
                } else {
                    Err(format!("registers {regs:02X?}"))
                };
            };
            // Let the frame being drawn finish, then compare.
            gb.run_frame().map_err(|e| e.to_string())?;
            gb.run_frame().map_err(|e| e.to_string())?;
            return if screen_matches(&gb, case.device, &load_png(shot)) {
                Ok(())
            } else {
                Err("screen differs".into())
            };
        }
        cycles += gb.step().map_err(|e| e.to_string())? as u64;
    }
    Err(format!("timed out after {TIMEOUT_SECONDS} s"))
}

#[test]
fn same_suite_and_age() {
    let root = common::test_dir();
    let cases = cases(&root);
    if cases.is_empty() {
        eprintln!("skipping: SameSuite and AGE are missing (run scripts/fetch-test-roms.sh)");
        return;
    }
    let verbose = std::env::var_os("FREKI_SAME_AGE").is_some();
    let mut failed = BTreeSet::new();
    for case in &cases {
        let name = case_name(case, &root);
        let result = run(case);
        if verbose {
            match &result {
                Ok(()) => println!("ok   {name}"),
                Err(e) => println!("FAIL {name}: {e}"),
            }
        }
        if result.is_err() {
            failed.insert(name);
        }
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FAILURES);
    if std::env::var_os("FREKI_UPDATE_SAME_AGE").is_some() {
        let list: String = failed.iter().map(|n| format!("{n}\n")).collect();
        std::fs::write(&path, list).expect("write failure list");
    }
    let known: BTreeSet<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let same = |prefix: &str| {
        let total = cases
            .iter()
            .filter(|c| case_name(c, &root).contains(prefix))
            .count();
        let bad = failed.iter().filter(|n| n.contains(prefix)).count();
        format!("{} / {total}", total - bad)
    };
    println!(
        "SameSuite {}, AGE {}",
        same(" same-suite/"),
        same(" age-test-roms/")
    );
    let fixed: Vec<_> = known.difference(&failed).collect();
    if !fixed.is_empty() {
        println!(
            "{} tests in {FAILURES} pass now (update it with FREKI_UPDATE_SAME_AGE=1)",
            fixed.len()
        );
    }
    let new: Vec<_> = failed.difference(&known).collect();
    assert!(new.is_empty(), "new failures: {new:#?}");
}
