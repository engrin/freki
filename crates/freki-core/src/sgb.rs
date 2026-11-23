//! Super Game Boy: colour palettes, attribute maps and borders, as the SNES side of the
//! SGB adds them to the Game Boy's picture.
//!
//! Games talk to the SGB through the joypad register: pulling P14 and P15 low together
//! starts a packet, then each pulse on P14 sends a 0 bit and on P15 a 1 bit, LSB first,
//! 16 bytes per packet, followed by a 0 stop bit. The first byte is command × 8 + number of
//! packets (1–7). Larger data (`*_TRN` commands) is read from the Game Boy's screen a few
//! frames after the command, 256 tiles of 2 bits per pixel. Sound commands and the SNES
//! program upload commands are ignored. See Pan Docs, "Super Game Boy".
//!
//! [`Sgb::frame`] is the 256×224 picture: the border (or, until a game sends one, its
//! colour 0) with the Game Boy screen at (48, 40), coloured by the four screen palettes
//! chosen per 8×8 cell.

use serde::{Deserialize, Serialize};

use crate::sgb_lle::SgbLle;
use crate::sgb_sound::SgbSound;

use crate::ppu::{SCREEN_HEIGHT, SCREEN_WIDTH};

/// Size of the SGB picture.
pub const SGB_WIDTH: usize = 256;
pub const SGB_HEIGHT: usize = 224;
/// Where the Game Boy screen sits in it.
const SCREEN_X: usize = 48;
const SCREEN_Y: usize = 40;

/// The SGB's built-in palette 1-A, used until a game sets its own.
const DEFAULT_PALETTE: [u16; 4] = [0x67BF, 0x265B, 0x10B5, 0x2866];

/// Frames after a `*_TRN` command until the screen is read.
const TRANSFER_DELAY: u8 = 3;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
enum Transfer {
    /// Border tiles $00–$7F or $80–$FF.
    Tiles { high: bool },
    /// Border map and palettes.
    Border,
    /// The 512 system palettes.
    Palettes,
    /// The 45 attribute files.
    Attributes,
    /// Code and data for the SNES sound chip (`SOU_TRN`).
    Sound,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
enum Mask {
    Off,
    Freeze,
    Black,
    Color0,
}

#[derive(Serialize, Deserialize)]
pub struct Sgb {
    /// The cartridge header unlocks SGB functions ($146 = $03, old licensee $33).
    enabled: bool,

    /// Packet being received: up to 7 × 16 bytes.
    packet: Vec<u8>,
    /// Bits received so far.
    bits: usize,
    /// A start pulse was seen and the lines went high again: bits may follow.
    receiving: bool,
    /// The 16 bytes of a packet are in; the next bit is the stop bit.
    awaiting_stop: bool,

    /// Players (MLT_REQ mode + 1: 1, 2, 4, or the unsupported 3) and the one whose joypad
    /// is read now (0-based).
    players: u8,
    player: u8,

    /// Screen palettes 0–3, 4 colours each (RGB555); colour 0 is shared.
    palettes: [u16; 16],
    /// Palette (0–3) of each 8×8 cell of the Game Boy screen, 20 × 18.
    attributes: Vec<u8>,
    system_palettes: Vec<u16>,
    attribute_files: Vec<u8>,
    /// Border tiles (256 × 32 bytes, 4 bits per pixel), map (32 × 32 entries) and
    /// palettes 4–7 (16 colours each).
    border_tiles: Vec<u8>,
    border_map: Vec<u16>,
    border_palettes: Vec<u16>,

    mask: Mask,
    /// Pending screen transfer and frames until it happens.
    transfer: Option<(Transfer, u8)>,

    /// The Game Boy screen as last shown (kept while frozen).
    #[serde(skip, default = "blank_screen")]
    shown: Vec<u8>,
    #[serde(skip, default = "blank_frame")]
    frame: Vec<u16>,
    /// The SNES sound chip with the SGB's sound engine, if the frontend gave freki the
    /// SGB's SNES ROM (not part of save states; kept when one is loaded).
    #[serde(skip)]
    sound: Option<Box<SgbSound>>,
    /// The SNES side emulated as hardware, running the SGB cartridge's SNES program
    /// (see [`crate::sgb_lle`]); replaces the built-in model above when present. Saved
    /// in states without its ROM, which a load takes from the running machine.
    #[serde(default)]
    pub lle: Option<Box<SgbLle>>,
}

fn blank_screen() -> Vec<u8> {
    vec![0; SCREEN_WIDTH * SCREEN_HEIGHT]
}

fn blank_frame() -> Vec<u16> {
    vec![0; SGB_WIDTH * SGB_HEIGHT]
}

impl Sgb {
    /// An SGB running the cartridge whose header is in `rom`.
    pub fn new(rom: &[u8]) -> Self {
        let enabled = rom.get(0x146) == Some(&0x03) && rom.get(0x14B) == Some(&0x33);
        let mut palettes = [0; 16];
        for p in 0..4 {
            palettes[p * 4..p * 4 + 4].copy_from_slice(&DEFAULT_PALETTE);
        }
        let mut sgb = Self {
            enabled,
            packet: vec![0; 16 * 7],
            bits: 0,
            receiving: false,
            awaiting_stop: false,
            players: 1,
            player: 0,
            palettes,
            attributes: vec![0; 20 * 18],
            system_palettes: vec![0; 512 * 4],
            attribute_files: vec![0; 0xFD2],
            border_tiles: vec![0; 256 * 32],
            border_map: vec![0; 32 * 32],
            border_palettes: vec![0; 16 * 4],
            mask: Mask::Off,
            transfer: None,
            shown: blank_screen(),
            frame: blank_frame(),
            sound: None,
            lle: None,
        };
        sgb.compose();
        sgb
    }

    /// Gives the SGB its sound (the SNES sound chip running the SGB ROM's engine).
    pub fn set_sound(&mut self, sound: Option<Box<SgbSound>>) {
        self.sound = sound;
    }

    pub fn has_sound(&self) -> bool {
        self.sound.is_some()
    }

    pub fn sound_mut(&mut self) -> Option<&mut SgbSound> {
        self.sound.as_deref_mut()
    }

    pub(crate) fn take_sound(&mut self) -> Option<Box<SgbSound>> {
        self.sound.take()
    }

    /// Runs the sound chip along with `cycles` Game Boy T-cycles.
    /// Output rate for the SGB's own sound (built-in model or SNES side as hardware).
    pub fn set_audio_rate(&mut self, rate: Option<u32>) {
        if let Some(sound) = self.sound.as_mut() {
            sound.set_rate(rate);
        }
        if let Some(lle) = self.lle.as_mut() {
            lle.set_audio_rate(rate);
        }
    }

    /// Mixes the SGB's own sound into the Game Boy's samples `out`.
    pub fn mix_audio(&mut self, out: &mut [f32]) {
        if let Some(lle) = self.lle.as_mut() {
            lle.mix_into(out);
        } else if let Some(sound) = self.sound.as_mut() {
            sound.mix_into(out);
        }
    }

    pub fn tick_sound(&mut self, cycles: u32) {
        // With the SNES side as hardware, its own sound chip plays instead.
        if self.lle.is_some() {
            return;
        }
        if let Some(sound) = self.sound.as_mut() {
            sound.tick(cycles);
        }
    }

    /// The 256×224 picture in RGB555, row by row.
    pub fn frame(&self) -> &[u16] {
        match &self.lle {
            Some(lle) => &lle.snes.bus.ppu.frame,
            None => &self.frame,
        }
    }

    /// The low nibble of P1 when the SNES side runs as hardware: the current player's
    /// buttons as the SNES program put them in the ICD2.
    pub fn lle_joypad(&self, select: u8) -> Option<u8> {
        self.lle
            .as_ref()
            .map(|lle| lle.joypad_nibble(self.player, select))
    }

    /// A write to P1 ($FF00): `old` and `new` are the P14/P15 select bits (bits 4–5).
    pub fn write_joypad(&mut self, old: u8, new: u8) {
        if let Some(lle) = &self.lle {
            self.players = lle.players();
            self.player %= self.players;
        }
        // The next joypad is selected when P15 goes high.
        if old & 0x20 == 0 && new & 0x20 != 0 && self.players % 2 == 0 {
            self.player = (self.player + 1) & (self.players - 1);
        }
        // On the real SGB packets reach the SNES whatever the header says.
        if !self.enabled && self.lle.is_none() {
            return;
        }
        let old = (old >> 4) & 3;
        match (new >> 4) & 3 {
            // Both low: a start pulse resets the receiver.
            0 => {
                self.receiving = false;
                if self.bits % 128 != 0 || self.bits == 0 || self.awaiting_stop {
                    self.reset_packet();
                }
            }
            // Both high again: commits the pulse that came before.
            3 => match old {
                0 => self.receiving = true,
                1 | 2 if self.receiving => self.receive_bit(old == 1),
                _ => {}
            },
            _ => {}
        }
    }

    fn reset_packet(&mut self) {
        self.packet.fill(0);
        self.bits = 0;
        self.awaiting_stop = false;
    }

    fn packets(&self) -> usize {
        usize::from(self.packet[0] & 7).max(1)
    }

    /// A pulse ended: P15 low sent a 1, P14 low a 0.
    fn receive_bit(&mut self, one: bool) {
        if self.awaiting_stop {
            // With the SNES side as hardware, each packet goes to the ICD2 by itself.
            if let Some(lle) = self.lle.as_mut() {
                lle.packet(&self.packet[..16]);
                self.reset_packet();
                self.receiving = false;
                return;
            }
            // The stop bit ends a packet; the command runs once all its packets are in.
            if self.bits == self.packets() * 128 {
                self.command();
                self.reset_packet();
            }
            self.awaiting_stop = false;
            self.receiving = false;
            return;
        }
        if self.bits < self.packet.len() * 8 {
            if one {
                self.packet[self.bits / 8] |= 1 << (self.bits % 8);
            }
            self.bits += 1;
            self.awaiting_stop = self.bits % 128 == 0;
        }
    }

    /// The low nibble P1 reads with neither button group selected: the current player's
    /// ID in multiplayer mode ($F, $E, $D, $C for players 1–4).
    pub fn joypad_id(&self) -> Option<u8> {
        (self.players > 1).then_some(0x0F - self.player)
    }

    /// Whether the joypad being read is player 1's (the only one connected).
    pub fn player_one(&self) -> bool {
        self.player == 0
    }

    fn command(&mut self) {
        let p = self.packet.clone();
        let word = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
        match p[0] >> 3 {
            0x00 => self.set_palettes(0, 1, &p),
            0x01 => self.set_palettes(2, 3, &p),
            0x02 => self.set_palettes(0, 3, &p),
            0x03 => self.set_palettes(1, 2, &p),
            0x04 => self.attr_blk(&p),
            0x05 => self.attr_lin(&p),
            0x06 => self.attr_div(&p),
            0x07 => self.attr_chr(&p),
            0x0A => {
                // PAL_SET: four system palettes; colour 0 comes from the first.
                for i in 0..4 {
                    let index = usize::from(word(1 + i * 2) & 0x1FF);
                    self.palettes[i * 4..i * 4 + 4]
                        .copy_from_slice(&self.system_palettes[index * 4..index * 4 + 4]);
                }
                let color0 = self.palettes[0];
                for i in 1..4 {
                    self.palettes[i * 4] = color0;
                }
                if p[9] & 0x80 != 0 {
                    self.load_attribute_file(p[9] & 0x3F);
                }
                if p[9] & 0x40 != 0 {
                    self.mask = Mask::Off;
                }
            }
            0x0B => self.transfer = Some((Transfer::Palettes, TRANSFER_DELAY)),
            0x11 => {
                // MLT_REQ: 1, 2 or 4 players. The unsupported mode 2 (3 players) counts
                // on once more (SameSuite, sgb/command_mlt_req; as SameBoy).
                self.players = (p[1] & 3) + 1;
                if self.players == 3 {
                    self.player += 1;
                }
                self.player &= self.players - 1;
            }
            0x13 => {
                let high = p[1] & 1 != 0;
                self.transfer = Some((Transfer::Tiles { high }, TRANSFER_DELAY));
            }
            0x14 => self.transfer = Some((Transfer::Border, TRANSFER_DELAY)),
            0x15 => self.transfer = Some((Transfer::Attributes, TRANSFER_DELAY)),
            0x16 => {
                self.load_attribute_file(p[1] & 0x3F);
                if p[1] & 0x40 != 0 {
                    self.mask = Mask::Off;
                }
            }
            0x17 => {
                self.mask = match p[1] & 3 {
                    0 => Mask::Off,
                    1 => Mask::Freeze,
                    2 => Mask::Black,
                    _ => Mask::Color0,
                }
            }
            // SOUND and SOU_TRN: the SNES sound chip, when the SGB ROM is there.
            0x08 => {
                if let Some(sound) = self.sound.as_mut() {
                    sound.command(&p);
                }
            }
            0x09 => self.transfer = Some((Transfer::Sound, TRANSFER_DELAY)),
            // SNES memory and program commands: not emulated.
            _ => {}
        }
    }

    /// PAL01, PAL23, PAL03, PAL12: colour 0 for all, then colours 1–3 of two palettes.
    fn set_palettes(&mut self, first: usize, second: usize, p: &[u8]) {
        let word = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
        let color0 = word(1);
        for i in 0..4 {
            self.palettes[i * 4] = color0;
        }
        for i in 0..3 {
            self.palettes[first * 4 + 1 + i] = word(3 + i * 2);
            self.palettes[second * 4 + 1 + i] = word(9 + i * 2);
        }
    }

    fn attr_blk(&mut self, p: &[u8]) {
        let count = usize::from(p[1]).min(0x12);
        for set in p[2..].as_chunks::<6>().0.iter().take(count) {
            let (inside, mut line, outside) = (set[0] & 1 != 0, set[0] & 2 != 0, set[0] & 4 != 0);
            let (inside_pal, mut line_pal, outside_pal) =
                (set[1] & 3, (set[1] >> 2) & 3, (set[1] >> 4) & 3);
            // Changing only the inside or only the outside also changes the border line.
            if inside && !line && !outside {
                line = true;
                line_pal = inside_pal;
            } else if outside && !line && !inside {
                line = true;
                line_pal = outside_pal;
            }
            let [left, top, right, bottom] = [set[2], set[3], set[4], set[5]].map(|c| c & 0x1F);
            for y in 0..18u8 {
                for x in 0..20u8 {
                    let cell = &mut self.attributes[usize::from(y) * 20 + usize::from(x)];
                    if x < left || x > right || y < top || y > bottom {
                        if outside {
                            *cell = outside_pal;
                        }
                    } else if x > left && x < right && y > top && y < bottom {
                        if inside {
                            *cell = inside_pal;
                        }
                    } else if line {
                        *cell = line_pal;
                    }
                }
            }
        }
    }

    fn attr_lin(&mut self, p: &[u8]) {
        let count = usize::from(p[1]).min(p.len() - 2);
        for &set in &p[2..2 + count] {
            let palette = (set >> 5) & 3;
            let line = usize::from(set & 0x1F);
            if set & 0x80 != 0 {
                if line < 18 {
                    self.attributes[line * 20..line * 20 + 20].fill(palette);
                }
            } else if line < 20 {
                for y in 0..18 {
                    self.attributes[y * 20 + line] = palette;
                }
            }
        }
    }

    fn attr_div(&mut self, p: &[u8]) {
        let (after, before, on) = (p[1] & 3, (p[1] >> 2) & 3, (p[1] >> 4) & 3);
        let horizontal = p[1] & 0x40 != 0;
        let line = usize::from(p[2] & 0x1F);
        for y in 0..18 {
            for x in 0..20 {
                let pos = if horizontal { y } else { x };
                self.attributes[y * 20 + x] = match pos.cmp(&line) {
                    std::cmp::Ordering::Less => before,
                    std::cmp::Ordering::Equal => on,
                    std::cmp::Ordering::Greater => after,
                };
            }
        }
    }

    fn attr_chr(&mut self, p: &[u8]) {
        let (mut x, mut y) = (usize::from(p[1]), usize::from(p[2]));
        let count = usize::from(u16::from_le_bytes([p[3], p[4]])).min((p.len() - 6) * 4);
        let vertical = p[5] != 0;
        if x >= 20 || y >= 18 {
            return;
        }
        for i in 0..count {
            self.attributes[y * 20 + x] = (p[6 + i / 4] >> (6 - (i % 4) * 2)) & 3;
            if vertical {
                y += 1;
                if y == 18 {
                    y = 0;
                    x += 1;
                    if x == 20 {
                        break;
                    }
                }
            } else {
                x += 1;
                if x == 20 {
                    x = 0;
                    y += 1;
                    if y == 18 {
                        break;
                    }
                }
            }
        }
    }

    fn load_attribute_file(&mut self, index: u8) {
        let index = usize::from(index);
        if index > 0x2C {
            return;
        }
        for (i, &byte) in self.attribute_files[index * 90..index * 90 + 90]
            .iter()
            .enumerate()
        {
            for j in 0..4 {
                self.attributes[i * 4 + j] = (byte >> (6 - j * 2)) & 3;
            }
        }
    }

    /// The PPU finished a frame; `screen` holds its shades (0–3).
    pub fn end_frame(&mut self, screen: &[u8]) {
        if let Some((what, frames)) = self.transfer {
            if frames <= 1 {
                self.transfer = None;
                self.receive_transfer(what, screen);
            } else {
                self.transfer = Some((what, frames - 1));
            }
        }
        if self.mask != Mask::Freeze {
            self.shown.copy_from_slice(screen);
        }
        self.compose();
    }

    /// Reads 4 KiB from the screen: tiles $00–$FF, 20 to a row, 2 bits per pixel as in
    /// Game Boy VRAM.
    fn receive_transfer(&mut self, what: Transfer, screen: &[u8]) {
        let mut data = vec![0u8; 0x1000];
        for tile in 0..256 {
            let (tx, ty) = ((tile % 20) * 8, (tile / 20) * 8);
            for row in 0..8 {
                let (mut low, mut high) = (0u8, 0u8);
                for x in 0..8 {
                    let shade = screen
                        .get((ty + row) * SCREEN_WIDTH + tx + x)
                        .copied()
                        .unwrap_or(0);
                    low |= (shade & 1) << (7 - x);
                    high |= (shade >> 1 & 1) << (7 - x);
                }
                data[tile * 16 + row * 2] = low;
                data[tile * 16 + row * 2 + 1] = high;
            }
        }
        let words = |range: std::ops::Range<usize>| -> Vec<u16> {
            data[range]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&w| u16::from_le_bytes(w))
                .collect()
        };
        match what {
            Transfer::Tiles { high } => {
                let start = if high { 0x1000 } else { 0 };
                self.border_tiles[start..start + 0x1000].copy_from_slice(&data);
            }
            Transfer::Border => {
                self.border_map.copy_from_slice(&words(0..0x800));
                self.border_palettes.copy_from_slice(&words(0x800..0x880));
            }
            Transfer::Palettes => self.system_palettes.copy_from_slice(&words(0..0x1000)),
            Transfer::Attributes => self.attribute_files.copy_from_slice(&data[..0xFD2]),
            Transfer::Sound => {
                if let Some(sound) = self.sound.as_mut() {
                    sound.transfer(&data);
                }
            }
        }
    }

    /// Draws [`Sgb::frame`].
    fn compose(&mut self) {
        let color0 = self.palettes[0];
        // Game Boy screen.
        for y in 0..SCREEN_HEIGHT {
            for x in 0..SCREEN_WIDTH {
                let color = match self.mask {
                    Mask::Black => 0,
                    Mask::Color0 => color0,
                    Mask::Off | Mask::Freeze => {
                        let palette = usize::from(self.attributes[(y / 8) * 20 + x / 8] & 3);
                        self.palettes
                            [palette * 4 + usize::from(self.shown[y * SCREEN_WIDTH + x] & 3)]
                    }
                };
                self.frame[(SCREEN_Y + y) * SGB_WIDTH + SCREEN_X + x] = color;
            }
        }
        // Border: 32 × 28 tiles; colour 0 shows the backdrop (screen colour 0), and inside
        // the Game Boy screen's area lets the screen through.
        for ty in 0..28 {
            for tx in 0..32 {
                let gb_area = (6..26).contains(&tx) && (5..23).contains(&ty);
                let entry = self.border_map[ty * 32 + tx];
                let tile = usize::from(entry & 0xFF);
                let palette = usize::from((entry >> 10) & 3);
                let (flip_x, flip_y) = (entry & 0x4000 != 0, entry & 0x8000 != 0);
                for y in 0..8 {
                    let row = if flip_y { 7 - y } else { y };
                    let base = tile * 32 + row * 2;
                    for x in 0..8 {
                        let bit = if flip_x { x } else { 7 - x };
                        let plane = |i: usize| (self.border_tiles[base + i] >> bit) & 1;
                        let index = plane(0) | plane(1) << 1 | plane(16) << 2 | plane(17) << 3;
                        let pixel = (ty * 8 + y) * SGB_WIDTH + tx * 8 + x;
                        if index != 0 {
                            self.frame[pixel] =
                                self.border_palettes[palette * 16 + usize::from(index)];
                        } else if !gb_area {
                            self.frame[pixel] = color0;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sgb_rom() -> Vec<u8> {
        let mut rom = vec![0; 0x150];
        rom[0x146] = 0x03;
        rom[0x14B] = 0x33;
        rom
    }

    /// Sends one packet the way games do: start pulse, 128 bits, stop bit.
    fn send(sgb: &mut Sgb, packet: &[u8; 16]) {
        let mut select = 0x30;
        let mut pulse = |sgb: &mut Sgb, low: u8| {
            sgb.write_joypad(select, low);
            sgb.write_joypad(low, 0x30);
            select = 0x30;
        };
        pulse(sgb, 0x00);
        for bit in 0..128 {
            let one = packet[bit / 8] >> (bit % 8) & 1 != 0;
            pulse(sgb, if one { 0x10 } else { 0x20 });
        }
        pulse(sgb, 0x20);
    }

    fn packet(bytes: &[u8]) -> [u8; 16] {
        let mut p = [0; 16];
        p[..bytes.len()].copy_from_slice(bytes);
        p
    }

    #[test]
    fn pal01_colours_the_screen() {
        let mut sgb = Sgb::new(&sgb_rom());
        // PAL01: colour 0 = $001F (red), palette 0 colours 1-3, palette 1 colours 1-3.
        let mut p = vec![0x01, 0x1F, 0x00];
        for c in [0x03E0u16, 0x7C00, 0x7FFF, 0x0001, 0x0002, 0x0003] {
            p.extend(c.to_le_bytes());
        }
        send(&mut sgb, &packet(&p));
        let mut screen = vec![0u8; SCREEN_WIDTH * SCREEN_HEIGHT];
        screen[0] = 2;
        sgb.end_frame(&screen);
        let at = |x: usize, y: usize| sgb.frame()[(SCREEN_Y + y) * SGB_WIDTH + SCREEN_X + x];
        assert_eq!(at(0, 0), 0x7C00, "palette 0 colour 2");
        assert_eq!(at(1, 0), 0x001F, "shared colour 0");
        // The border shows colour 0 until the game sends one.
        assert_eq!(sgb.frame()[0], 0x001F);
    }

    #[test]
    fn attr_blk_and_mask() {
        let mut sgb = Sgb::new(&sgb_rom());
        // ATTR_BLK: one set, inside only (line follows), palette 2, cells (1,1)–(3,3).
        send(&mut sgb, &packet(&[0x21, 0x01, 0x01, 0x02, 1, 1, 3, 3]));
        assert_eq!(sgb.attributes[20 + 1], 2, "line takes the inside palette");
        assert_eq!(sgb.attributes[2 * 20 + 2], 2);
        assert_eq!(sgb.attributes[0], 0);
        // MASK_EN black.
        send(&mut sgb, &packet(&[0xB9, 0x02]));
        sgb.end_frame(&[3; SCREEN_WIDTH * SCREEN_HEIGHT]);
        assert_eq!(sgb.frame()[SCREEN_Y * SGB_WIDTH + SCREEN_X], 0);
    }

    #[test]
    fn transfers_read_the_screen_after_three_frames() {
        let mut sgb = Sgb::new(&sgb_rom());
        send(&mut sgb, &packet(&[0x99])); // CHR_TRN, low tiles
        // Tile 0's first row: pixel 0 is shade 3, pixel 1 shade 1.
        let mut screen = vec![0u8; SCREEN_WIDTH * SCREEN_HEIGHT];
        screen[0] = 3;
        screen[1] = 1;
        sgb.end_frame(&screen);
        sgb.end_frame(&screen);
        assert_eq!(sgb.border_tiles[0], 0, "not yet");
        sgb.end_frame(&screen);
        assert_eq!(sgb.border_tiles[..2], [0xC0, 0x80]);
    }

    #[test]
    fn mlt_req_reports_the_joypad_id() {
        let mut sgb = Sgb::new(&sgb_rom());
        assert_eq!(sgb.joypad_id(), None);
        send(&mut sgb, &packet(&[0x89, 0x01])); // two players
        let first = sgb.joypad_id().unwrap();
        sgb.write_joypad(0x10, 0x30); // P15 goes high: next player
        assert_ne!(sgb.joypad_id().unwrap(), first);
        assert!([0x0F, 0x0E].contains(&first));
    }

    #[test]
    fn commands_need_the_sgb_header() {
        let mut sgb = Sgb::new(&[0; 0x150]);
        send(&mut sgb, &packet(&[0x89, 0x01]));
        assert_eq!(sgb.joypad_id(), None);
    }
}
