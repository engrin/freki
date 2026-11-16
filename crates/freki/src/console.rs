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
  cheat add CODE [name]        add a GameShark (01VVLLHH) or Game Genie (ABC-DEF[-GHI]) code
  cheat on|off|rm N            enable, disable or remove cheat N
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
other:
  pause | resume               pause emulation (also P in the window)
  help
numbers: decimal, or hex with $ or 0x (e.g. $C0A0)";

pub struct Console {
    lines: Receiver<String>,
    cheat_file: PathBuf,
    search: Option<MemorySearch>,
    pub paused: bool,
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

    /// Sets the `.cht` file used by `cheat save` and `cheat reload` (the loaded ROM's).
    pub fn set_cheat_file(&mut self, path: PathBuf) {
        self.cheat_file = path;
    }

    fn with_receiver(lines: Receiver<String>, cheat_file: PathBuf) -> Self {
        Self {
            lines,
            cheat_file,
            search: None,
            paused: false,
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
                self.paused = true;
                Ok("paused".into())
            }
            ["resume"] => {
                self.paused = false;
                Ok("resumed".into())
            }
            ["cheats"] | ["cheat", "list"] => Ok(list_cheats(gb)),
            ["cheat", rest @ ..] => self.cheat(rest, gb),
            ["search", rest @ ..] => self.search(rest, gb),
            ["peek", addr] => peek(gb, parse(addr)?, 1),
            ["peek", addr, count] => peek(gb, parse(addr)?, parse(count)?),
            ["poke", addr, value] => {
                let (addr, value) = (parse(addr)?, byte(value)?);
                gb.poke(addr, value);
                Ok(format!("${addr:04X} = ${value:02X}"))
            }
            _ => Err(format!("unknown command {line:?} (type help)")),
        }
    }

    fn cheat(&mut self, args: &[&str], gb: &mut GameBoy) -> Result<String, String> {
        match args {
            ["add", code, name @ ..] => {
                let i = gb
                    .cheats_mut()
                    .add(code, &name.join(" "))
                    .map_err(|e| e.to_string())?;
                Ok(format!("added cheat {i}: {}", describe(gb, i)))
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
            ["save"] => save_cheat_file(&self.cheat_file, gb),
            ["reload"] => Ok(load_cheat_file(&self.cheat_file, gb)
                .unwrap_or_else(|| format!("{} not found", self.cheat_file.display()))),
            _ => Err("usage: cheat list|add|on|off|rm|all|save|reload (type help)".into()),
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
            let n = search.filter(filter, |a| gb.peek(a));
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
                let &(addr, _) = search
                    .candidates()
                    .get(i)
                    .ok_or(format!("no candidate {i}"))?;
                let (value, name) = match rest {
                    [v, name @ ..] if byte(v).is_ok() => (byte(v)?, name.join(" ")),
                    name => (gb.peek(addr), name.join(" ")),
                };
                let code = Code::gameshark(addr, value);
                let name = if name.is_empty() {
                    format!("${addr:04X} = {value}")
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

/// Loads a `.cht` file into the cheat list, if it exists. Returns a message to show.
pub fn load_cheat_file(path: &Path, gb: &mut GameBoy) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    gb.cheats_mut().clear();
    let errors = gb.cheats_mut().load_text(&text);
    let mut msg = format!(
        "loaded {} cheat(s) from {}",
        gb.cheats().list().len(),
        path.display()
    );
    for (line, e) in errors {
        msg.push_str(&format!("\n  line {line}: {e}"));
    }
    Some(msg)
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
    format!("{state} {} {}", c.text, c.name)
        .trim_end()
        .to_string()
}

fn list_cheats(gb: &GameBoy) -> String {
    let cheats = gb.cheats();
    if cheats.list().is_empty() {
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
    out
}

fn list_candidates(search: &MemorySearch, gb: &GameBoy, max: usize) -> String {
    let mut out = String::new();
    for (i, &(addr, _)) in search.candidates().iter().take(max).enumerate() {
        let v = gb.peek(addr);
        out.push_str(&format!("  {i:>2}  ${addr:04X} = {v:>3} (${v:02X})\n"));
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
