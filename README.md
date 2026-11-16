# freki

A Game Boy (DMG) emulator written in Rust, for Windows, Linux and macOS.

Named after Freki, one of Odin's two wolves.

> **Status:** early skeleton. The memory map, timer, joypad and frame timing are in place.
> The CPU only knows a handful of opcodes, the PPU does not draw anything yet, and only
> ROM-only cartridges (no MBC) load.

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

On Linux you need the X11/Wayland development headers
(`libxkbcommon-dev libwayland-dev` on Debian/Ubuntu).

## Testing

```sh
cargo test --workspace
./scripts/fetch-test-roms.sh            # Blargg cpu_instrs + dmg-acid2
cargo run --release -- roms/test/cpu_instrs/01-special.gb
```

ROM files are never committed; see `.gitignore`.

## References

- [Pan Docs](https://gbdev.io/pandocs/): the hardware reference
- [Opcode table](https://gbdev.io/gb-opcodes/optables/)
- [Blargg's test ROMs](https://github.com/retrio/gb-test-roms), [dmg-acid2](https://github.com/mattcurrie/dmg-acid2), [Mooneye test suite](https://github.com/Gekkio/mooneye-test-suite)

## License

MIT
