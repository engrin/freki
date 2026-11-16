#!/usr/bin/env bash
# Downloads freely distributable Game Boy test ROMs into roms/test/ (git-ignored).
#   - Blargg's cpu_instrs (via retrio/gb-test-roms, pinned commit)
#   - dmg-acid2 by Matt Currie (MIT)
set -euo pipefail

DEST="$(cd "$(dirname "$0")/.." && pwd)/roms/test"
BLARGG="https://raw.githubusercontent.com/retrio/gb-test-roms/c240dd7d700e5c0b00a7bbba52b53e4ee67b5f15"
ACID2="https://github.com/mattcurrie/dmg-acid2/releases/download/v1.0/dmg-acid2.gb"

mkdir -p "$DEST/cpu_instrs"

fetch() { # url dest
  if [[ -f "$2" ]]; then echo "have  $2"; return; fi
  echo "fetch $2"
  curl -fsSL --retry 3 -o "$2" "$1"
}

fetch "$ACID2" "$DEST/dmg-acid2.gb"
fetch "$BLARGG/cpu_instrs/cpu_instrs.gb" "$DEST/cpu_instrs.gb"

for rom in "01-special" "02-interrupts" "03-op sp,hl" "04-op r,imm" "05-op rp" "06-ld r,r" \
           "07-jr,jp,call,ret,rst" "08-misc instrs" "09-op r,r" "10-bit ops" "11-op a,(hl)"; do
  url="$BLARGG/cpu_instrs/individual/$(printf '%s' "$rom" | sed 's/ /%20/g').gb"
  fetch "$url" "$DEST/cpu_instrs/$rom.gb"
done

echo "Test ROMs are in $DEST"
