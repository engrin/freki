# freki

A Game Boy and Game Boy Color emulator written in Rust, for Windows, Linux and macOS.

Named after Freki, one of Odin's two wolves.

> **Status:** most original Game Boy games should run, with sound. Game Boy Color games
> run in colour (banked memory, palettes, tile attributes, double speed, VRAM DMA),
> `cgb-acid2` renders pixel-perfect and all of Blargg's `cgb_sound` tests pass.
> freki passes all of Blargg's `cpu_instrs`, `instr_timing`, `mem_timing`, `mem_timing-2`
> and `dmg_sound` tests, and renders `dmg-acid2` pixel-perfect. Cartridges with no MBC,
> MBC1, MBC2, MBC3 (with clock) and MBC5 (with rumble, passed on to the gamepad) work, with battery saves, save states, rewind and cheats.
> The PPU draws pixel by pixel with DMG-accurate timing: 22 of the 24 mealybug-tearoom
> mid-scanline tests match real hardware exactly (the other two differ by 4 and 3 pixels),
> and all 94 DMG Mooneye acceptance and MBC tests pass (OAM DMA, timer, serial,
> interrupt, PPU mode and VRAM/OAM access timing).
> On the Game Boy Color, the 58 Mooneye tests meant for all models or the CGB pass as well
> (boot state, I/O registers, interrupt and PPU timing), with or without its boot ROM.
> Mid-scanline drawing on the GBC matches real hardware (CPU CGB C) in 20 of the 27
> mealybug-tearoom-tests that have a CGB reference; the others involve changing the tile
> data select or sprite settings in the middle of a fetch.
> Of Gambatte's hardware tests (on-screen results), 1517 of 1651 pass on the DMG and 2494
> of 3023 on the GBC; the failures are listed in `crates/freki-core/tests/gambatte_failures.txt`.
> SameSuite and the AGE test ROMs are not run yet.
> Original Game Boy games can also run colourised on the GBC (with its boot ROM).

## Layout

| Path | What |
|---|---|
| `crates/freki-core` | Platform-independent emulation core (no OS, window or audio dependencies) |
| `crates/freki` | Desktop app: GUI with menus ([egui](https://crates.io/crates/egui)), gamepads ([gilrs](https://crates.io/crates/gilrs)), sound ([cpal](https://crates.io/crates/cpal)), save files, save states, cheats |
| `scripts/fetch-test-roms.sh` | Downloads freely distributable test ROMs into `roms/test/` |

## Build and run

Requires nightly Rust.

```sh
cargo run --release -- path/to/game.gb   # or start without a ROM and use File → Open ROM…
```

ROMs can also be dropped onto the window. Zipped ROMs (`.zip`) open directly. A patch
with the ROM's name next to it (`game.bps` or `game.ips` beside `game.gb` or `game.zip`)
is applied when the game loads, without changing the ROM file; battery saves and save
states stay named after the ROM. The menu bar has File (open/close ROM, recent
ROMs, quit), Emulation (pause, fast-forward, reset, console, boot ROM), States (save/load, slot), Cheats (on/off, cheat manager, memory
search), View (fullscreen, scale, integer scaling, palette, frame timing, debug views) and Help (controls, about).
Palettes: DMG green, greyscale, Pocket and Light (backlit). Game Boy Color games are shown
in their own colours, by default corrected to look like the GBC's screen (View → GBC colour
correction). The palette, scale, recent ROMs,
fast-forward speed, console, boot ROMs and controls are remembered between runs.

View → Tiles, Tile map, Sprites and Palettes show video memory live: all 384 tiles, either
tile map with the visible screen area outlined, the 40 sprites with position, tile and flags,
and the palettes. Hover tiles and map cells for their addresses. For Game Boy Color games
they show both VRAM banks (with a palette of your choice for tiles), the tile attributes in
the map, sprite palettes and banks, and all 16 colour palettes with their RGB555 values.

View → Debugger pauses and steps the CPU (F10 continue, F11 step one instruction, or step a
scanline or frame), shows registers, disassembly around PC, memory and the stack, and sets
breakpoints (click the dot next to an instruction) and read/write watchpoints on address
ranges. Hitting one pauses the game and opens the debugger. The terminal console has the
same tools: `regs`, `dis`, `step`, `break`, `watch`, `breaks`.

| Game Boy | Keyboard | Gamepad |
|---|---|---|
| D-pad | Arrow keys | D-pad or left stick |
| A / B | X / Z | Right / bottom face button (Nintendo layout) |
| Start / Select | Enter / Backspace | Start / Select (Back) |
| Open ROM / reset / quit | Ctrl+O / Ctrl+R / Ctrl+Q | |
| Pause | P | |
| Fullscreen | Alt+Enter (Esc leaves) | |

Gamepads can be plugged in while freki is running. The Game Boy buttons can be bound to
other keys and gamepad buttons under Emulation → Controls… (click a binding, then press the
key or button); the left stick always steers too.

### Headless runs

`freki --headless ROM [options]` runs a game without a window or sound, as fast as
possible, for debugging and scripted checks: run a number of frames, press buttons at
given frames, stop at breakpoints or watchpoints, log every instruction in
[Gameboy Doctor](https://github.com/robert/gameboy-doctor)'s format, then run console
commands, print the serial output, save a screenshot or a save state.
`freki --headless --help` lists the options.

```sh
freki --headless tetris.gb --frames 1200 --press 200:start --screenshot out.png --exec regs
freki --headless 01-special.gb --until-serial Passed --serial      # exit 0 once it passes
freki --headless game.gb --watch '$C0A0 w' --trace trace.log       # who writes $C0A0?
```

### Console and boot ROM

Emulation → Console chooses the hardware: **Auto** runs Game Boy Color games on a GBC and
all others on a Game Boy; **Game Boy Color** also runs original Game Boy games on a GBC, in
its compatibility mode, where they get colours like on a real GBC.

By default games start in the state the boot ROM leaves behind. To see the logo animation
and hear the sound, choose dumps of the boot ROMs under Emulation → Boot ROM → Choose file…
(the 256-byte Game Boy and the 2304-byte Game Boy Color boot ROM; they are Nintendo's code
and not included). The Game Boy Color boot ROM is also what picks the colours for original
Game Boy games; without it they are shown
in grey. Changes apply from the next reset or ROM load, and freki remembers them.

### Save states and rewind

| Key | Action |
|---|---|
| F5 | Save state to the current slot |
| F8 | Load state from the current slot |
| F6 / F7 | Previous / next slot (0–9) |
| R (hold) | Rewind, up to 20 seconds, at 2× speed (sound muted) |
| Tab (hold) | Fast-forward (2×–8×, set under Emulation; sound muted) |
| F12 | Screenshot: the screen as shown, saved next to the ROM as `<rom>-001.png`, … |

Save states are written next to the ROM as `<rom>.ss0` … `<rom>.ss9` (about 150 KB each).
They only load into the same game, and only in freki versions with the same state format.
Status messages appear in the window title.

### Cheats

Cheats → Cheat manager… lists, adds, toggles and removes codes; Cheats → Memory search…
finds the address of a value and turns it into a cheat. F9 turns all cheats on/off.

GameShark (`01VVLLHH`; on the Game Boy Color also `9xVVLLHH` for work RAM bank `x`) and
Game Genie (`ABC-DEF` or `ABC-DEF-GHI`) codes are read from
`<rom>.cht` next to the ROM, one per line with an optional name (`#` starts a comment,
`!` adds a code switched off):

```text
010A10C1 Infinite lives
! 00A-17B-C49 Start with all items
```

While the game runs you can type commands into the terminal freki was started from
(`help` lists them): add, toggle and save cheats, `peek`/`poke` memory, and search memory
to make new cheats. For example, to find the lives counter:

```text
search start          # lives = 3
search eq 3
                      # lose a life in the game
search down           # keeps addresses whose value decreased
search list
search add 0 9 Lives  # candidate 0 becomes GameShark code 0109xxxx
cheat save            # writes <rom>.cht
```

On the Game Boy Color the search covers all work RAM banks; results in banks 1–7 show as
`3:$D123` and become `93…` codes, which write to that bank whichever one the game has
selected.

Battery-backed games are saved next to the ROM as `<rom>.sav` (about once a second
while the game writes to it, and on exit).

The game runs at the Game Boy's own frame rate (~59.73 fps) in real time; sound stays in
sync through tiny (inaudible) adjustments of its sample rate. Without an audio device
freki still runs.

On Linux you need the X11/Wayland, ALSA and udev development headers
(`libxkbcommon-dev libwayland-dev libasound2-dev libudev-dev` on Debian/Ubuntu).

## Testing

```sh
./scripts/fetch-test-roms.sh                        # Blargg, dmg/cgb-acid2, mealybug-tearoom, Mooneye, Gambatte
cargo test --workspace                              # unit tests and all test ROMs (headless)
```

These run without a window: the Blargg tests read results from the serial port or
cartridge RAM, the Mooneye tests check the registers the ROM sets when it finishes, and the dmg-acid2 and cgb-acid2 tests compare the screen with the reference image (on a
mismatch, freki's screen is saved to `target/<test>-actual.png`). The CPU and timing tests
also run in CGB mode. If the ROMs have not
been fetched, those tests are skipped.

ROM files are never committed; see `.gitignore`.

## References

- [Pan Docs](https://gbdev.io/pandocs/): the hardware reference
- [Opcode table](https://gbdev.io/gb-opcodes/optables/)
- [Blargg's test ROMs](https://github.com/retrio/gb-test-roms), [dmg-acid2](https://github.com/mattcurrie/dmg-acid2), [cgb-acid2](https://github.com/mattcurrie/cgb-acid2), [Mooneye test suite](https://github.com/Gekkio/mooneye-test-suite), [mealybug-tearoom-tests](https://github.com/mattcurrie/mealybug-tearoom-tests), [Gambatte hwtests](https://github.com/pokemon-speedrunning/gambatte-core/tree/master/test/hwtests)

## License

MIT
