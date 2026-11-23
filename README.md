<h1 align="center">freki</h1>

<p align="center">
A Game Boy, Game Boy Color and Super Game Boy emulator written in Rust,<br>
for Windows, Linux and macOS. Named after Freki, one of Odin's two wolves.
</p>

<p align="center">
<img src="https://img.shields.io/badge/rust-nightly-555?logo=rust" alt="Rust nightly">
<img src="https://img.shields.io/badge/platforms-windows%20%7C%20linux%20%7C%20macos-2f6db5" alt="Windows, Linux, macOS">
<img src="https://img.shields.io/badge/mooneye-152%20%2F%20152-3d7a3d" alt="Mooneye 152 / 152">
<img src="https://img.shields.io/badge/acid2-pixel--perfect-3d7a3d" alt="acid2 pixel-perfect">
<img src="https://img.shields.io/badge/license-MIT-b5592f" alt="MIT license">
</p>

<p align="center">
<a href="#features">Features</a> ·
<a href="#download">Download</a> ·
<a href="#getting-started">Getting started</a> ·
<a href="#controls">Controls</a> ·
<a href="#accuracy">Accuracy</a> ·
<a href="#guide">Guide</a> ·
<a href="#development">Development</a>
</p>

<p align="center">
<img src="docs/images/desktop.png" alt="freki running The Legend of Zelda: Link's Awakening DX, with the debugger window next to it">
</p>

<table align="center">
<tr>
<td align="center"><img src="docs/images/zelda-dx.png" width="200" alt="Link's Awakening DX intro"><br>Game Boy Color</td>
<td align="center"><img src="docs/images/pokemon-sgb.png" width="204" alt="Pokémon Blue with its Super Game Boy border"><br>Super Game Boy borders</td>
<td align="center"><img src="docs/images/donkey-kong-sgb.png" width="204" alt="Donkey Kong with its Super Game Boy border"><br>SGB colour and sound</td>
<td align="center"><img src="docs/images/camera.png" width="200" alt="Game Boy Camera title screen"><br>Game Boy Camera</td>
</tr>
</table>

## Features

| | |
|---|---|
| **Accurate**<br>cycle-timed CPU, timer and DMA, and a pixel-by-pixel PPU for mid-scanline effects | **Every console**<br>Game Boy, Game Boy Color and Super Game Boy, with SGB borders, palettes and the SNES's own sound |
| **Every cartridge**<br>MBC1–7, MMM01, HuC1/HuC3, TAMA5, clocks, rumble, tilt, and unlicensed carts | **Accessories**<br>link cable and infrared between two Game Boys, Game Boy Printer, Game Boy Camera with a webcam |
| **Comfortable play**<br>rewind, fast-forward, 10 save states with previews, soft patching, gamepads with their own button labels | **Cheats**<br>GameShark, Game Genie and Xploder codes, a cheat manager and a memory search |
| **For developers**<br>debugger with labels from `.sym` files, VRAM and sound viewers, a terminal console | **Headless mode**<br>scripted runs with button presses, traces, screenshots and save states |

<details>
<summary><b>Emulation in detail</b></summary>

- **Game Boy and Game Boy Color**, including double speed, VRAM DMA, colour palettes and
  tile attributes, and original Game Boy games colourised by the GBC's boot ROM
- **Cycle-accurate core:** the CPU, timer, serial port and OAM DMA are timed to the
  M-cycle, and the PPU draws pixel by pixel, so mid-scanline effects come out right
- **Sound** on all four channels, kept in sync with the picture without audible
  adjustments
- **Cartridges:** no MBC, MBC1 (including multicarts), MMM01 collections, MBC2, MBC3
  with real-time clock (kept in the save file, so it moves on while freki is closed) and
  its MBC30 variant (Japanese Pokémon Crystal: 64 KiB save RAM), MBC5 with rumble, MBC6
  with its flash chip (Net de Get, which downloads minigames into it), MBC7 with tilt
  sensor (tilted with the D-pad, arrow keys or left stick), Hudson's HuC1 and HuC3 (with
  real-time clock), Bandai's TAMA5 (Tamagotchi 3, with its clock), and the **Game Boy
  Camera** (see below)
- **Unlicensed cartridges**, recognised from the ROM since their headers don't say: Wisdom
  Tree (Spiritual Warfare, the NIV Bible, …), Sachen MMC1 and MMC2 (with the trick that
  gets them past the boot ROM's logo check) and M161 (Mani 4-in-1)
- **Super Game Boy:** the borders and colours SGB games bring along (Pokémon, Donkey Kong,
  Dragon Warrior Monsters, …), with palettes, colour attributes, screen masking and the
  multiplayer request games use to detect an SGB; with the SGB cartridge's SNES ROM also
  the **SGB's own sound** (sound effects and music games play on the SNES sound chip, such
  as Donkey Kong's)
- **Link cable and infrared:** a second game runs next to the first, connected by cable
  and facing it with the infrared port, for trading, two-player games (Tetris, Pokémon, …)
  and infrared features (the GBC's port and Hudson's HuC1/HuC3 cartridges; tested:
  Pokémon Crystal's Mystery Gift)
- **Game Boy Printer:** games print onto paper shown in a window, and every page is saved
  as a PNG (tested: Super Mario Bros. Deluxe calendars, Link's Awakening DX photos, Pokémon
  Crystal box lists)
- **Optional boot ROMs:** start with the real Game Boy or Game Boy Color boot animation, or
  straight into the game

</details>

<details>
<summary><b>Playing in detail</b></summary>

- **Open ROMs your way:** `.gb`, `.gbc` or `.zip`, from the menu, the recent list, the
  command line or by dropping them on the window
- **Soft patching:** a `.bps` or `.ips` patch next to the ROM is applied when the game
  loads (translations, romhacks); the ROM file is never changed
- **Battery saves** written automatically (RetroArch's `.srm` saves work too), **rewind**
  (up to 20 seconds) and **fast-forward** (2×–8×)
- **10 save-state slots** with previews: the States menu shows each slot's screen and how
  long ago it was saved (click to select, double-click to load). State files are PNGs of
  the screen with the state inside, so image viewers show them too
- **Volume and mute** (Emulation menu, or M), **turbo buttons** for A and B, and **pause
  in the background:** the game waits while no freki window has focus (can be switched
  off; never while recording video)
- **Fullscreen** and window scales from 2× to 6×, with sharp integer scaling (can be
  switched off)
- **Settings per game:** a game can keep its own console, palette, colour correction and
  frame blending (Emulation → Settings for this game)
- **Frame blending** like the real console's slow LCD, so sprites that games flicker every
  other frame look steady (View → Frame blending)
- **Four Game Boy palettes** (DMG green, greyscale, Pocket, Light), and **GBC colour
  correction** that looks like the real screen
- **Keyboard and gamepads**, plugged in at any time. The Controls window shows a Game Boy
  with every binding next to its button, and rebinding is a click and a key press.
- **Controller-aware labels:** Xbox (360, One, Series), PlayStation 4 and 5, Steam and
  Switch Pro controllers are recognised and shown with their own buttons (Ⓐ, ✕, Options,
  Menu, …)
- **Rumble:** rumble cartridges shake your gamepad
- **Cheats:** GameShark, Game Genie and Xploder codes, a cheat manager, and a memory
  search that finds values such as lives or money and turns them into cheats. RetroArch
  cheat files are read and written, and codes made for another version of the game are
  marked
- **Screenshots** of the screen as shown, and **video recording** with sound (WebM: AV1
  picture, Opus sound), encoded inside freki without external tools

</details>

<details>
<summary><b>Debugging in detail</b></summary>

- **Debugger:** pause, step by instruction, scanline or frame; registers, disassembly,
  memory and stack; breakpoints and read/write watchpoints
- **Symbol files:** with the assembler's `game.sym` next to the ROM (RGBDS, WLA-DX and
  others write them), the debugger and console show labels instead of addresses
  (`CALL UpdateSprites`, `LD A,(wLives)`, `breakpoint at $4003 (Main.loop)`), in the
  ROM bank that is mapped in, and take labels wherever they take an address
  (`break Main.loop`, `peek wLives`). Resetting reloads the ROM and its labels, so a
  rebuilt game is one Ctrl+R away
- **Sound channels:** what each of the four channels plays (pitch and note, duty, volume
  and envelope, sweep, length, left/right), with a live oscilloscope and channel 3's wave
  RAM; mute or solo channels to hear, or record, one part of the music alone
- **Video memory viewers:** tiles, tile maps, sprites and palettes, live, for both VRAM
  banks on the GBC
- The debugger, the viewers, the sound channels, the cheat manager, the memory search and
  the printer each open as a window of their own, to place next to the game or on another
  screen. They reopen where they were left, at the same size, and the emulator's hotkeys
  (function keys, P, M, Ctrl shortcuts) work in them too
- **Terminal console:** the same debugger, cheat and memory commands, typed while the game
  runs
- **Headless mode:** run a game without a window for scripts and regression checks,
  with scripted button presses, cheats, linked games, instruction traces in Gameboy
  Doctor's format, screenshots and save states

</details>

## Download

Ready-made builds are on the [releases page](https://github.com/engrin/freki/releases):

- **macOS** (Apple Silicon and Intel, macOS 11 or later): a `.dmg` with `freki.app`. It
  is not notarized, so macOS refuses to open it the first time; then choose **System
  Settings → Privacy & Security → Open Anyway**.
- **Windows** (64-bit): a `.zip` with `freki.exe`. SmartScreen may warn about an unknown
  publisher: **More info → Run anyway**.
- **Linux** (x86-64; needs ALSA and X11 or Wayland): a `.deb` (Debian, Ubuntu, Mint), an
  `.rpm` (Fedora, openSUSE), an AppImage for any distribution (`chmod +x` it and run it),
  or a `.tar.gz` with the bare `freki` binary.

freki comes without games or boot ROMs. To build it yourself, see below.

## Getting started

freki needs nightly Rust. On Linux, install the X11/Wayland, ALSA and udev development
headers and libclang first (`libxkbcommon-dev libwayland-dev libasound2-dev libudev-dev
libclang-dev` on Debian/Ubuntu); libclang is only for webcams, and
`cargo build --no-default-features` leaves them out.

```sh
cargo run --release -- path/to/game.gb    # or start without a ROM: File → Open ROM…
```

## Controls

| | Keyboard | Gamepad |
|---|---|---|
| D-pad | Arrow keys | D-pad or left stick |
| A / B | X / Z | Right / bottom face button |
| Start / Select | Enter / Backspace | Start / Select |
| Turbo A / turbo B (15 presses a second while held) | S / A | Top / left face button |
| SNES L / R (Super Game Boy as hardware: both open its menu) | Q / W | Left / right shoulder |
| Save / load state | F5 / F8 | |
| Previous / next slot | F6 / F7 | |
| Rewind | hold R | |
| Fast-forward | hold Tab | |
| Pause | P | |
| Mute | M | |
| Cheats on / off | F9 | |
| Keyboard to the other linked Game Boy | F4 | |
| Screenshot | F12 | |
| Record video (start / stop) | Shift+F12 | |
| Fullscreen | Alt+Enter (Esc leaves) | |
| Open ROM / reset / quit | Ctrl+O / Ctrl+R / Ctrl+Q | |

These shortcuts also work while a tool window (debugger, viewers, cheats, printer, sound) has
focus, except rewind and fast-forward; the Game Boy buttons only work in the game's window.

The Game Boy buttons, the turbo buttons and L / R can be rebound under **Emulation →
Controls…**. Settings such as the palette, scale, controls, volume, recent ROMs, boot ROMs,
the Game Boy Camera's picture and where the tool windows were are remembered between runs.

## Two Game Boys, one window

<p align="center">
<img src="docs/images/mystery-gift.png" alt="Two linked Game Boys after a Mystery Gift in Pokémon Crystal">
</p>

**Emulation → Link cable → Connect a second game…** starts a second Game Boy to the right of
the first, connected by a link cable (or from the command line: `freki red.gb --link
blue.gb`). Both run in step, so trades and two-player games work as on two real Game Boys.
The keyboard and the first gamepad play the left Game Boy and the second gamepad the right
one; F4 moves the keyboard to the other side. You hear the Game Boy the keyboard plays.
The two also face each other with their infrared ports: the GBC's (Pokémon Gold, Silver
and Crystal's Mystery Gift, …) and those of Hudson's HuC1 and HuC3 cartridges; as on the
real console, each one also sees its own LED.
Each game keeps its battery save; a game linked with itself saves the second copy in
`game.2.sav`. Save states and rewind are off while linked. Headless mode can link too
(`--link`, `--link-sav`, `--press2`).

## Accuracy

<p align="center">
<img src="docs/images/dmg-acid2.png" width="240" alt="dmg-acid2 as freki draws it">
<img src="docs/images/cgb-acid2.png" width="240" alt="cgb-acid2 as freki draws it">
</p>

| Test suite | Result |
|---|---|
| Blargg's `cpu_instrs`, `instr_timing`, `mem_timing`, `mem_timing-2`, `dmg_sound`, `cgb_sound` | all pass |
| `dmg-acid2`, `cgb-acid2` | pixel-perfect |
| Mooneye acceptance and MBC tests (DMG) | 94 / 94 |
| Mooneye tests for all models or the CGB, with or without the boot ROM | 58 / 58 |
| mealybug-tearoom mid-scanline tests (DMG) | 22 / 24 (the other two are 3–4 pixels off) |
| mealybug-tearoom mid-scanline tests (CGB) | 20 / 27 |
| Gambatte hardware tests | DMG 1525 / 1651, GBC 2601 / 3023 |
| SameSuite (GBC and SGB; mostly sound, sample-exact) | 31 / 78 |
| AGE test ROMs (DMG, GBC and GBC in DMG mode) | 15 / 59 |

The failures are listed in `crates/freki-core/tests/gambatte_failures.txt` and
`same_age_failures.txt`. Some AGE tests exist once per hardware revision, so not all of them
can pass on one emulated console.

## Guide

<details>
<summary><b>Files next to the ROM</b></summary>

| File | What |
|---|---|
| `game.sav` | Battery save, written about once a second while the game saves, and on exit. A cartridge clock is stored at the end in the format VBA-M, BGB and mGBA use for MBC3 |
| `game.ss0` … `game.ss9` | Save states (about 155 KB each; they load only into the same game and freki state format). Each is a PNG of the screen at the time, with the state stored inside, so image viewers show the picture |
| `game.srm` | RetroArch's battery save, used (and saved to) when there is no `game.sav` |
| `game.2.sav` | Battery save of the second copy when a game is linked with itself |
| `game.cht` | Cheats, saved whenever one is added, removed or switched on or off |
| `game.bps`, `game.ips` | Patch applied when the game loads |
| `game.sym` | Labels for the debugger, from the assembler (read only) |
| `game-001.png`, … | Screenshots |
| `game-001.webm`, … | Videos (File → Record video) |

</details>

<details>
<summary><b>Consoles, boot ROMs and Super Game Boy sound</b></summary>

**Emulation → Console** chooses the hardware. **Auto** runs Game Boy Color games on a GBC,
games with Super Game Boy features on an SGB, and everything else on a Game Boy. **Game Boy
Color** also runs original Game Boy games on a GBC, in colour as on the real thing. **Super
Game Boy** also runs games made for both the GBC and the SGB (such as Dragon Warrior Monsters)
with their SGB border; the window grows to the SGB's 256×224 picture. Games without SGB
features get a plain border in the SGB's default colours. The SGB's own boot ROM is not
emulated.

**Super Game Boy sound.** Some SGB games play sound effects (applause, explosions, wind, …)
or their own music on the SNES's sound chip, through the `SOUND` and `SOU_TRN` commands.
For that, choose the Super Game Boy cartridge's SNES ROM under **Emulation → Boot ROM →
Super Game Boy sound ROM…** (a dump such as `Super Game Boy (World) (Rev 2).sfc`, 256 KiB;
Nintendo's code, not included). freki runs the sound engine and samples from it on an
emulated SNES sound chip (SPC700 and S-DSP) and mixes it into the Game Boy's sound; the
SNES CPU itself is not emulated, freki passes the commands on the way the SGB's SNES
program does. The two are mixed at bsnes's balance (the SNES sound twice as loud as
SameBoy's Game Boy scale); the Game Boy's sound gets a little quieter to make room. Headless
mode takes the ROM with `--sgb-rom`.

**Super Game Boy as hardware.** With that ROM chosen, **Emulation → Boot ROM → Run the SGB's
SNES program** emulates the SNES side too: its 65816 CPU, picture chip, DMA, sound chip and
the chip that links it with the Game Boy, running the SGB ROM's own program. Then games that
upload code to the SNES work: Space Invaders' **arcade mode** (choose it with Right and Start
on the mode screen) is a whole SNES game sent over from the cartridge. As on the real SGB,
the Game Boy then runs from the SNES's clock, about 2.4 % faster than a Game Boy. L and R
together open the SGB's own menu (palettes, borders, button setup, colour editor and
drawing), steered with a pointer that moves while the D-pad is held. Save states hold
the SNES side too (they need the same SGB ROM to load), and the SNES picture leaves out
mosaic and hi-res modes so far. Headless mode: `--sgb-rom FILE --sgb-hardware`.

**Emulation → Settings for this game** keeps the console, palette, colour correction and
frame blending chosen while it is ticked for the loaded game only, for example Dragon
Warrior Monsters always on the Super Game Boy. Other games keep your general settings.
Games are recognised by their header (title and checksum), so renaming the file keeps
them.

By default games start in the state the boot ROM leaves behind. For the logo animation and
sound, choose your own boot ROM dumps under **Emulation → Boot ROM → Choose file…**: the
256-byte Game Boy and the 2304-byte Game Boy Color boot ROM. They are Nintendo's code and
not included. The GBC boot ROM also picks the colours for original Game Boy games; without
it they are shown in grey.

</details>

<details>
<summary><b>Game Boy Printer</b></summary>

**Emulation → Link cable → Game Boy Printer** plugs a printer into the link cable port (in
place of a second game). When a game prints, the Printer window shows the paper coming out,
and each finished page is saved next to the ROM as `game-print-001.png`, `-002`, … A page is
finished when the game feeds the paper out after printing; pictures printed in several parts
end up on one page. **Tear off** finishes the page by hand, for games that never feed the
paper out. Headless mode prints with `--printer page.png`.

</details>

<details>
<summary><b>Game Boy Camera</b></summary>

The camera "sees" a picture you choose: **Emulation → Camera picture…** takes a PNG or JPEG,
which freki turns grey, crops to the sensor's shape and scales to its 128 × 120 pixels.
Without one it sees a test pattern (a gradient with a dark disc). The game processes the
picture like the real cartridge: exposure, the sensor's edge enhancement and the dithering
the brightness and contrast settings choose, and taking a picture takes as long as on the
real sensor. Photos are kept in the battery save like on the cartridge.

**Emulation → Camera: webcam** lists the webcams found; choose one and the camera sees it live
(the same grey crop of each frame). macOS asks for camera access the first time. The webcam
only runs while a camera cartridge is in. The picture or webcam is remembered between runs;
**Camera: test pattern** goes back to the pattern. Headless mode takes a picture with
`--camera picture.png`.

</details>

<details>
<summary><b>Cheats and memory search</b></summary>

**Cheats → Cheat manager…** adds, toggles, edits (✎) and removes codes, and **Cheats →
Memory search…** finds the address of a value. Both open as windows of their own, so they can sit
next to the game or on another screen at any size; long names are cut off and shown in
full on hover. Codes are kept in `game.cht` and saved as soon as
the list changes, one per line with an optional name (`#` starts a comment, `!` adds a code
switched off, `+` joins several codes into one cheat):

```text
010A10C1 Infinite lives
! 00A-17B-C49 Start with all items
01??2CA2 Lives
```

A code with `??` (or `X`) where the value goes is a **template**, a cheat whose value you
choose: enter it like any code, then type a value under **Templates** in the cheat manager
(or `cheat fill T VALUE`), and the cheat is added, such as "Lives = 99".

RetroArch's cheat files are read too, and saved back in their format. Game Genie codes made
for another version of the game (their compare byte is not in your ROM, so they do
nothing) are marked with ⚠ in the cheat manager and in `cheat list`. Codes with a value to
fill in (`01??B6D7` "MP Modifier", `0X0-64B-219`) are templates too: enter a value (0–255,
or 0–15 for a single `X`; decimal, or hex with `$`) and the cheat is added, such as "MP
Modifier = 99". Entries without a code (`N/A`, notes) are kept in the file. A file named as in
cheat databases, such as `metal-gear-solid-usa.cht` next to `Metal Gear Solid (USA).gbc`,
is imported when the game has no `game.cht` yet.

GameShark (`01VVLLHH`; on the GBC also `9xVVLLHH` for work RAM bank `x`) and Game Genie
(`ABC-DEF` or `ABC-DEF-GHI`) codes are supported, and so are Xploder codes (`$0D22A281`,
for Blaze's Xploder GB, as German cheat sites and RetroArch's "(Xploder)" files list them).
Those are encrypted RAM writes; freki decrypts them the way the device's firmware does. The
rare type `$Ex` is a condition: while the address holds the value, no cheat writes anything,
as on the device. The cheat manager and `cheat list` show each cheat's type; entries of the
file freki cannot use are listed greyed out with the reason, and stay in the file.

A Game Genie code changes a ROM byte in every bank where that byte has the code's compare
value. For a cheat whose codes change neighbouring bytes (an instruction and its operands),
freki only applies them in banks where all of those bytes match: a real Game Genie also
patches the banks where only some match, which can crash the game (Super Mario Bros.
Deluxe's "Jump In Midair" on Rev 2 does). If a game stops while cheats are on, freki
suggests switching them off and trying them one at a time.

Many games keep lives, coins or timers in **BCD**, one decimal digit per hex digit: 99 lives
are the byte `$99`, so a plain 100 (`$64`) shows as "64". Tick **BCD** in the memory search
to search for and hold the number the game shows; in templates, type such values in hex
(`$99`).

The same tools work from the terminal
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

</details>

<details>
<summary><b>Recording video</b></summary>

**File → Record video** (Shift+F12) records the screen as shown (Super Game Boy border,
linked games and frame blending included) with its sound, until you stop it; the video is
saved next to the ROM as `game-001.webm`. The picture is doubled in size without
smoothing, so pixels stay sharp, and encoded as AV1; the sound as Opus. Both encoders are
pure Rust and part of freki. Encoding runs on all CPU cores in the background, so the game
does not slow down. The files play in Chrome, Firefox, VLC and mpv; Safari plays AV1 only
on recent Apple hardware.

</details>

<details>
<summary><b>Headless mode</b></summary>

`freki --headless ROM [options]` runs a game as fast as possible without a window or
sound. It can press buttons at given frames, stop at breakpoints or watchpoints, and log
every instruction. At the end it can run console commands, print the serial output, and
save a screenshot, a save state or the battery save, and a Game Boy Printer can save what
the game prints. The Game Boy Camera can see a picture (`--camera`), and Super Game Boy
games get their SGB sound with `--sgb-rom` (and the SGB as hardware with `--sgb-hardware`). A battery save can be
loaded as if the Game Boy had been off for a while, so the cartridge clock catches up, and a
second game can run on a link cable. `freki --headless --help` lists the options.

```sh
freki --headless tetris.gb --frames 1200 --press 200:start --screenshot out.png --exec regs
freki --headless 01-special.gb --until-serial Passed --serial      # exit 0 once it passes
freki --headless game.gb --watch '$C0A0 w' --trace trace.log       # who writes $C0A0?
freki --headless rtc.gbc --write-sav a.sav && freki --headless rtc.gbc --sav a.sav --off 2h
freki --headless game.gbc --cheats --cheat 3 --cheat 010A10C1   # cheat 3 of game.cht, and a code
freki --headless tetris.gb --link tetris.gb --press 300:start --press2 300:start --screenshot both.png
freki --headless game.gbc --frames 400 --frame-blend --screenshot blended.png
freki --headless game.gbc --frames 1200 --record clip.webm          # 20 seconds of video
freki --headless game.gbc --sav game.sav --press 300:a --printer page.png  # print pages
```

</details>

## Development

| Path | What |
|---|---|
| `crates/freki-core` | Platform-independent emulation core (no OS, window or audio dependencies) |
| `crates/freki` | Desktop app: [egui](https://crates.io/crates/egui) GUI, [gilrs](https://crates.io/crates/gilrs) gamepads, [cpal](https://crates.io/crates/cpal) sound, [nokhwa](https://crates.io/crates/nokhwa) webcams, [rav1e](https://crates.io/crates/rav1e) and [rusty-opus](https://crates.io/crates/rusty-opus) for video |
| `scripts/fetch-test-roms.sh` | Downloads freely distributable test ROMs into `roms/test/` |
| `xtask` | `cargo xtask dist`, which builds the release packages (see [below](#release-packages)) |
| `assets` | The icon (SVG and its PNG, ICO and macOS renderings), the disk image background, the Linux `.desktop` file |
| `.github/workflows/release.yml` | Runs `cargo xtask dist` on Linux, Windows and macOS; pushing a tag `v*` makes a draft release with the packages |

```sh
./scripts/fetch-test-roms.sh      # Blargg, acid2, mealybug-tearoom, Mooneye, Gambatte, SameSuite, AGE, 65816
cargo test --workspace            # unit tests and all test ROMs, without a window
```

The test ROMs run headless:
- Blargg's tests report through the serial port or cartridge RAM.
- Mooneye's tests leave their result in the CPU registers.
- The acid2 tests compare the screen with a reference image. On a mismatch, freki's screen
  is saved to `target/<test>-actual.png`.

Tests whose ROMs have not been fetched are skipped. ROM files are never committed.

### Release packages

`cargo xtask dist` builds freki for the OS it runs on and packages it into `dist/`:

| OS | Packages | Needs |
|---|---|---|
| Linux | `.tar.gz`, `.deb`, `.rpm`, AppImage | `dpkg-deb`, `rpmbuild`, [appimagetool](https://github.com/AppImage/appimagetool) |
| Windows | `.zip` | |
| macOS | `.dmg` with a universal `freki.app` (`cargo xtask dist app`: just the app) | `rustup target add aarch64-apple-darwin x86_64-apple-darwin`; [uv](https://docs.astral.sh/uv/) for the disk image's styled window |

Formats can be named, e.g. `cargo xtask dist deb rpm`. On macOS, uv runs
`xtask/dmg.py` with [dmgbuild](https://github.com/dmgbuild/dmgbuild), whose versions are
pinned in the script's header; without uv the disk image is a plain one.

## References

- [Pan Docs](https://gbdev.io/pandocs/), the hardware reference, and the
  [opcode table](https://gbdev.io/gb-opcodes/optables/)
- Test ROMs: [Blargg's](https://github.com/retrio/gb-test-roms),
  [dmg-acid2](https://github.com/mattcurrie/dmg-acid2),
  [cgb-acid2](https://github.com/mattcurrie/cgb-acid2),
  [Mooneye test suite](https://github.com/Gekkio/mooneye-test-suite),
  [mealybug-tearoom-tests](https://github.com/mattcurrie/mealybug-tearoom-tests),
  [Gambatte hwtests](https://github.com/pokemon-speedrunning/gambatte-core/tree/master/test/hwtests),
  [SameSuite](https://github.com/LIJI32/SameSuite),
  [AGE test ROMs](https://github.com/c-sp/age-test-roms),
  [SingleStepTests 65816](https://github.com/SingleStepTests/65816) (the SNES CPU)
- Hardware behaviour not covered by the documentation was taken from
  [SameBoy](https://github.com/LIJI32/SameBoy) (MIT) and
  [Gambatte](https://github.com/pokemon-speedrunning/gambatte-core) (GPL-2.0) as
  references, some timing rules from the findings that
  [Cartouche](https://github.com/phmatray/cartouche) (MIT) publishes in its issues, and the
  unlicensed cartridges' from [mGBA](https://github.com/mgba-emu/mgba) (MPL-2.0). freki contains no code from Gambatte or mGBA; from SameBoy only one test
  compares freki's Game Genie decoding with a port of SameBoy's (with its MIT notice).
  Gambatte's test programs are only downloaded for local test runs and are not part of the
  repository.
- The SNES sound chip (SPC700 and S-DSP) follows nocash's
  [fullsnes](https://problemkaputt.de/fullsnes.htm) documentation; how the SGB passes on
  its sound commands was read from the SGB's own SNES program.
- The Game Boy Camera's picture processing follows the sample code in Pan Docs (CC0), from
  [Antonio Niño Díaz's research](https://github.com/AntonioND/gbcam-rev-engineer) on the
  camera.

## License

MIT
