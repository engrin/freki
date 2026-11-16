# freki

A Game Boy (DMG) emulator written in Rust, for Windows, Linux and macOS.

Named after Freki, one of Odin's two wolves.

> **Status:** most original Game Boy games should run, without sound.
> The CPU passes all of Blargg's `cpu_instrs` tests and the PPU renders `dmg-acid2`
> pixel-perfect. Cartridges with no MBC, MBC1, MBC2, MBC3 (with clock) and MBC5 work,
> with battery saves. Missing: sound, Game Boy Color, cycle-accurate timing.

## Layout

| Path | What |
|---|---|
| `crates/freki-core` | Platform-independent emulation core (no OS, window or audio dependencies) |
| `crates/freki` | Desktop frontend: window, input and frame pacing ([minifb](https://crates.io/crates/minifb)) |
| `scripts/fetch-test-roms.sh` | Downloads freely distributable test ROMs into `roms/test/` |

## Build and run

Requires nightly Rust.

```sh
cargo run --release -- path/to/game.gb
```

| Key | Game Boy |
|---|---|
| Arrow keys | D-pad |
| X / Z | A / B |
| Enter / Backspace | Start / Select |
| Esc | Quit |

Battery-backed games are saved next to the ROM as `<rom>.sav` (about once a second
while the game writes to it, and on exit).

On Linux you need the X11/Wayland development headers
(`libxkbcommon-dev libwayland-dev` on Debian/Ubuntu).

## Testing

```sh
./scripts/fetch-test-roms.sh                        # Blargg cpu_instrs + dmg-acid2
cargo test --workspace                              # unit tests, Blargg cpu_instrs, dmg-acid2 (headless)
cargo test -p freki-core --test blargg -- --ignored # combined cpu_instrs.gb
```

These run without a window: the Blargg tests read results from the serial port, and
the dmg-acid2 test compares the screen with the reference image (on a mismatch, freki's
screen is saved to `target/dmg-acid2-actual.png`). If the ROMs have not been fetched,
those tests are skipped.

ROM files are never committed; see `.gitignore`.

## References

- [Pan Docs](https://gbdev.io/pandocs/): the hardware reference
- [Opcode table](https://gbdev.io/gb-opcodes/optables/)
- [Blargg's test ROMs](https://github.com/retrio/gb-test-roms), [dmg-acid2](https://github.com/mattcurrie/dmg-acid2), [Mooneye test suite](https://github.com/Gekkio/mooneye-test-suite)

## License

MIT
