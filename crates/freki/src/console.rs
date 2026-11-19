//! Terminal console for cheats and memory inspection, typed while the game runs.
//!
//! A background thread reads lines from stdin; the main loop executes them between
//! frames with [`Console::execute`]. Type `help` for the command list.

use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use freki_core::{Code, Filter, GameBoy, MemorySearch};

const HELP: &str = "\
cheats:
  cheat list                   show cheats (also: cheats)
  cheat add CODE [name]        add a GameShark (01VVLLHH, GBC: 9xVVLLHH), Game Genie (ABC-DEF[-GHI])
                               or Xploder ($0DVVLLHH) code; with ?? (01??2CA2) a template
  cheat edit N CODE [name]     change cheat N's code and name
  cheat on|off|rm N            enable, disable or remove cheat N
  cheat fill T VALUE           add a cheat from template T (cheat list shows them), e.g. 99
  cheat all on|off             master switch (also F9 in the window)
  cheat save | reload          write / re-read the .cht file next to the ROM
memory search (find the address of lives, money, ...):
  search start                 remember all RAM values
  search eq|ne N               keep addresses that now equal / differ from N
  search changed|same          keep addresses that changed / stayed the same
  search up|down               keep addresses that increased / decreased
  search list [N]              show up to N candidates (default 20)
  search add I [VALUE] [name]  turn candidate I into a cheat (default: its current value)
memory:
  peek ADDR [COUNT]            show memory
  poke ADDR VALUE              write memory once
debugger (also View → Debugger):
  regs                         show CPU registers
  dis [ADDR] [N]               disassemble N instructions (default: at PC, 10)
  step [N]                     execute N instructions (pause first)
  break ADDR | unbreak ADDR    set / remove a breakpoint
  watch ADDR [END] [r|w|rw]    stop when the CPU reads/writes ADDR..END (default w)
  unwatch N | breaks           remove watchpoint N / list all
  labels [TEXT]                list the symbol file's labels (containing TEXT)
other:
  pause | resume               pause emulation (also P in the window)
  help
numbers: decimal, or hex with $ or 0x (e.g. $C0A0)
addresses: a number or a label from game.sym next to the ROM (e.g. break Main.loop)";

pub struct Console {
    lines: Receiver<String>,
    cheat_file: PathBuf,
    search: Option<MemorySearch>,
    /// Set by `pause` / `resume`; applied by the app.
    pause_request: Option<bool>,
}

impl Console {
    /// Starts reading commands from stdin.
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self::with_receiver(rx, PathBuf::new())
    }

    /// A console that only runs commands passed to [`Console::execute`] (no stdin).
    pub fn detached(cheat_file: PathBuf) -> Self {
        Self::with_receiver(mpsc::channel().1, cheat_file)
    }

    /// A pending `pause` (true) or `resume` (false) command.
    pub fn take_pause_request(&mut self) -> Option<bool> {
        self.pause_request.take()
    }

    /// Sets the `.cht` file used by `cheat save` and `cheat reload` (the loaded ROM's).
    pub fn set_cheat_file(&mut self, path: PathBuf) {
        self.cheat_file = path;
    }

    fn with_receiver(lines: Receiver<String>, cheat_file: PathBuf) -> Self {
        Self {
            lines,
            cheat_file,
            search: None,
            pause_request: None,
        }
    }

    /// Executes all commands typed since the last call, printing their output.
    /// Without a loaded game only `help` works.
    pub fn poll(&mut self, mut gb: Option<&mut GameBoy>) {
        while let Ok(line) = self.lines.try_recv() {
            let output = match gb.as_deref_mut() {
                Some(gb) => self.execute(&line, gb),
                None if line.trim() == "help" => HELP.to_string(),
                None if line.trim().is_empty() => String::new(),
                None => "error: no game loaded (File → Open ROM…)".to_string(),
            };
            if !output.is_empty() {
                println!("{output}");
            }
        }
    }

    /// Runs one command and returns its output (errors as `error: …`).
    pub fn execute(&mut self, line: &str, gb: &mut GameBoy) -> String {
        self.run(line, gb).unwrap_or_else(|e| format!("error: {e}"))
    }

    fn run(&mut self, line: &str, gb: &mut GameBoy) -> Result<String, String> {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            [] => Ok(String::new()),
            ["help" | "?"] => Ok(HELP.to_string()),
            ["pause"] => {
                self.pause_request = Some(true);
                Ok("paused".into())
            }
            ["resume"] => {
                self.pause_request = Some(false);
                Ok("resumed".into())
            }
            ["regs"] => Ok(registers(gb)),
            ["dis"] => Ok(disassembly(gb, gb.pc(), 10)),
            ["dis", addr] => Ok(disassembly(gb, address(gb, addr)?, 10)),
            ["dis", addr, n] => Ok(disassembly(gb, address(gb, addr)?, index(n)?.min(100))),
            ["step"] => step(gb, 1),
            ["step", n] => step(gb, index(n)?.min(100_000)),
            ["break", addr] => {
                let addr = address(gb, addr)?;
                gb.breakpoints_mut().pc.insert(addr);
                Ok(format!("breakpoint at {}", show_address(gb, addr)))
            }
            ["unbreak", addr] => {
                let addr = address(gb, addr)?;
                if gb.breakpoints_mut().pc.remove(&addr) {
                    Ok(format!("removed breakpoint at {}", show_address(gb, addr)))
                } else {
                    Err(format!("no breakpoint at {}", show_address(gb, addr)))
                }
            }
            ["labels", filter @ ..] => Ok(list_labels(gb, filter.first().copied())),
            ["watch", rest @ ..] => watch(gb, rest),
            ["unwatch", n] => {
                let i = index(n)?;
                let watches = &mut gb.breakpoints_mut().watches;
                if i >= watches.len() {
                    return Err(format!("no watchpoint {i}"));
                }
                watches.remove(i);
                Ok(format!("removed watchpoint {i}"))
            }
            ["breaks"] => Ok(list_breaks(gb)),
            ["cheats"] | ["cheat", "list"] => Ok(list_cheats(gb)),
            ["cheat", rest @ ..] => self.cheat(rest, gb),
            ["search", rest @ ..] => self.search(rest, gb),
            ["peek", addr] => peek(gb, address(gb, addr)?, 1),
            ["peek", addr, count] => peek(gb, address(gb, addr)?, parse(count)?),
            ["poke", addr, value] => {
                let (addr, value) = (address(gb, addr)?, byte(value)?);
                gb.poke(addr, value);
                Ok(format!("${addr:04X} = ${value:02X}"))
            }
            _ => Err(format!("unknown command {line:?} (type help)")),
        }
    }

    fn cheat(&mut self, args: &[&str], gb: &mut GameBoy) -> Result<String, String> {
        match args {
            ["add", code, name @ ..] => {
                match gb
                    .cheats_mut()
                    .add_or_template(code, &name.join(" "))
                    .map_err(|e| e.to_string())?
                {
                    freki_core::Added::Cheat(i) => {
                        Ok(format!("added cheat {i}: {}", describe(gb, i)))
                    }
                    freki_core::Added::Template => {
                        let t = gb.cheats().templates().len() - 1;
                        Ok(format!("added template T{t} (cheat fill {t} VALUE)"))
                    }
                }
            }
            ["edit", n, code, name @ ..] => {
                let i = index(n)?;
                gb.cheats_mut()
                    .edit(i, code, &name.join(" "))
                    .map_err(|e| e.to_string())?;
                Ok(format!("cheat {i}: {}", describe(gb, i)))
            }
            [action @ ("on" | "off"), n] => {
                let i = index(n)?;
                if !gb.cheats_mut().set_enabled(i, *action == "on") {
                    return Err(format!("no cheat {i}"));
                }
                Ok(format!("cheat {i} {action}"))
            }
            ["rm", n] => {
                let i = index(n)?;
                let c = gb.cheats_mut().remove(i).ok_or(format!("no cheat {i}"))?;
                Ok(format!("removed {} {}", c.text, c.name))
            }
            ["all", state @ ("on" | "off")] => {
                gb.cheats_mut().enabled = *state == "on";
                Ok(format!("all cheats {state}"))
            }
            ["fill", t, typed] => {
                let t = t.trim_start_matches(['T', 't']);
                let (t, value, raw) = (index(t)?, byte(typed)?, *typed);
                let i = gb
                    .cheats_mut()
                    .fill_template_as(t, value, raw)
                    .map_err(|e| e.to_string())?;
                Ok(format!("added cheat {i}: {}", describe(gb, i)))
            }
            ["save"] => save_cheat_file(&self.cheat_file, gb),
            ["reload"] => Ok(load_cheat_file(&self.cheat_file, gb)
                .unwrap_or_else(|| format!("{} not found", self.cheat_file.display()))),
            _ => Err("usage: cheat list|add|on|off|rm|fill|all|save|reload (type help)".into()),
        }
    }

    fn search(&mut self, args: &[&str], gb: &mut GameBoy) -> Result<String, String> {
        if let ["start"] = args {
            let s = gb.start_memory_search();
            let n = s.len();
            self.search = Some(s);
            return Ok(format!(
                "{n} addresses; change the value in game, then filter"
            ));
        }
        let search = self
            .search
            .as_mut()
            .ok_or("no search running (search start)")?;
        let filter = match args {
            ["eq", n] => Some(Filter::Equal(byte(n)?)),
            ["ne", n] => Some(Filter::NotEqual(byte(n)?)),
            ["changed"] => Some(Filter::Changed),
            ["same"] => Some(Filter::Unchanged),
            ["up"] => Some(Filter::Increased),
            ["down"] => Some(Filter::Decreased),
            _ => None,
        };
        if let Some(filter) = filter {
            let n = search.filter(filter, |l| gb.peek_location(l));
            let mut out = format!("{n} candidate(s)");
            if (1..=10).contains(&n) {
                out.push('\n');
                out.push_str(&list_candidates(search, gb, 10));
            }
            return Ok(out);
        }
        match args {
            ["list"] => Ok(list_candidates(search, gb, 20)),
            ["list", n] => Ok(list_candidates(search, gb, index(n)?)),
            ["add", i, rest @ ..] => {
                let i = index(i)?;
                let &(location, _) = search
                    .candidates()
                    .get(i)
                    .ok_or(format!("no candidate {i}"))?;
                let (value, name) = match rest {
                    [v, name @ ..] if byte(v).is_ok() => (byte(v)?, name.join(" ")),
                    name => (gb.peek_location(location), name.join(" ")),
                };
                let code = Code::gameshark(location, value);
                let name = if name.is_empty() {
                    format!("{location} = {value}")
                } else {
                    name
                };
                let n = gb
                    .cheats_mut()
                    .add(&code, &name)
                    .map_err(|e| e.to_string())?;
                Ok(format!("added cheat {n}: {}", describe(gb, n)))
            }
            _ => Err("usage: search start|eq|ne|changed|same|up|down|list|add (type help)".into()),
        }
    }
}

/// CPU registers and flags on one line.
pub fn registers(gb: &GameBoy) -> String {
    let r = gb.registers();
    let flag = |bit: u8, c: char| if r.f & bit != 0 { c } else { '-' };
    format!(
        "AF=${:04X} BC=${:04X} DE=${:04X} HL=${:04X} SP=${:04X} PC=${:04X} [{}{}{}{}] IME={} {}",
        r.af(),
        r.bc(),
        r.de(),
        r.hl(),
        r.sp,
        r.pc,
        flag(0x80, 'Z'),
        flag(0x40, 'N'),
        flag(0x20, 'H'),
        flag(0x10, 'C'),
        u8::from(gb.ime()),
        if gb.halted() { "HALT" } else { "" }
    )
    .trim_end()
    .to_string()
        + &gb
            .describe_address(r.pc)
            .map(|label| format!(" ({label})"))
            .unwrap_or_default()
}

/// `$4003 (Main.loop)`, or just `$4003` without a label nearby.
pub fn show_address(gb: &GameBoy, addr: u16) -> String {
    match gb.describe_address(addr) {
        Some(label) => format!("${addr:04X} ({label})"),
        None => format!("${addr:04X}"),
    }
}

fn disassembly(gb: &GameBoy, mut addr: u16, count: usize) -> String {
    let mut out = Vec::new();
    for _ in 0..count {
        if let Some(label) = gb.label_at(addr) {
            out.push(format!("{label}:"));
        }
        let ins = gb.disassemble_labeled(addr);
        let marker = if addr == gb.pc() { '>' } else { ' ' };
        let bp = if gb.breakpoints().pc.contains(&addr) {
            '*'
        } else {
            ' '
        };
        let bytes: Vec<String> = ins.bytes.iter().map(|b| format!("{b:02X}")).collect();
        out.push(format!(
            "{marker}{bp}${addr:04X}  {:<9} {}",
            bytes.join(" "),
            ins.text
        ));
        addr = addr.wrapping_add(ins.len as u16);
    }
    out.join("\n")
}

fn step(gb: &mut GameBoy, count: usize) -> Result<String, String> {
    for i in 0..count {
        let mut stop = gb.step_checked().map_err(|e| e.to_string())?;
        // The first step always executes, passing over a breakpoint at the current PC;
        // later steps stop at breakpoints.
        if i == 0 && matches!(stop, Some(freki_core::Stop::Breakpoint { .. })) {
            stop = gb.step_checked().map_err(|e| e.to_string())?;
        }
        if let Some(stop) = stop {
            return Ok(format!(
                "{}\n{}",
                describe_stop(&stop, gb),
                disassembly(gb, gb.pc(), 1)
            ));
        }
    }
    Ok(format!(
        "{}\n{}",
        registers(gb),
        disassembly(gb, gb.pc(), 1)
    ))
}

/// Human-readable reason for a debugger stop, with labels from the symbol file.
pub fn describe_stop(stop: &freki_core::Stop, gb: &GameBoy) -> String {
    match stop {
        freki_core::Stop::Breakpoint { pc } => format!("breakpoint at {}", show_address(gb, *pc)),
        freki_core::Stop::Watch { pc, hit } => {
            let kind = match hit.access {
                freki_core::Access::Read => "read",
                freki_core::Access::Write => "write",
            };
            format!(
                "{kind} {} = ${:02X} by instruction at {}",
                show_address(gb, hit.addr),
                hit.value,
                show_address(gb, *pc)
            )
        }
    }
}

fn watch(gb: &mut GameBoy, args: &[&str]) -> Result<String, String> {
    let (mode, addrs): (&str, Vec<&str>) = match args.last() {
        Some(&m @ ("r" | "w" | "rw")) => (m, args[..args.len() - 1].to_vec()),
        _ => ("w", args.to_vec()),
    };
    let (start, end) = match addrs.as_slice() {
        [a] => (address(gb, a)?, address(gb, a)?),
        [a, b] => (address(gb, a)?, address(gb, b)?),
        _ => return Err("usage: watch ADDR [END] [r|w|rw]".into()),
    };
    let w = freki_core::Watchpoint::new(start, end, mode.contains('r'), mode.contains('w'));
    let text = describe_watch(&w);
    gb.breakpoints_mut().watches.push(w);
    Ok(format!("watching {text}"))
}

/// e.g. `$C000-$C00F rw`.
pub fn describe_watch(w: &freki_core::Watchpoint) -> String {
    let range = if w.start == w.end {
        format!("${:04X}", w.start)
    } else {
        format!("${:04X}-${:04X}", w.start, w.end)
    };
    let mode = match (w.read, w.write) {
        (true, true) => "rw",
        (true, false) => "r",
        _ => "w",
    };
    format!("{range} {mode}")
}

fn list_breaks(gb: &GameBoy) -> String {
    let b = gb.breakpoints();
    if b.is_empty() {
        return "no breakpoints or watchpoints".into();
    }
    let mut out = Vec::new();
    for pc in &b.pc {
        out.push(format!("  break {}", show_address(gb, *pc)));
    }
    for (i, w) in b.watches.iter().enumerate() {
        out.push(format!("  watch {i}: {}", describe_watch(w)));
    }
    out.join("\n")
}

/// Loads the cheats for `path` (`game.cht`). If that file does not exist, a cheat file
/// named the way cheat databases name it (`game-title-usa.cht` for
/// `Game Title (USA) (Rev 1).gbc`) is imported instead; changes are still saved to
/// `path`. Returns a message to show, or `None` if there is no cheat file.
pub fn load_cheat_file(path: &Path, gb: &mut GameBoy) -> Option<String> {
    let source = find_cheat_file(path)?;
    let text = fs::read_to_string(&source).ok()?;
    gb.cheats_mut().clear();
    let errors = gb.cheats_mut().load_text(&text);
    let verb = if source == path { "loaded" } else { "imported" };
    let mut msg = format!(
        "{verb} {} cheat(s) from {}",
        gb.cheats().list().len(),
        source.display()
    );
    let other = gb
        .cheats()
        .list()
        .iter()
        .filter(|c| !gb.cheat_fits(c))
        .count();
    if other > 0 {
        msg.push_str(&format!(
            "; {other} made for another version of the game (they will do nothing)"
        ));
    }
    for (line, e) in errors {
        msg.push_str(&format!("\n  line {line}: {e}"));
    }
    Some(msg)
}

/// `path` if it exists, otherwise a `.cht` file in the same directory whose name matches
/// the ROM's as a lowercase, dashed slug: of the whole name, of the title and first
/// bracket (`metal-gear-solid-usa`), or of the title alone.
pub fn find_cheat_file(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let stem = path.file_stem()?.to_str()?;
    let first_bracket = stem.find(')').map_or(stem, |i| &stem[..=i]);
    let title = stem.split('(').next().unwrap_or(stem);
    let wanted = [slug(stem), slug(first_bracket), slug(title)];
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let candidates: Vec<(String, PathBuf)> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("cht")))
        .filter_map(|p| Some((slug(p.file_stem()?.to_str()?), p)))
        .collect();
    wanted
        .iter()
        .filter(|w| !w.is_empty())
        .find_map(|w| candidates.iter().find(|(s, _)| s == w))
        .map(|(_, p)| p.clone())
}

/// Lowercase letters and digits, every other run of characters becomes one dash.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Writes the cheat list to a `.cht` file.
pub fn save_cheat_file(path: &Path, gb: &GameBoy) -> Result<String, String> {
    fs::write(path, gb.cheats().to_text())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(format!("saved cheats to {}", path.display()))
}

fn describe(gb: &GameBoy, i: usize) -> String {
    let c = &gb.cheats().list()[i];
    let state = if c.enabled { "on " } else { "off" };
    let other = if gb.cheat_fits(c) {
        ""
    } else {
        "  [made for another version of the game]"
    };
    format!("{state} {:<22} {} {}", c.kind(), c.text, c.name)
        .trim_end()
        .to_string()
        + other
}

fn list_cheats(gb: &GameBoy) -> String {
    let cheats = gb.cheats();
    let unsupported = cheats.unsupported();
    if cheats.list().is_empty() && cheats.templates().is_empty() && unsupported.is_empty() {
        return "no cheats (cheat add CODE [name])".into();
    }
    let mut out = format!(
        "cheats are {}",
        if cheats.enabled {
            "ON"
        } else {
            "OFF (cheat all on / F9)"
        }
    );
    for i in 0..cheats.list().len() {
        out.push_str(&format!("\n  {i:>2}  {}", describe(gb, i)));
    }
    if !unsupported.is_empty() {
        out.push_str("\nnot supported (kept in the file, they do nothing):");
        for entry in &unsupported {
            out.push_str(&format!(
                "\n   –  --- {:<22} {} {}",
                entry.kind, entry.code, entry.name
            ));
        }
    }
    let templates = cheats.templates();
    if !templates.is_empty() {
        out.push_str("\ntemplates (cheat fill T VALUE):");
        for (t, template) in templates.iter().enumerate() {
            out.push_str(&format!(
                "\n  T{t:<2} {} {}  (0–{})",
                template.code,
                template.name,
                template.max()
            ));
        }
    }
    out
}

fn list_candidates(search: &MemorySearch, gb: &GameBoy, max: usize) -> String {
    let mut out = String::new();
    for (i, &(location, _)) in search.candidates().iter().take(max).enumerate() {
        let v = gb.peek_location(location);
        out.push_str(&format!("  {i:>2}  {location:>7} = {v:>3} (${v:02X})\n"));
    }
    if search.len() > max {
        out.push_str(&format!("  … {} more\n", search.len() - max));
    }
    out.trim_end().to_string()
}

fn peek(gb: &GameBoy, addr: u16, count: u16) -> Result<String, String> {
    let mut out = String::new();
    for row in (0..count.min(256)).step_by(16) {
        let start = addr.wrapping_add(row);
        out.push_str(&format!("${start:04X}:"));
        for i in 0..16.min(count - row) {
            out.push_str(&format!(" {:02X}", gb.peek(start.wrapping_add(i))));
        }
        out.push('\n');
    }
    Ok(out.trim_end().to_string())
}

/// Parses an address: a number like [`parse`] takes, or a label from the symbol file.
fn address(gb: &GameBoy, s: &str) -> Result<u16, String> {
    parse(s).or_else(|_| {
        gb.symbols()
            .find(s)
            .map(|at| at.addr)
            .ok_or_else(|| format!("{s:?} is neither a number nor a label"))
    })
}

/// The labels from the symbol file whose names contain `filter` (any case), at most 50.
fn list_labels(gb: &GameBoy, filter: Option<&str>) -> String {
    let symbols = gb.symbols();
    if symbols.is_empty() {
        return "no labels (put the assembler's game.sym next to the ROM)".into();
    }
    let filter = filter.map(str::to_lowercase);
    let matches: Vec<String> = symbols
        .iter()
        .filter(|(_, name)| {
            filter
                .as_ref()
                .is_none_or(|f| name.to_lowercase().contains(f.as_str()))
        })
        .map(|(at, name)| format!("  {:02X}:{:04X} {name}", at.bank, at.addr))
        .collect();
    let shown = matches.len().min(50);
    let mut out = matches[..shown].join("\n");
    if matches.len() > shown {
        out.push_str(&format!("\n  … and {} more", matches.len() - shown));
    }
    if out.is_empty() {
        out = "no matching labels".into();
    }
    out
}

/// Parses a 16-bit number: decimal, or hex with `$` or `0x`.
fn parse(s: &str) -> Result<u16, String> {
    let parsed = if let Some(hex) = s.strip_prefix('$').or_else(|| s.strip_prefix("0x")) {
        u16::from_str_radix(hex, 16)
    } else {
        s.parse()
    };
    parsed.map_err(|_| format!("{s:?} is not a number"))
}

fn byte(s: &str) -> Result<u8, String> {
    u8::try_from(parse(s)?).map_err(|_| format!("{s} does not fit in a byte"))
}

fn index(s: &str) -> Result<usize, String> {
    s.parse().map_err(|_| format!("{s:?} is not an index"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// $0150: INC A; LD ($C000),A; JR -6 (counts in $C000).
    fn counting_gb() -> GameBoy {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
        rom[0x150..0x156].copy_from_slice(&[0x3C, 0xEA, 0x00, 0xC0, 0x18, 0xFA]);
        GameBoy::new(rom).unwrap()
    }

    fn console(dir: &str) -> Console {
        let (_tx, rx) = mpsc::channel();
        let path =
            std::env::temp_dir().join(format!("freki-console-{dir}-{}.cht", std::process::id()));
        Console::with_receiver(rx, path)
    }

    #[test]
    fn search_finds_counter_and_makes_a_cheat() {
        let mut gb = counting_gb();
        let mut c = console("search");
        gb.run_frame().unwrap();
        assert!(c.execute("search start", &mut gb).contains("addresses"));
        for _ in 0..4 {
            gb.run_frame().unwrap();
            c.execute("search changed", &mut gb);
        }
        let list = c.execute("search list", &mut gb);
        let index = list
            .lines()
            .find(|l| l.contains("$C000"))
            .and_then(|l| l.split_whitespace().next())
            .expect("counter found");
        let out = c.execute(&format!("search add {index} $42 frozen"), &mut gb);
        assert!(out.contains("014200C0"), "{out}");
        gb.run_frame().unwrap();
        assert_eq!(gb.peek(0xC000), 0x42);
    }

    #[test]
    fn cheat_commands_and_file_round_trip() {
        let mut gb = counting_gb();
        let mut c = console("file");
        assert!(
            c.execute("cheat add 010A10C1 Lives", &mut gb)
                .contains("added cheat 0")
        );
        assert!(
            c.execute("cheat add nonsense", &mut gb)
                .starts_with("error")
        );
        assert!(c.execute("cheat off 0", &mut gb).contains("off"));
        assert!(c.execute("cheat save", &mut gb).contains("saved"));
        c.execute("cheat rm 0", &mut gb);
        assert!(gb.cheats().list().is_empty());
        assert!(c.execute("cheat reload", &mut gb).contains("loaded 1"));
        assert!(!gb.cheats().list()[0].enabled);
        fs::remove_file(&c.cheat_file).unwrap();
    }

    #[test]
    fn imports_cheat_file_named_like_a_cheat_database() {
        assert_eq!(
            slug("Super Mario Bros. Deluxe (USA, Europe)"),
            "super-mario-bros-deluxe-usa-europe"
        );
        let dir = std::env::temp_dir().join(format!("freki-cht-import-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let own = dir.join("Super Mario Bros. Deluxe (USA, Europe) (Rev 2).cht");
        assert_eq!(find_cheat_file(&own), None);
        let db = dir.join("super-mario-bros-deluxe-usa-europe.cht");
        fs::write(&db, "cheats = 1\ncheat0_desc = \"Lives\"\ncheat0_code = \"010A10C1\"\ncheat0_enable = true\n").unwrap();
        fs::write(dir.join("super-mario-bros.cht"), "").unwrap();
        assert_eq!(find_cheat_file(&own), Some(db.clone()));
        let mut gb = counting_gb();
        let msg = load_cheat_file(&own, &mut gb).unwrap();
        assert!(msg.starts_with("imported 1"), "{msg}");
        // Saving goes to the ROM's own file, which is preferred from then on.
        save_cheat_file(&own, &gb).unwrap();
        assert_eq!(find_cheat_file(&own), Some(own.clone()));
        assert!(fs::read_to_string(&own).unwrap().starts_with("cheats = 1"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn debugger_commands() {
        let mut gb = counting_gb();
        let mut c = console("debug");
        assert!(c.execute("regs", &mut gb).contains("PC=$0100"));
        assert!(c.execute("dis $0150 2", &mut gb).contains("INC A"));
        assert!(c.execute("break $0151", &mut gb).contains("$0151"));
        assert!(c.execute("watch $C000 w", &mut gb).contains("$C000 w"));
        assert!(c.execute("breaks", &mut gb).contains("break $0151"));
        // Stepping from $0100 reaches the loop and stops at the breakpoint.
        let out = c.execute("step 20", &mut gb);
        assert!(out.contains("breakpoint at $0151"), "{out}");
        let out = c.execute("step 5", &mut gb);
        assert!(out.contains("write $C000"), "{out}");
        assert!(c.execute("unwatch 0", &mut gb).contains("removed"));
        assert!(c.execute("unbreak $0151", &mut gb).contains("removed"));
        assert!(c.execute("pause", &mut gb).contains("paused"));
        assert_eq!(c.take_pause_request(), Some(true));
    }

    #[test]
    fn peek_poke_and_numbers() {
        let mut gb = counting_gb();
        let mut c = console("peek");
        c.execute("poke $D000 0x7F", &mut gb);
        assert_eq!(c.execute("peek $D000", &mut gb), "$D000: 7F");
        assert!(
            c.execute("poke $D000 300", &mut gb)
                .contains("does not fit")
        );
        assert!(c.execute("frobnicate", &mut gb).contains("unknown command"));
    }
}
