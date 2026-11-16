# freki

A Game Boy (DMG) emulator written in Rust, for Windows, Linux and macOS.

Named after Freki, one of Odin's two wolves.

> **Status:** most original Game Boy games should run, with sound.
> freki passes all of Blargg's `cpu_instrs`, `instr_timing`, `mem_timing`, `mem_timing-2`
> and `dmg_sound` tests, and renders `dmg-acid2` pixel-perfect. Cartridges with no MBC,
> MBC1, MBC2, MBC3 (with clock) and MBC5 work, with battery saves, save states, rewind and cheats.
> The PPU draws pixel by pixel with DMG-accurate timing: 22 of the 24 mealybug-tearoom
> mid-scanline tests match real hardware exactly (the other two differ by 4 and 3 pixels).
> Missing: Game Boy Color.

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

ROMs can also be dropped onto the window. The menu bar has File (open/close ROM, quit),
Emulation (pause, reset), States (save/load, slot), Cheats (on/off, cheat manager, memory
search), View (scale, palette) and Help (controls, about).

| Game Boy | Keyboard | Gamepad |
|---|---|---|
| D-pad | Arrow keys | D-pad or left stick |
| A / B | X / Z | Right / bottom face button (Nintendo layout) |
| Start / Select | Enter / Backspace | Start / Select (Back) |
| Open ROM / reset / quit | Ctrl+O / Ctrl+R / Ctrl+Q | |
| Pause | P | |

Gamepads can be plugged in while freki is running.

### Save states and rewind

| Key | Action |
|---|---|
| F5 | Save state to the current slot |
| F8 | Load state from the current slot |
| F6 / F7 | Previous / next slot (0–9) |
| R (hold) | Rewind, up to 20 seconds, at 2× speed (sound muted) |

Save states are written next to the ROM as `<rom>.ss0` … `<rom>.ss9` (about 40 KB each).
They only load into the same game, and only in freki versions with the same state format.
Status messages appear in the window title.

### Cheats

Cheats → Cheat manager… lists, adds, toggles and removes codes; Cheats → Memory search…
finds the address of a value and turns it into a cheat. F9 turns all cheats on/off.

GameShark (`01VVLLHH`) and Game Genie (`ABC-DEF` or `ABC-DEF-GHI`) codes are read from
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

Battery-backed games are saved next to the ROM as `<rom>.sav` (about once a second
while the game writes to it, and on exit).

Emulation speed follows the sound card's clock, so audio stays in sync. Without an audio
device freki still runs, paced at ~60 fps.

On Linux you need the X11/Wayland, ALSA and udev development headers
(`libxkbcommon-dev libwayland-dev libasound2-dev libudev-dev` on Debian/Ubuntu).

## Testing

```sh
./scripts/fetch-test-roms.sh                        # Blargg, dmg-acid2, mealybug-tearoom
cargo test --workspace                              # unit tests, Blargg, dmg-acid2 (headless)
```

These run without a window: the Blargg tests read results from the serial port or
cartridge RAM, and the dmg-acid2 test compares the screen with the reference image (on a
mismatch, freki's screen is saved to `target/dmg-acid2-actual.png`). If the ROMs have not
been fetched, those tests are skipped.

ROM files are never committed; see `.gitignore`.

## References

- [Pan Docs](https://gbdev.io/pandocs/): the hardware reference
- [Opcode table](https://gbdev.io/gb-opcodes/optables/)
- [Blargg's test ROMs](https://github.com/retrio/gb-test-roms), [dmg-acid2](https://github.com/mattcurrie/dmg-acid2), [Mooneye test suite](https://github.com/Gekkio/mooneye-test-suite)

## License

MIT
