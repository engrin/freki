//! Runs the SingleStepTests 65816 suite (`roms/test/65816/*.json`, from
//! `scripts/fetch-test-roms.sh`): for each opcode in native and emulation mode, one
//! instruction from a given state must end in the given state, with the same bus cycles.
//! Skipped if the tests have not been fetched.

mod common;

use std::collections::HashMap;

use freki_core::w65816::{Bus65816, W65816};
use serde_json::Value;

#[derive(Default)]
struct TestBus {
    ram: HashMap<u32, u8>,
    /// (address, value, write) of each memory access; `None` for internal cycles.
    cycles: Vec<Option<(u32, u8, bool)>>,
}

impl Bus65816 for TestBus {
    fn read(&mut self, addr: u32) -> u8 {
        let v = *self.ram.get(&addr).unwrap_or(&0);
        self.cycles.push(Some((addr, v, false)));
        v
    }
    fn write(&mut self, addr: u32, value: u8) {
        self.ram.insert(addr, value);
        self.cycles.push(Some((addr, value, true)));
    }
    fn idle(&mut self) {
        self.cycles.push(None);
    }
}

fn reg(state: &Value, name: &str) -> u64 {
    state[name].as_u64().unwrap()
}

fn load(state: &Value) -> W65816 {
    let e = reg(state, "e") != 0;
    let s = reg(state, "s") as u16;
    W65816 {
        a: reg(state, "a") as u16,
        x: reg(state, "x") as u16,
        y: reg(state, "y") as u16,
        // Emulation mode keeps the stack in page 1 (the tests' start states don't).
        s: if e { 0x0100 | (s & 0xFF) } else { s },
        d: reg(state, "d") as u16,
        dbr: reg(state, "dbr") as u8,
        pbr: reg(state, "pbr") as u8,
        pc: reg(state, "pc") as u16,
        p: reg(state, "p") as u8,
        e,
        ..W65816::default()
    }
}

/// Runs one test; returns why it failed.
fn run(test: &Value) -> Result<(), String> {
    let mut cpu = load(&test["initial"]);
    let mut bus = TestBus::default();
    for pair in test["initial"]["ram"].as_array().unwrap() {
        bus.ram.insert(
            pair[0].as_u64().unwrap() as u32,
            pair[1].as_u64().unwrap() as u8,
        );
    }
    let start = (cpu.pbr, cpu.pc);
    cpu.step(&mut bus);
    // MVN/MVP move one byte per step and repeat until the count runs out.
    let op = test["name"].as_str().unwrap_or("").get(..2).unwrap_or("");
    if op == "44" || op == "54" {
        while (cpu.pbr, cpu.pc) == start && bus.cycles.len() < 20_000 {
            cpu.step(&mut bus);
        }
    }
    let want = load(&test["final"]);
    let got = W65816 {
        waiting: false,
        stopped: false,
        ..cpu.clone()
    };
    if got != want {
        return Err(format!("registers: got {got:?}\n want {want:?}"));
    }
    for pair in test["final"]["ram"].as_array().unwrap() {
        let (a, v) = (
            pair[0].as_u64().unwrap() as u32,
            pair[1].as_u64().unwrap() as u8,
        );
        let have = *bus.ram.get(&a).unwrap_or(&0);
        if have != v {
            return Err(format!("ram[{a:06X}] = {have:02X}, want {v:02X}"));
        }
    }
    let cycles = test["cycles"].as_array().unwrap();
    if cycles.len() != bus.cycles.len() {
        return Err(format!(
            "{} cycles, want {}",
            bus.cycles.len(),
            cycles.len()
        ));
    }
    for (i, (c, mine)) in cycles.iter().zip(&bus.cycles).enumerate() {
        let flags = c[2].as_str().unwrap().as_bytes();
        let access = flags[0] == b'd' || flags[1] == b'p' || flags[2] == b'v';
        let write = flags[3] == b'w';
        match mine {
            None if access => return Err(format!("cycle {i}: idle, want an access")),
            Some(_) if !access => return Err(format!("cycle {i}: access, want idle")),
            Some((a, _, w)) if *a != c[0].as_u64().unwrap() as u32 || *w != write => {
                return Err(format!(
                    "cycle {i}: {} {a:06X}, want {} {:06X}",
                    if *w { "write" } else { "read" },
                    if write { "write" } else { "read" },
                    c[0].as_u64().unwrap()
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

#[test]
fn singlestep_65816() {
    let dir = common::test_dir().join("65816");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skipping: no 65816 tests (run scripts/fetch-test-roms.sh)");
        return;
    };
    let filter = std::env::var("FREKI_65816").ok();
    let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    files.sort();
    let mut failed = Vec::new();
    let mut total = 0;
    for file in files {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        if filter
            .as_ref()
            .is_some_and(|f| !name.starts_with(f.as_str()))
        {
            continue;
        }
        let tests: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let tests = tests.as_array().unwrap();
        total += tests.len();
        let errors: Vec<_> = tests
            .iter()
            .filter_map(|t| run(t).err().map(|e| (t["name"].clone(), e)))
            .collect();
        if let Some((test, why)) = errors.first() {
            failed.push(format!(
                "{name}: {} of {} fail, e.g. {test}: {why}",
                errors.len(),
                tests.len()
            ));
        }
    }
    println!("65816: {} files failing, {total} tests run", failed.len());
    assert!(failed.is_empty(), "\n{}", failed.join("\n"));
}
