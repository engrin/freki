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

use freki_core::{BootRom, Button, GameBoy, Model, SCREEN_HEIGHT, SCREEN_WIDTH, Screen};

use crate::console::{self, Console};
use crate::session::{self, Start};
use crate::settings::{self, Palette};

pub const USAGE: &str = "\
usage: freki --headless ROM [options]

Runs ROM without a window or sound, as fast as possible.

run:
  --frames N              frames to run (default 600, about 10 seconds)
  --until-serial TEXT     stop once the serial output contains TEXT (exit 1 if it never does)
  --model dmg|cgb         console (default: GBC for colour games, else Game Boy)
  --boot FILE             start with this boot ROM (DMG or GBC, by size)
  --sav FILE              load this battery save (never written back)
  --load-state FILE       start from a save state (.ss0 … .ss9)
  --press FRAME:KEYS[:N]  hold KEYS (a+b+start+select+up+down+left+right) from FRAME
                          for N frames (default 6); repeatable
debugging:
  --break ADDR            stop when PC reaches ADDR; repeatable
  --watch 'ADDR [END] [r|w|rw]'
                          stop when the CPU accesses ADDR..END (default w); repeatable
  --trace FILE            log every instruction in Gameboy Doctor's format
                          (A:.. F:.. … PC:00:0100 PCMEM:..)
at the end (in this order):
  --exec 'COMMAND'        run a console command (regs, dis, peek $C000 16, …); repeatable
  --serial                print what the game sent over the serial port
  --screenshot FILE.png   save the screen
  --palette NAME          DMG shades for the screenshot: green, grey (default), pocket, light
  --color-correction      GBC colours as the GBC screen shows them (default: raw)
  --save-state FILE       save the machine state

numbers: decimal, or hex with $ or 0x (e.g. $C0A0)
exit status: 0 finished, 2 stopped at a breakpoint or watchpoint, 1 error";

#[derive(Default)]
struct Options {
    rom: Option<PathBuf>,
    frames: u64,
    until_serial: Option<String>,
    model: Option<Model>,
    boot: Option<PathBuf>,
    sav: Option<PathBuf>,
    load_state: Option<PathBuf>,
    presses: Vec<Press>,
    breaks: Vec<String>,
    watches: Vec<String>,
    trace: Option<PathBuf>,
    exec: Vec<String>,
    serial: bool,
    screenshot: Option<PathBuf>,
    palette: Option<Palette>,
    color_correction: bool,
    save_state: Option<PathBuf>,
}

/// Buttons held from frame `from` for `frames` frames.
struct Press {
    from: u64,
    frames: u64,
    buttons: Vec<Button>,
}

impl Press {
    fn held_at(&self, frame: u64) -> bool {
        (self.from..self.from + self.frames).contains(&frame)
    }
}

/// Runs headless with `args` (everything after the program name, `--headless` removed).
/// Returns the process exit status.
pub fn main(args: &[String]) -> i32 {
    let options = match parse_args(args) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{USAGE}");
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
                    m => return Err(format!("unknown model {m:?} (dmg or cgb)")),
                });
            }
            "--boot" => o.boot = Some(value(arg)?.into()),
            "--sav" => o.sav = Some(value(arg)?.into()),
            "--load-state" => o.load_state = Some(value(arg)?.into()),
            "--press" => o.presses.push(parse_press(&value(arg)?)?),
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
            "--save-state" => o.save_state = Some(value(arg)?.into()),
            a if a.starts_with('-') => return Err(format!("unknown option {a}")),
            _ if o.rom.is_none() => o.rom = Some(arg.into()),
            a => return Err(format!("unexpected argument {a:?}")),
        }
    }
    if o.rom.is_none() {
        return Err("no ROM given".into());
    }
    Ok(Some(o))
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

fn run(o: &Options) -> Result<i32, Box<dyn Error>> {
    let rom_path = o.rom.as_deref().expect("checked by parse_args");
    let rom = fs::read(rom_path).map_err(|e| format!("cannot read {}: {e}", rom_path.display()))?;
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
    let mut gb = session::power_on(rom, &start)?;
    if let Some(path) = &o.sav {
        gb.load_save_data(&read(path)?);
    }
    if let Some(path) = &o.load_state {
        gb.load_state(&read(path)?)?;
    }

    let mut console = Console::detached(rom_path.with_extension("cht"));
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

    let mut status = 0;
    let mut pressed: Vec<Button> = Vec::new();
    let mut frame = 0;
    while frame < o.frames {
        let held: Vec<Button> = o
            .presses
            .iter()
            .filter(|p| p.held_at(frame))
            .flat_map(|p| p.buttons.iter().copied())
            .collect();
        for &b in &pressed {
            if !held.contains(&b) {
                gb.set_button(b, false);
            }
        }
        for &b in &held {
            gb.set_button(b, true);
        }
        pressed = held;

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
                println!("frame {frame}: {}", console::describe_stop(&stop));
                println!("{}", console::registers(&gb));
                println!("{}", console.execute("dis", &mut gb));
                status = 2;
                break;
            }
            Ok(None) => {}
            Err(e) => {
                println!("frame {frame}: {e}");
                println!("{}", console::registers(&gb));
                status = 1;
                break;
            }
        }
        frame += 1;
        if let Some(text) = &o.until_serial
            && String::from_utf8_lossy(gb.serial_output()).contains(text.as_str())
        {
            println!("frame {frame}: serial output contains {text:?}");
            break;
        }
    }
    if status == 0 && frame == o.frames {
        println!("ran {frame} frame{}", if frame == 1 { "" } else { "s" });
        if let Some(text) = &o.until_serial {
            println!("serial output never contained {text:?}");
            status = 1;
        }
    }
    if let Some(out) = trace.as_mut() {
        out.flush()?;
    }

    for line in &o.exec {
        println!("> {line}");
        println!("{}", console.execute(line, &mut gb));
    }
    if o.serial {
        println!(
            "serial output:\n{}",
            String::from_utf8_lossy(gb.serial_output())
        );
    }
    if let Some(path) = &o.screenshot {
        save_png(
            path,
            &screen_rgba(&gb, o.palette.unwrap_or(Palette::Grey), o.color_correction),
        )?;
        println!("screenshot saved to {}", path.display());
    }
    if let Some(path) = &o.save_state {
        fs::write(path, gb.save_state())
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        println!("state saved to {}", path.display());
    }
    Ok(status)
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

fn screen_rgba(gb: &GameBoy, palette: Palette, correct: bool) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(SCREEN_WIDTH * SCREEN_HEIGHT * 4);
    match gb.screen() {
        Screen::Shades(shades) => {
            let colors = palette.colors();
            for &shade in shades {
                let [r, g, b] = colors[(shade & 3) as usize];
                rgba.extend([r, g, b, 0xFF]);
            }
        }
        Screen::Rgb555(pixels) => {
            for &color in pixels {
                let [r, g, b] = settings::cgb_color(color, correct);
                rgba.extend([r, g, b, 0xFF]);
            }
        }
    }
    rgba
}

fn save_png(path: &Path, rgba: &[u8]) -> Result<(), Box<dyn Error>> {
    let file =
        fs::File::create(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(
        BufWriter::new(file),
        SCREEN_WIDTH as u32,
        SCREEN_HEIGHT as u32,
    );
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
    }

    #[test]
    fn rejects_bad_options() {
        assert!(parse_args(&args("--frames 3")).is_err(), "no ROM");
        assert!(parse_args(&args("a.gb --press 10")).is_err());
        assert!(parse_args(&args("a.gb --bogus")).is_err());
        assert!(parse_args(&args("a.gb --help")).unwrap().is_none());
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
