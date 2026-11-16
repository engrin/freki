#!/usr/bin/env bash
# Downloads freely distributable Game Boy test ROMs into roms/test/ (git-ignored).
#   - Blargg's cpu_instrs, dmg_sound, cgb_sound, instr_timing and mem_timing (via retrio/gb-test-roms, pinned commit)
#   - dmg-acid2 and cgb-acid2 by Matt Currie (MIT), plus their reference screenshots
#   - mealybug-tearoom-tests by Matt Currie (MIT): mid-scanline PPU tests + DMG references
#   - Mooneye Test Suite by Joonas Javanainen (MIT): acceptance and emulator-only (MBC) tests
set -euo pipefail

DEST="$(cd "$(dirname "$0")/.." && pwd)/roms/test"
BLARGG="https://raw.githubusercontent.com/retrio/gb-test-roms/c240dd7d700e5c0b00a7bbba52b53e4ee67b5f15"
ACID2="https://github.com/mattcurrie/dmg-acid2/releases/download/v1.0/dmg-acid2.gb"
ACID2_REF="https://raw.githubusercontent.com/mattcurrie/dmg-acid2/master/img/reference-dmg.png"
CGB_ACID2="https://github.com/mattcurrie/cgb-acid2/releases/download/v1.1/cgb-acid2.gbc"
CGB_ACID2_REF="https://raw.githubusercontent.com/mattcurrie/cgb-acid2/master/img/reference.png"
MTS="mts-20240926-1737-443f6e1"
MTS_URL="https://gekkio.fi/files/mooneye-test-suite/$MTS/$MTS.tar.xz"
MEALYBUG="https://raw.githubusercontent.com/mattcurrie/mealybug-tearoom-tests/70e88fb90b59d19dfbb9c3ac36c64105202bb1f4"

mkdir -p "$DEST/cpu_instrs" "$DEST/dmg_sound" "$DEST/mem_timing" "$DEST/mem_timing-2"

fetch() { # url dest
  if [[ -f "$2" ]]; then echo "have  $2"; return; fi
  echo "fetch $2"
  curl -fsSL --retry 3 -o "$2" "$1"
}

fetch "$ACID2" "$DEST/dmg-acid2.gb"
fetch "$ACID2_REF" "$DEST/dmg-acid2-reference.png"
fetch "$CGB_ACID2" "$DEST/cgb-acid2.gbc"
fetch "$CGB_ACID2_REF" "$DEST/cgb-acid2-reference.png"
fetch "$BLARGG/cpu_instrs/cpu_instrs.gb" "$DEST/cpu_instrs.gb"

for rom in "01-special" "02-interrupts" "03-op sp,hl" "04-op r,imm" "05-op rp" "06-ld r,r" \
           "07-jr,jp,call,ret,rst" "08-misc instrs" "09-op r,r" "10-bit ops" "11-op a,(hl)"; do
  url="$BLARGG/cpu_instrs/individual/$(printf '%s' "$rom" | sed 's/ /%20/g').gb"
  fetch "$url" "$DEST/cpu_instrs/$rom.gb"
done

fetch "$BLARGG/dmg_sound/dmg_sound.gb" "$DEST/dmg_sound.gb"
for rom in "01-registers" "02-len ctr" "03-trigger" "04-sweep" "05-sweep details" \
           "06-overflow on trigger" "07-len sweep period sync" "08-len ctr during power" \
           "09-wave read while on" "10-wave trigger while on" "11-regs after power" \
           "12-wave write while on"; do
  url="$BLARGG/dmg_sound/rom_singles/$(printf '%s' "$rom" | sed 's/ /%20/g').gb"
  fetch "$url" "$DEST/dmg_sound/$rom.gb"
done

mkdir -p "$DEST/cgb_sound"
fetch "$BLARGG/cgb_sound/cgb_sound.gb" "$DEST/cgb_sound.gb"
for rom in "01-registers" "02-len ctr" "03-trigger" "04-sweep" "05-sweep details" \
           "06-overflow on trigger" "07-len sweep period sync" "08-len ctr during power" \
           "09-wave read while on" "10-wave trigger while on" "11-regs after power" \
           "12-wave"; do
  url="$BLARGG/cgb_sound/rom_singles/$(printf '%s' "$rom" | sed 's/ /%20/g').gb"
  fetch "$url" "$DEST/cgb_sound/$rom.gb"
done

fetch "$BLARGG/instr_timing/instr_timing.gb" "$DEST/instr_timing.gb"
for rom in "01-read_timing" "02-write_timing" "03-modify_timing"; do
  fetch "$BLARGG/mem_timing/individual/$rom.gb" "$DEST/mem_timing/$rom.gb"
  fetch "$BLARGG/mem_timing-2/rom_singles/$rom.gb" "$DEST/mem_timing-2/$rom.gb"
done

# Mealybug: built ROMs ship as a zip in the repo; references are DMG screenshots.
mkdir -p "$DEST/mealybug"
if [[ ! -f "$DEST/mealybug/m3_scx_low_3_bits.gb" ]]; then
  fetch "$MEALYBUG/mealybug-tearoom-tests.zip" "$DEST/mealybug.zip"
  unzip -q -o -j "$DEST/mealybug.zip" '*.gb' -d "$DEST/mealybug"
  rm "$DEST/mealybug.zip"
fi
for rom in "$DEST"/mealybug/*.gb; do
  name="$(basename "$rom" .gb)"
  ref="$DEST/mealybug/$name.png"
  [[ -f "$ref" ]] && continue
  curl -fsS --retry 3 -o "$ref" "$MEALYBUG/expected/DMG-blob/$name.png" 2>/dev/null \
    && echo "fetch $ref" || echo "no DMG reference for $name"
done

# Mooneye: prebuilt release tarball; keep the acceptance and emulator-only ROMs.
if [[ ! -d "$DEST/mooneye/acceptance" ]]; then
  fetch "$MTS_URL" "$DEST/mooneye.tar.xz"
  mkdir -p "$DEST/mooneye"
  tar -xJf "$DEST/mooneye.tar.xz" -C "$DEST/mooneye" --strip-components=1 \
    "$MTS/acceptance" "$MTS/emulator-only" "$MTS/LICENSE"
  rm "$DEST/mooneye.tar.xz"
fi

echo "Test ROMs are in $DEST"
