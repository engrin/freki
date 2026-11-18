# freki

A Game Boy and Game Boy Color emulator written in Rust, for Windows, Linux and macOS.

Named after Freki, one of Odin's two wolves.

## Features

### Emulation

- **Game Boy and Game Boy Color**, including double speed, VRAM DMA, colour palettes and
  tile attributes, and original Game Boy games colourised by the GBC's boot ROM
- **Cycle-accurate core:** the CPU, timer, serial port and OAM DMA are timed to the
  M-cycle, and the PPU draws pixel by pixel, so mid-scanline effects come out right
- **Sound** on all four channels, kept in sync with the picture without audible
  adjustments
- **Cartridges:** no MBC, MBC1 (including multicarts), MMM01 collections, MBC2, MBC3
  with real-time clock (kept in the save file, so it moves on while freki is closed),
  MBC5 with rumble, MBC7 with tilt sensor (tilted with the D-pad, arrow keys or left
  stick), Hudson's HuC1 and HuC3 (with real-time clock), and Bandai's TAMA5
  (Tamagotchi 3, with its clock)
- **Optional boot ROMs:** start with the real Game Boy or Game Boy Color boot animation, or
  straight into the game

### Playing

- **Open ROMs your way:** `.gb`, `.gbc` or `.zip`, from the menu, the recent list, the
  command line or by dropping them on the window
- **Soft patching:** a `.bps` or `.ips` patch next to the ROM is applied when the game
  loads (translations, romhacks); the ROM file is never changed
- **Battery saves** written automatically, **10 save-state slots**, **rewind** (up to 20
  seconds) and **fast-forward** (2×–8×)
- **Fullscreen** and window scales from 2× to 6×, with sharp integer scaling (can be
  switched off)
- **Four Game Boy palettes** (DMG green, greyscale, Pocket, Light), and **GBC colour
  correction** that looks like the real screen
- **Keyboard and gamepads**, plugged in at any time. The Controls window shows a Game Boy
  with every binding next to its button, and rebinding is a click and a key press.
- **Controller-aware labels:** Xbox (360, One, Series), PlayStation 4 and 5, Steam and
  Switch Pro controllers are recognised and shown with their own buttons (Ⓐ, ✕, Options,
  Menu, …)
- **Rumble:** rumble cartridges shake your gamepad
- **Cheats:** GameShark and Game Genie codes, a cheat manager, and a memory search that
  finds values such as lives or money and turns them into cheats
- **Screenshots** of the screen as shown

### Debugging

- **Debugger:** pause, step by instruction, scanline or frame; registers, disassembly,
  memory and stack; breakpoints and read/write watchpoints
- **Video memory viewers:** tiles, tile maps, sprites and palettes, live, for both VRAM
  banks on the GBC
- **Terminal console:** the same debugger, cheat and memory commands, typed while the game
  runs
- **Headless mode:** run a game without a window for scripts and regression checks,
  with scripted button presses, cheats, instruction traces in Gameboy Doctor's format, screenshots
  and save states

## Accuracy

| Test suite | Result |
|---|---|
| Blargg's `cpu_instrs`, `instr_timing`, `mem_timing`, `mem_timing-2`, `dmg_sound`, `cgb_sound` | all pass |
| `dmg-acid2`, `cgb-acid2` | pixel-perfect |
| Mooneye acceptance and MBC tests (DMG) | 94 / 94 |
| Mooneye tests for all models or the CGB, with or without the boot ROM | 58 / 58 |
| mealybug-tearoom mid-scanline tests (DMG) | 22 / 24 (the other two are 3–4 pixels off) |
| mealybug-tearoom mid-scanline tests (CGB) | 20 / 27 |
| Gambatte hardware tests | DMG 1517 / 1651, GBC 2505 / 3023 |

The Gambatte failures are listed in `crates/freki-core/tests/gambatte_failures.txt`.
SameSuite and the AGE test ROMs are not run yet.

## Getting started

freki needs nightly Rust. On Linux, install the X11/Wayland, ALSA and udev development
headers first (`libxkbcommon-dev libwayland-dev libasound2-dev libudev-dev` on
Debian/Ubuntu).

```sh
cargo run --release -- path/to/game.gb    # or start without a ROM: File → Open ROM…
```

## Controls

| | Keyboard | Gamepad |
|---|---|---|
| D-pad | Arrow keys | D-pad or left stick |
| A / B | X / Z | Right / bottom face button |
| Start / Select | Enter / Backspace | Start / Select |
| Save / load state | F5 / F8 | |
| Previous / next slot | F6 / F7 | |
| Rewind | hold R | |
| Fast-forward | hold Tab | |
| Pause | P | |
| Cheats on / off | F9 | |
| Screenshot | F12 | |
| Fullscreen | Alt+Enter (Esc leaves) | |
| Open ROM / reset / quit | Ctrl+O / Ctrl+R / Ctrl+Q | |

The Game Boy buttons can be rebound under **Emulation → Controls…**. Settings such as the
palette, scale, controls, recent ROMs and boot ROMs are remembered between runs.

## Files next to the ROM

| File | What |
|---|---|
| `game.sav` | Battery save, written about once a second while the game saves, and on exit. A cartridge clock is stored at the end in the format VBA-M, BGB and mGBA use for MBC3 |
| `game.ss0` … `game.ss9` | Save states (about 150 KB each; they load only into the same game and freki state format) |
| `game.cht` | Cheats, saved whenever one is added, removed or switched on or off |
| `game.bps`, `game.ips` | Patch applied when the game loads |
| `game-001.png`, … | Screenshots |

## Consoles and boot ROMs

**Emulation → Console** chooses the hardware. **Auto** runs Game Boy Color games on a GBC
and everything else on a Game Boy. **Game Boy Color** also runs original Game Boy games on
a GBC, in colour as on the real thing.

By default games start in the state the boot ROM leaves behind. For the logo animation and
sound, choose your own boot ROM dumps under **Emulation → Boot ROM → Choose file…**: the
256-byte Game Boy and the 2304-byte Game Boy Color boot ROM. They are Nintendo's code and
not included. The GBC boot ROM also picks the colours for original Game Boy games; without
it they are shown in grey.

## Cheats

**Cheats → Cheat manager…** adds, toggles and removes codes, and **Cheats → Memory
search…** finds the address of a value. Codes are kept in `game.cht` and saved as soon as
the list changes, one per line with an optional name (`#` starts a comment, `!` adds a code
switched off, `+` joins several codes into one cheat):

```text
010A10C1 Infinite lives
! 00A-17B-C49 Start with all items
```

RetroArch's cheat files are read too, and saved back in their format. A file named as in
cheat databases, such as `metal-gear-solid-usa.cht` next to `Metal Gear Solid (USA).gbc`,
is imported when the game has no `game.cht` yet.

GameShark (`01VVLLHH`; on the GBC also `9xVVLLHH` for work RAM bank `x`) and Game Genie
(`ABC-DEF` or `ABC-DEF-GHI`) codes are supported. The same tools work from the terminal
freki was started from (`help` lists the commands). For example, to find the lives counter:

```text
search start          # lives = 3
search eq 3
                      # lose a life in the game
search down           # keeps addresses whose value decreased
search list
search add 0 9 Lives  # candidate 0 becomes GameShark code 0109xxxx
cheat save            # writes game.cht
```

## Headless mode

`freki --headless ROM [options]` runs a game as fast as possible without a window or
sound. It can press buttons at given frames, stop at breakpoints or watchpoints, and log
every instruction. At the end it can run console commands, print the serial output, and
save a screenshot, a save state or the battery save. A battery save can be loaded as if
the Game Boy had been off for a while, so the cartridge clock catches up. `freki --headless --help` lists the options.

```sh
freki --headless tetris.gb --frames 1200 --press 200:start --screenshot out.png --exec regs
freki --headless 01-special.gb --until-serial Passed --serial      # exit 0 once it passes
freki --headless game.gb --watch '$C0A0 w' --trace trace.log       # who writes $C0A0?
freki --headless rtc.gbc --write-sav a.sav && freki --headless rtc.gbc --sav a.sav --off 2h
freki --headless game.gbc --cheats --cheat 3 --cheat 010A10C1   # cheat 3 of game.cht, and a code
```

## Development

| Path | What |
|---|---|
| `crates/freki-core` | Platform-independent emulation core (no OS, window or audio dependencies) |
| `crates/freki` | Desktop app: [egui](https://crates.io/crates/egui) GUI, [gilrs](https://crates.io/crates/gilrs) gamepads, [cpal](https://crates.io/crates/cpal) sound |
| `scripts/fetch-test-roms.sh` | Downloads freely distributable test ROMs into `roms/test/` |

```sh
./scripts/fetch-test-roms.sh      # Blargg, dmg/cgb-acid2, mealybug-tearoom, Mooneye, Gambatte
cargo test --workspace            # unit tests and all test ROMs, without a window
```

The test ROMs run headless:
- Blargg's tests report through the serial port or cartridge RAM.
- Mooneye's tests leave their result in the CPU registers.
- The acid2 tests compare the screen with a reference image. On a mismatch, freki's screen
  is saved to `target/<test>-actual.png`.

Tests whose ROMs have not been fetched are skipped. ROM files are never committed.

## References

- [Pan Docs](https://gbdev.io/pandocs/), the hardware reference, and the
  [opcode table](https://gbdev.io/gb-opcodes/optables/)
- Test ROMs: [Blargg's](https://github.com/retrio/gb-test-roms),
  [dmg-acid2](https://github.com/mattcurrie/dmg-acid2),
  [cgb-acid2](https://github.com/mattcurrie/cgb-acid2),
  [Mooneye test suite](https://github.com/Gekkio/mooneye-test-suite),
  [mealybug-tearoom-tests](https://github.com/mattcurrie/mealybug-tearoom-tests),
  [Gambatte hwtests](https://github.com/pokemon-speedrunning/gambatte-core/tree/master/test/hwtests)

## License

MIT
