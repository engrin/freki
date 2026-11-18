//! `freki --headless`: runs a game without a window or sound, as fast as possible, and
//! reports what happened. For debugging games and for scripted checks.
//!
//! Example: run 20 seconds of Tetris, pressing Start at frame 200, then save a
//! screenshot and print the registers:
//!
//! ```text
//! freki --headless tetris.gb --frames 1200 --press 200:start --screenshot out.png --exec regs
//! ```
//!
//! Exit status: 0 when the run finished (all frames run, or the `--until-serial` text
//! seen), 2 when it stopped at a breakpoint or watchpoint, 1 on errors (including a CPU
//! locked up by an illegal opcode, or `--until-serial` text that never came).

use std::error::Error;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use freki_core::{BootRom, Button, CLOCK_HZ, CYCLES_PER_FRAME, GameBoy, Link, Model};

use crate::console::{self, Console};
use crate::rom_file;
use crate::session::{self, Start};
use crate::settings::{self, Palette};

/// Prints a line to stdout. When stdout is a closed pipe (`freki --headless … | head`),
/// exits quietly with status 141, as a program killed by SIGPIPE would, instead of
/// panicking like `println!`.
macro_rules! out {
    ($($arg:tt)*) => {
        print_line(format_args!($($arg)*))
    };
}

fn print_line(args: std::fmt::Arguments) {
    let result = writeln!(std::io::stdout().lock(), "{args}");
    if let Err(e) = result {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(141);
        }
        panic!("failed printing to stdout: {e}");
    }
}

pub const USAGE: &str = "\
usage: freki --headless ROM [options]

Runs ROM without a window or sound, as fast as possible.

run:
  --frames N              frames to run (default 600, about 10 seconds)
  --until-serial TEXT     stop once the serial output contains TEXT (exit 1 if it never does)
  --model dmg|cgb|sgb     console (default: GBC for colour games, SGB for SGB games,
                          else Game Boy)
  --boot FILE             start with this boot ROM (DMG or GBC, by size)
  --patch FILE            apply this IPS or BPS patch (default: ROM.bps or ROM.ips if present)
  --sav FILE              load this battery save (never written back; see --write-sav)
  --off DURATION          with --sav: the Game Boy was off this long, so a cartridge
                          clock catches up (90, 90s, 30m, 2h, 3d)
  --load-state FILE       start from a save state (.ss0 … .ss9)
  --press FRAME:KEYS[:N]  hold KEYS (a+b+start+select+up+down+left+right) from FRAME
                          for N frames (default 6); repeatable
  --tilt FRAME:X,Y[:N]    tilt sensor games (MBC7): tilt so things roll right (X) and
                          down the screen (Y), in g (about -1 to 1), from FRAME for N
                          frames (default 60); repeatable
  --link ROM2             connect ROM2 by link cable, on a second Game Boy (its console
                          chosen as by default); it runs in step with ROM
  --link-sav FILE         load this battery save into ROM2
  --press2 FRAME:KEYS[:N] like --press, for ROM2
  --cheats                load the game's cheat file (ROM.cht, or one named as in cheat
                          databases) with the cheats switched on as saved there
  --cheat N|CODE          switch on cheat N of that file, or add a GameShark or Game
                          Genie code (A+B for several); repeatable; active from frame 0
debugging:
  --break ADDR            stop when PC reaches ADDR; repeatable
  --watch 'ADDR [END] [r|w|rw]'
                          stop when the CPU accesses ADDR..END (default w); repeatable
  --trace FILE            log every instruction in Gameboy Doctor's format
                          (A:.. F:.. … PC:00:0100 PCMEM:..)
at the end (in this order):
  --exec 'COMMAND'        run a console command (regs, dis, peek $C000 16, …); repeatable
  --serial                print what the game sent over the serial port
  --screenshot FILE.png   save the screen (with --link: both screens side by side)
  --palette NAME          DMG shades for the screenshot: green, grey (default), pocket, light
  --color-correction      GBC colours as the GBC screen shows them (default: raw)
  --frame-blend           mix the screenshot with the frame before, like the real LCD
  --save-state FILE       save the machine state
  --write-sav FILE        write the battery save (RAM, then the clock, if any)

clock time: a run starts at the time the --sav file's clock was written (plus --off),
or at 0, and moves on with the emulated time; --write-sav stamps the clock with it.
So a save written by one run and loaded with --off 1h in the next is one hour behind.

numbers: decimal, or hex with $ or 0x (e.g. $C0A0)
exit status: 0 finished, 2 stopped at a breakpoint or watchpoint, 1 error";

#[derive(Default)]
struct Options {
    rom: Option<PathBuf>,
    frames: u64,
    until_serial: Option<String>,
    model: Option<Model>,
    boot: Option<PathBuf>,
    patch: Option<PathBuf>,
    sav: Option<PathBuf>,
    off: Option<u64>,
    load_state: Option<PathBuf>,
    presses: Vec<Press>,
    link: Option<PathBuf>,
    link_sav: Option<PathBuf>,
    presses2: Vec<Press>,
    tilts: Vec<Tilt>,
    load_cheats: bool,
    cheats: Vec<String>,
    breaks: Vec<String>,
    watches: Vec<String>,
    trace: Option<PathBuf>,
    exec: Vec<String>,
    serial: bool,
    screenshot: Option<PathBuf>,
    palette: Option<Palette>,
    color_correction: bool,
    frame_blend: bool,
    save_state: Option<PathBuf>,
    write_sav: Option<PathBuf>,
}

/// Buttons held from frame `from` for `frames` frames.
struct Press {
    from: u64,
    frames: u64,
    buttons: Vec<Button>,
}

/// The Game Boy tilted by `x`, `y` (in g) from frame `from` for `frames` frames.
struct Tilt {
    from: u64,
    frames: u64,
    x: f32,
    y: f32,
}

impl Press {
    fn held_at(&self, frame: u64) -> bool {
        (self.from..self.from + self.frames).contains(&frame)
    }
}

/// Holds the buttons `presses` call for at `frame`, releasing those of `pressed` that no
/// longer are; returns the held buttons.
fn hold(gb: &mut GameBoy, presses: &[Press], frame: u64, pressed: &[Button]) -> Vec<Button> {
    let held: Vec<Button> = presses
        .iter()
        .filter(|p| p.held_at(frame))
        .flat_map(|p| p.buttons.iter().copied())
        .collect();
    for &b in pressed {
        if !held.contains(&b) {
            gb.set_button(b, false);
        }
    }
    for &b in &held {
        gb.set_button(b, true);
    }
    held
}

/// Runs headless with `args` (everything after the program name, `--headless` removed).
/// Returns the process exit status.
pub fn main(args: &[String]) -> i32 {
    let options = match parse_args(args) {
        Ok(Some(options)) => options,
        Ok(None) => {
            out!("{USAGE}");
            return 0;
        }
        Err(e) => {
            eprintln!("freki: {e}\n\n{USAGE}");
            return 1;
        }
    };
    match run(&options) {
        Ok(status) => status,
        Err(e) => {
            eprintln!("freki: {e}");
            1
        }
    }
}

fn parse_args(args: &[String]) -> Result<Option<Options>, String> {
    let mut o = Options {
        frames: 600,
        ..Default::default()
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--frames" => {
                o.frames = value(arg)?
                    .parse()
                    .map_err(|_| "--frames needs a number".to_string())?;
            }
            "--until-serial" => o.until_serial = Some(value(arg)?),
            "--model" => {
                o.model = Some(match value(arg)?.to_ascii_lowercase().as_str() {
                    "dmg" | "gb" => Model::Dmg,
                    "cgb" | "gbc" => Model::Cgb,
                    "sgb" => Model::Sgb,
                    m => return Err(format!("unknown model {m:?} (dmg, cgb or sgb)")),
                });
            }
            "--boot" => o.boot = Some(value(arg)?.into()),
            "--patch" => o.patch = Some(value(arg)?.into()),
            "--sav" => o.sav = Some(value(arg)?.into()),
            "--off" => o.off = Some(parse_duration(&value(arg)?)?),
            "--load-state" => o.load_state = Some(value(arg)?.into()),
            "--press" => o.presses.push(parse_press(&value(arg)?)?),
            "--link" => o.link = Some(value(arg)?.into()),
            "--link-sav" => o.link_sav = Some(value(arg)?.into()),
            "--press2" => o.presses2.push(parse_press(&value(arg)?)?),
            "--tilt" => o.tilts.push(parse_tilt(&value(arg)?)?),
            "--cheats" => o.load_cheats = true,
            "--cheat" => o.cheats.push(value(arg)?),
            "--break" => o.breaks.push(value(arg)?),
            "--watch" => o.watches.push(value(arg)?),
            "--trace" => o.trace = Some(value(arg)?.into()),
            "--exec" => o.exec.push(value(arg)?),
            "--serial" => o.serial = true,
            "--screenshot" => o.screenshot = Some(value(arg)?.into()),
            "--palette" => {
                o.palette = Some(match value(arg)?.to_ascii_lowercase().as_str() {
                    "green" => Palette::Green,
                    "grey" | "gray" => Palette::Grey,
                    "pocket" => Palette::Pocket,
                    "light" => Palette::Light,
                    p => return Err(format!("unknown palette {p:?}")),
                });
            }
            "--color-correction" | "--colour-correction" => o.color_correction = true,
            "--frame-blend" => o.frame_blend = true,
            "--save-state" => o.save_state = Some(value(arg)?.into()),
            "--write-sav" => o.write_sav = Some(value(arg)?.into()),
            a if a.starts_with('-') => return Err(format!("unknown option {a}")),
            _ if o.rom.is_none() => o.rom = Some(arg.into()),
            a => return Err(format!("unexpected argument {a:?}")),
        }
    }
    if o.rom.is_none() {
        return Err("no ROM given".into());
    }
    if o.off.is_some() && o.sav.is_none() {
        return Err("--off needs --sav".into());
    }
    if o.link.is_none() && (o.link_sav.is_some() || !o.presses2.is_empty()) {
        return Err("--link-sav and --press2 need --link".into());
    }
    if o.link.is_some() && (o.trace.is_some() || !o.breaks.is_empty() || !o.watches.is_empty()) {
        return Err("--trace, --break and --watch do not work with --link".into());
    }
    Ok(Some(o))
}

/// Seconds, with an optional unit: `90`, `90s`, `30m`, `2h`, `3d`.
fn parse_duration(spec: &str) -> Result<u64, String> {
    let bad = || format!("bad duration {spec:?} (e.g. 90, 90s, 30m, 2h, 3d)");
    let (number, unit) = match spec.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => spec.split_at(i),
        None => (spec, "s"),
    };
    let scale = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return Err(bad()),
    };
    let n: u64 = number.parse().map_err(|_| bad())?;
    n.checked_mul(scale).ok_or_else(bad)
}

/// `FRAME:KEYS[:N]`, e.g. `120:start` or `300:a+right:30`.
fn parse_press(spec: &str) -> Result<Press, String> {
    let bad = || format!("bad --press {spec:?} (FRAME:KEYS[:N], e.g. 120:start or 300:a+right:30)");
    let parts: Vec<&str> = spec.split(':').collect();
    let (from, keys, frames) = match parts.as_slice() {
        [f, k] => (f, k, "6"),
        [f, k, n] => (f, k, *n),
        _ => return Err(bad()),
    };
    let buttons = keys
        .split('+')
        .map(|k| match k.trim().to_ascii_lowercase().as_str() {
            "a" => Ok(Button::A),
            "b" => Ok(Button::B),
            "start" => Ok(Button::Start),
            "select" => Ok(Button::Select),
            "up" => Ok(Button::Up),
            "down" => Ok(Button::Down),
            "left" => Ok(Button::Left),
            "right" => Ok(Button::Right),
            _ => Err(bad()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Press {
        from: from.parse().map_err(|_| bad())?,
        frames: frames.parse().map_err(|_| bad())?,
        buttons,
    })
}

/// `FRAME:X,Y[:N]`, e.g. `300:1,0` or `300:-0.5,0.2:120`.
fn parse_tilt(spec: &str) -> Result<Tilt, String> {
    let bad = || format!("bad --tilt {spec:?} (FRAME:X,Y[:N], e.g. 300:1,0 or 300:-0.5,0:120)");
    let parts: Vec<&str> = spec.split(':').collect();
    let (from, xy, frames) = match parts.as_slice() {
        [f, xy] => (f, xy, "60"),
        [f, xy, n] => (f, xy, *n),
        _ => return Err(bad()),
    };
    let (x, y) = xy.split_once(',').ok_or_else(bad)?;
    Ok(Tilt {
        from: from.parse().map_err(|_| bad())?,
        frames: frames.parse().map_err(|_| bad())?,
        x: x.trim().parse().map_err(|_| bad())?,
        y: y.trim().parse().map_err(|_| bad())?,
    })
}

fn run(o: &Options) -> Result<i32, Box<dyn Error>> {
    let rom_path = o.rom.as_deref().expect("checked by parse_args");
    let rom = rom_file::load(rom_path, o.patch.as_deref())?;
    if !rom.describe().is_empty() {
        out!("loaded {}{}", session::file_name(rom_path), rom.describe());
    }
    let mut start = Start {
        model: o.model,
        ..Default::default()
    };
    if let Some(path) = &o.boot {
        let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let boot = BootRom::new(&bytes)?;
        if boot.is_cgb() {
            start.cgb_boot_rom = Some(boot);
        } else {
            start.dmg_boot_rom = Some(boot);
        }
    }
    let mut gb = session::power_on(rom.data, &start)?;
    // Unix time at the start of the run; see USAGE.
    let mut start_time = 0;
    if let Some(path) = &o.sav {
        let data = read(path)?;
        let saved = gb.save_time(&data);
        match (saved, o.off) {
            (Some(saved), Some(off)) => {
                start_time = saved.saturating_add(off);
                gb.load_save_data(&data, Some(start_time));
                out!("clock caught up on {off} s off");
            }
            (saved, off) => {
                if off.is_some() {
                    out!("--off ignored: the save has no clock");
                }
                start_time = saved.unwrap_or(0);
                gb.load_save_data(&data, None);
            }
        }
    }
    if let Some(path) = &o.load_state {
        gb.load_state(&read(path)?)?;
    }

    let mut console = Console::detached(rom_path.with_extension("cht"));
    let mut cheat_lines = Vec::new();
    if o.load_cheats {
        cheat_lines.push("cheat reload".to_string());
    }
    for cheat in &o.cheats {
        cheat_lines.push(if cheat.parse::<usize>().is_ok() {
            format!("cheat on {cheat}")
        } else {
            format!("cheat add {cheat}")
        });
    }
    for line in cheat_lines {
        let out = console.execute(&line, &mut gb);
        if let Some(e) = out.strip_prefix("error: ") {
            return Err(format!("{line}: {e}").into());
        }
        out!("{out}");
    }
    for addr in &o.breaks {
        command(&mut console, &format!("break {addr}"), &mut gb)?;
    }
    for spec in &o.watches {
        command(&mut console, &format!("watch {spec}"), &mut gb)?;
    }
    let mut trace = match &o.trace {
        Some(path) => {
            Some(BufWriter::new(fs::File::create(path).map_err(|e| {
                format!("cannot create {}: {e}", path.display())
            })?))
        }
        None => None,
    };

    // The second Game Boy on the link cable.
    let mut partner = match &o.link {
        Some(path) => {
            let rom = rom_file::load(path, None)?;
            out!("linked {}{}", session::file_name(path), rom.describe());
            let partner_start = Start {
                model: None,
                ..start.clone()
            };
            let mut gb2 = session::power_on(rom.data, &partner_start)?;
            if let Some(sav) = &o.link_sav {
                gb2.load_save_data(&read(sav)?, None);
            }
            Some((gb2, Link::new()))
        }
        None => None,
    };

    let mut status = 0;
    let mut pressed: Vec<Button> = Vec::new();
    let mut pressed2: Vec<Button> = Vec::new();
    let mut frame = 0;
    // With --frame-blend: the picture before the last frame.
    let mut previous = None;
    while frame < o.frames {
        if o.frame_blend && frame + 1 == o.frames {
            previous = Some(picture(o, &gb, partner.as_ref().map(|(gb2, _)| gb2)));
        }
        pressed = hold(&mut gb, &o.presses, frame, &pressed);
        if let Some((gb2, _)) = partner.as_mut() {
            pressed2 = hold(gb2, &o.presses2, frame, &pressed2);
        }
        let (x, y) = o
            .tilts
            .iter()
            .filter(|t| (t.from..t.from + t.frames).contains(&frame))
            .fold((0.0, 0.0), |(x, y), t| (x + t.x, y + t.y));
        gb.set_tilt(x, y);

        if let Some((gb2, link)) = partner.as_mut() {
            if let Err(e) = link.run_frame(&mut gb, gb2) {
                out!("frame {frame}: {e}");
                status = 1;
                break;
            }
            frame += 1;
            continue;
        }
        let result = match trace.as_mut() {
            Some(out) => {
                let mut io = Ok(());
                let stop = gb.run_frame_traced(|gb| {
                    if io.is_ok() {
                        io = writeln!(out, "{}", doctor_line(gb));
                    }
                });
                io?;
                stop
            }
            None => gb.run_frame_checked(),
        };
        match result {
            Ok(Some(stop)) => {
                out!("frame {frame}: {}", console::describe_stop(&stop));
                out!("{}", console::registers(&gb));
                out!("{}", console.execute("dis", &mut gb));
                status = 2;
                break;
            }
            Ok(None) => {}
            Err(e) => {
                out!("frame {frame}: {e}");
                out!("{}", console::registers(&gb));
                status = 1;
                break;
            }
        }
        frame += 1;
        if let Some(text) = &o.until_serial
            && String::from_utf8_lossy(gb.serial_output()).contains(text.as_str())
        {
            out!("frame {frame}: serial output contains {text:?}");
            break;
        }
    }
    if status == 0 && frame == o.frames {
        out!("ran {frame} frame{}", if frame == 1 { "" } else { "s" });
        if let Some(text) = &o.until_serial {
            out!("serial output never contained {text:?}");
            status = 1;
        }
    }
    if let Some(out) = trace.as_mut() {
        out.flush()?;
    }

    for line in &o.exec {
        out!("> {line}");
        out!("{}", console.execute(line, &mut gb));
    }
    if o.serial {
        out!(
            "serial output:\n{}",
            String::from_utf8_lossy(gb.serial_output())
        );
    }
    if let Some(path) = &o.screenshot {
        let (mut rgba, size) = picture(o, &gb, partner.as_ref().map(|(gb2, _)| gb2));
        if let Some((before, before_size)) = &previous
            && *before_size == size
        {
            settings::blend(before, &mut rgba);
        }
        save_png(path, &rgba, size)?;
        out!("screenshot saved to {}", path.display());
    }
    if let Some(path) = &o.save_state {
        fs::write(path, gb.save_state())
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        out!("state saved to {}", path.display());
    }
    if let Some(path) = &o.write_sav {
        let now = start_time + frame * u64::from(CYCLES_PER_FRAME) / u64::from(CLOCK_HZ);
        let data = gb
            .save_data(now)
            .ok_or("--write-sav: the cartridge has no battery")?;
        fs::write(path, &data).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        out!(
            "battery save written to {} ({} bytes)",
            path.display(),
            data.len()
        );
    }
    Ok(status)
}

/// The screen as a screenshot shows it (RGBA, with its size); with a linked game, both.
fn picture(o: &Options, gb: &GameBoy, partner: Option<&GameBoy>) -> (Vec<u8>, [usize; 2]) {
    let shot = |gb: &GameBoy| {
        settings::screen_rgba(
            gb.screen(),
            o.palette.unwrap_or(Palette::Grey),
            o.color_correction,
        )
    };
    match partner {
        Some(gb2) => settings::side_by_side(shot(gb), shot(gb2)),
        None => shot(gb),
    }
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Runs a console command that sets something up; its errors abort the run.
fn command(console: &mut Console, line: &str, gb: &mut GameBoy) -> Result<(), String> {
    let out = console.execute(line, gb);
    match out.strip_prefix("error: ") {
        Some(e) => Err(format!("{line}: {e}")),
        None => Ok(()),
    }
}

/// One line of a Gameboy Doctor log: registers, then PC and the 4 bytes there.
fn doctor_line(gb: &GameBoy) -> String {
    let r = gb.registers();
    let mem: Vec<String> = (0..4)
        .map(|i| format!("{:02X}", gb.peek(r.pc.wrapping_add(i))))
        .collect();
    format!(
        "A:{:02X} F:{:02X} B:{:02X} C:{:02X} D:{:02X} E:{:02X} H:{:02X} L:{:02X} SP:{:04X} PC:00:{:04X} PCMEM:{}",
        r.a,
        r.f,
        r.b,
        r.c,
        r.d,
        r.e,
        r.h,
        r.l,
        r.sp,
        r.pc,
        mem.join(",")
    )
}

fn save_png(path: &Path, rgba: &[u8], [width, height]: [usize; 2]) -> Result<(), Box<dyn Error>> {
    let file =
        fs::File::create(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_options() {
        let o = parse_args(&args(
            "game.gb --frames 30 --press 10:a+start:2 --break $0150 --serial --model cgb",
        ))
        .unwrap()
        .unwrap();
        assert_eq!(o.rom, Some(PathBuf::from("game.gb")));
        assert_eq!(o.frames, 30);
        assert_eq!(o.breaks, ["$0150"]);
        assert!(o.serial);
        assert_eq!(o.model, Some(Model::Cgb));
        let p = &o.presses[0];
        assert_eq!(p.buttons, [Button::A, Button::Start]);
        assert!(!p.held_at(9) && p.held_at(10) && p.held_at(11) && !p.held_at(12));
        let o = parse_args(&args("game.gb --tilt 5:-0.5,1:9"))
            .unwrap()
            .unwrap();
        let t = &o.tilts[0];
        assert_eq!((t.from, t.x, t.y, t.frames), (5, -0.5, 1.0, 9));
        let o = parse_args(&args("game.gb --sav in.sav --off 2h --write-sav out.sav"))
            .unwrap()
            .unwrap();
        assert_eq!(o.off, Some(7200));
        assert_eq!(o.write_sav, Some(PathBuf::from("out.sav")));
        let o = parse_args(&args("game.gb --cheats --cheat 3 --cheat 010A10C1"))
            .unwrap()
            .unwrap();
        assert!(o.load_cheats);
        assert_eq!(o.cheats, ["3", "010A10C1"]);
        let o = parse_args(&args("a.gb --link b.gb --link-sav b.sav --press2 5:a"))
            .unwrap()
            .unwrap();
        assert_eq!(o.link, Some(PathBuf::from("b.gb")));
        assert_eq!(o.link_sav, Some(PathBuf::from("b.sav")));
        assert_eq!(o.presses2[0].buttons, [Button::A]);
        assert_eq!(parse_duration("90"), Ok(90));
        assert_eq!(parse_duration("3d"), Ok(3 * 86_400));
    }

    #[test]
    fn rejects_bad_options() {
        assert!(parse_args(&args("--frames 3")).is_err(), "no ROM");
        assert!(parse_args(&args("a.gb --press 10")).is_err());
        assert!(parse_args(&args("a.gb --tilt 10:1")).is_err());
        assert!(parse_args(&args("a.gb --bogus")).is_err());
        assert!(
            parse_args(&args("a.gb --press2 5:a")).is_err(),
            "--press2 without --link"
        );
        assert!(parse_args(&args("a.gb --link b.gb --break 100")).is_err());
        assert!(
            parse_args(&args("a.gb --off 1h")).is_err(),
            "--off without --sav"
        );
        assert!(parse_args(&args("a.gb --sav a.sav --off 1y")).is_err());
        assert!(parse_args(&args("a.gb --help")).unwrap().is_none());
    }

    /// An MBC3 game with a clock and 8 KiB of battery RAM that loops at $0100: one run
    /// writes its save, the next loads it an hour later and finds the clock moved on.
    #[test]
    fn battery_save_round_trip_catches_the_clock_up() {
        let dir = std::env::temp_dir().join(format!("freki-headless-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut rom = vec![0; 0x8000];
        rom[0x100..0x102].copy_from_slice(&[0x18, 0xFE]); // jr -2
        rom[0x147] = 0x10;
        rom[0x149] = 0x02;
        let rom_path = dir.join("clock.gb");
        fs::write(&rom_path, &rom).unwrap();
        let (first, second) = (dir.join("first.sav"), dir.join("second.sav"));
        let run_with = |extra: String| {
            let mut o = parse_args(&args(&format!("x --frames 60 {extra}")))
                .unwrap()
                .unwrap();
            o.rom = Some(rom_path.clone());
            assert_eq!(run(&o).unwrap(), 0);
        };
        // 60 frames are just over a second.
        run_with(format!("--write-sav {}", first.display()));
        run_with(format!(
            "--sav {} --off 1h --write-sav {}",
            first.display(),
            second.display()
        ));
        let (a, b) = (fs::read(&first).unwrap(), fs::read(&second).unwrap());
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(a.len(), 8192 + 48);
        // Seconds, minutes and hours are the first three 32-bit words after RAM.
        let hms = |d: &[u8]| [d[8192], d[8196], d[8200]];
        assert_eq!(hms(&a), [1, 0, 0]);
        assert_eq!(hms(&b), [2, 0, 1], "an hour off, then another second");
        assert_eq!(&b[8232..], &(1u64 + 3600 + 1).to_le_bytes());
    }

    /// Runs Blargg's first cpu_instrs test (if fetched) until it reports on serial.
    #[test]
    fn runs_a_rom_until_serial_text() {
        let rom = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../roms/test/cpu_instrs/01-special.gb");
        if !rom.exists() {
            eprintln!("skipping: {} not fetched", rom.display());
            return;
        }
        let mut o = parse_args(&args("x --frames 3000 --until-serial Passed"))
            .unwrap()
            .unwrap();
        o.rom = Some(rom);
        assert_eq!(run(&o).unwrap(), 0);
    }
}
