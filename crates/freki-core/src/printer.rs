//! The Game Boy Printer, attached to the serial port in place of a second Game Boy.
//!
//! The game clocks every transfer and sends packets: the magic bytes $88 $33, a command,
//! a compression flag, a 16-bit data length (LSB first), the data, and a 16-bit checksum
//! (the sum of all bytes from the command on). The printer answers $00 to all of them, then
//! $81 (it is there) and its status byte to the two bytes that follow the checksum. The
//! command runs after that, so the status describes the printer before the command.
//!
//! Commands: $01 initialise (empties the buffer), $02 print (sheets, margins, palette,
//! exposure), $04 image data (tiles, 20 per row, in packets of up to 2 rows; an empty one
//! ends the data), $08 break (cancels) and $0F status.
//!
//! The printed image goes onto a strip of paper: a page is finished when a print feeds
//! paper after it (the low nibble of the margins), so pictures printed in several parts
//! without feeds in between (Pokémon's Pokédex pages) end up on one page.

use crate::ppu::SCREEN_WIDTH;

/// Width of the paper in pixels.
pub const PAPER_WIDTH: usize = SCREEN_WIDTH;

/// Most image data the printer holds: 9 packets of two tile rows (160×144 pixels and a
/// little more, as the real printer's 8 KiB).
const BUFFER_LIMIT: usize = 0x2000;

/// The printer forgets a half-received packet after this long without a byte (100 ms).
const TIMEOUT_CYCLES: u32 = 4_194_304 / 10;

/// How long printing takes per pixel row (the real printer needs a few seconds per picture;
/// this is about a quarter second for a whole screen).
const CYCLES_PER_ROW: u32 = 7_000;

/// Pixel rows of paper fed per unit of the margins.
const FEED_ROWS: usize = 8;

pub mod status {
    pub const CHECKSUM_ERROR: u8 = 0x01;
    pub const PRINTING: u8 = 0x02;
    pub const DATA_FULL: u8 = 0x04;
    pub const UNPROCESSED: u8 = 0x08;
}

/// Where the next byte of a packet goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Magic1,
    Magic2,
    Command,
    Compression,
    LengthLow,
    LengthHigh,
    Data,
    ChecksumLow,
    ChecksumHigh,
    Alive,
    Status,
}

/// A finished page: shades 0 (white) to 3 (black), [`PAPER_WIDTH`] per row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub shades: Vec<u8>,
}

impl Page {
    pub fn height(&self) -> usize {
        self.shades.len() / PAPER_WIDTH
    }
}

#[derive(Debug, Clone)]
pub struct Printer {
    field: Field,
    command: u8,
    compressed: bool,
    length: u16,
    data: Vec<u8>,
    checksum: u16,
    received_checksum: u16,
    /// The byte shifted out during the next transfer.
    out: u8,
    status: u8,
    /// Decompressed image data (2bpp tiles).
    buffer: Vec<u8>,
    /// Cycles of printing left (while the PRINTING status bit is set).
    busy: u32,
    /// Cycles since the last byte.
    idle: u32,
    /// The page being printed on.
    paper: Vec<u8>,
    /// Pages fed out of the printer, not yet taken.
    finished: Vec<Page>,
    /// The paper changed since [`Printer::take_changed`].
    changed: bool,
}

impl Default for Printer {
    fn default() -> Self {
        Self::new()
    }
}

impl Printer {
    pub fn new() -> Self {
        Self {
            field: Field::Magic1,
            command: 0,
            compressed: false,
            length: 0,
            data: Vec::new(),
            checksum: 0,
            received_checksum: 0,
            out: 0,
            status: 0,
            buffer: Vec::new(),
            busy: 0,
            idle: 0,
            paper: Vec::new(),
            finished: Vec::new(),
            changed: false,
        }
    }

    /// The game clocked `byte` out; returns the byte the printer sent back at the same time.
    pub fn exchange(&mut self, byte: u8) -> u8 {
        self.idle = 0;
        let back = std::mem::take(&mut self.out);
        self.field = match self.field {
            Field::Magic1 if byte == 0x88 => Field::Magic2,
            Field::Magic1 => Field::Magic1,
            Field::Magic2 if byte == 0x33 => Field::Command,
            Field::Magic2 if byte == 0x88 => Field::Magic2,
            Field::Magic2 => Field::Magic1,
            Field::Command => {
                self.command = byte;
                self.checksum = u16::from(byte);
                Field::Compression
            }
            Field::Compression => {
                self.compressed = byte & 1 != 0;
                self.add(byte);
                Field::LengthLow
            }
            Field::LengthLow => {
                self.length = u16::from(byte);
                self.add(byte);
                Field::LengthHigh
            }
            Field::LengthHigh => {
                self.length |= u16::from(byte) << 8;
                self.add(byte);
                self.data.clear();
                if self.length == 0 {
                    Field::ChecksumLow
                } else {
                    Field::Data
                }
            }
            Field::Data => {
                self.data.push(byte);
                self.add(byte);
                if self.data.len() >= usize::from(self.length) {
                    Field::ChecksumLow
                } else {
                    Field::Data
                }
            }
            Field::ChecksumLow => {
                self.received_checksum = u16::from(byte);
                Field::ChecksumHigh
            }
            Field::ChecksumHigh => {
                self.received_checksum |= u16::from(byte) << 8;
                self.out = 0x81;
                Field::Alive
            }
            Field::Alive => {
                self.out = self.status;
                Field::Status
            }
            Field::Status => {
                self.run_command();
                Field::Magic1
            }
        };
        back
    }

    /// Lets `cycles` T-cycles pass (printing, the timeout between bytes).
    pub fn tick(&mut self, cycles: u32) {
        if self.busy > 0 {
            self.busy = self.busy.saturating_sub(cycles);
            if self.busy == 0 {
                self.status &= !status::PRINTING;
            }
        }
        if self.field != Field::Magic1 {
            self.idle = self.idle.saturating_add(cycles);
            if self.idle >= TIMEOUT_CYCLES {
                self.field = Field::Magic1;
                self.out = 0;
            }
        }
    }

    fn add(&mut self, byte: u8) {
        self.checksum = self.checksum.wrapping_add(u16::from(byte));
    }

    fn run_command(&mut self) {
        if self.checksum != self.received_checksum {
            self.status |= status::CHECKSUM_ERROR;
            return;
        }
        self.status &= !status::CHECKSUM_ERROR;
        match self.command {
            0x01 => {
                self.buffer.clear();
                self.status = 0;
                self.busy = 0;
            }
            0x02 if self.data.len() >= 4 => {
                let [sheets, margins, palette, _exposure] =
                    [self.data[0], self.data[1], self.data[2], self.data[3]];
                let rows = self.print(sheets, margins, palette);
                self.buffer.clear();
                self.status = status::PRINTING | status::DATA_FULL;
                self.busy = (rows as u32).max(1) * CYCLES_PER_ROW;
            }
            0x04 => {
                if self.data.is_empty() {
                    return;
                }
                let data = std::mem::take(&mut self.data);
                if self.compressed {
                    decompress(&data, &mut self.buffer);
                } else {
                    self.buffer.extend_from_slice(&data);
                }
                self.buffer.truncate(BUFFER_LIMIT);
                self.status = (self.status & !status::DATA_FULL) | status::UNPROCESSED;
            }
            0x08 => {
                self.buffer.clear();
                self.status = 0;
                self.busy = 0;
            }
            _ => {}
        }
    }

    /// Prints the buffer onto the paper; returns how many pixel rows that took.
    fn print(&mut self, sheets: u8, margins: u8, palette: u8) -> usize {
        let before = usize::from(margins >> 4) * FEED_ROWS;
        let after = usize::from(margins & 0x0F) * FEED_ROWS;
        let image = self.image(palette);
        if !self.paper.is_empty() {
            self.feed(before);
        }
        for _ in 0..sheets {
            self.paper.extend_from_slice(&image);
        }
        if after > 0 && !self.paper.is_empty() {
            self.feed(after);
            self.tear_off();
        }
        self.changed = true;
        before + image.len() / PAPER_WIDTH * usize::from(sheets) + after
    }

    /// The buffer's tiles as shades, rows of 20 tiles.
    fn image(&self, palette: u8) -> Vec<u8> {
        let tile_rows = self.buffer.len() / (16 * 20);
        let mut image = vec![0; tile_rows * 8 * PAPER_WIDTH];
        let tiles = self.buffer.as_chunks::<16>().0;
        for (tile, bytes) in tiles.iter().take(tile_rows * 20).enumerate() {
            let (tx, ty) = (tile % 20, tile / 20);
            for row in 0..8 {
                let (low, high) = (bytes[row * 2], bytes[row * 2 + 1]);
                for col in 0..8 {
                    let bit = 7 - col;
                    let colour = (low >> bit & 1) | (high >> bit & 1) << 1;
                    let shade = palette >> (colour * 2) & 3;
                    image[(ty * 8 + row) * PAPER_WIDTH + tx * 8 + col] = shade;
                }
            }
        }
        image
    }

    fn feed(&mut self, rows: usize) {
        self.paper.resize(self.paper.len() + rows * PAPER_WIDTH, 0);
    }

    /// Finishes the page being printed on (if anything is on it).
    pub fn tear_off(&mut self) {
        let shades = std::mem::take(&mut self.paper);
        if shades.iter().any(|&s| s != 0) {
            self.finished.push(Page { shades });
        }
        self.changed = true;
    }

    /// The page being printed on: shades, [`PAPER_WIDTH`] per row.
    pub fn paper(&self) -> &[u8] {
        &self.paper
    }

    /// Pages fed out since the last call.
    pub fn take_pages(&mut self) -> Vec<Page> {
        std::mem::take(&mut self.finished)
    }

    /// Whether the paper changed since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    pub fn status(&self) -> u8 {
        self.status
    }
}

/// The printer's run-length encoding: a control byte below $80 is followed by that many
/// plus one bytes as they are; one from $80 on by one byte repeated (control & $7F) + 2
/// times.
fn decompress(data: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    while i < data.len() {
        let control = data[i];
        i += 1;
        if control & 0x80 != 0 {
            let Some(&byte) = data.get(i) else { break };
            i += 1;
            out.extend(std::iter::repeat_n(byte, usize::from(control & 0x7F) + 2));
        } else {
            let n = (usize::from(control) + 1).min(data.len() - i);
            out.extend_from_slice(&data[i..i + n]);
            i += n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sends a packet; returns the printer's two answer bytes ($81 and the status).
    fn send(printer: &mut Printer, command: u8, compressed: bool, data: &[u8]) -> [u8; 2] {
        let len = data.len() as u16;
        let mut body = vec![command, u8::from(compressed), len as u8, (len >> 8) as u8];
        body.extend_from_slice(data);
        let sum: u16 = body.iter().fold(0u16, |s, &b| s.wrapping_add(u16::from(b)));
        let mut packet = vec![0x88, 0x33];
        packet.extend(body);
        packet.extend([sum as u8, (sum >> 8) as u8]);
        for byte in packet {
            assert_eq!(printer.exchange(byte), 0);
        }
        [printer.exchange(0), printer.exchange(0)]
    }

    /// Two tile rows: colour 3 on the first, colour 0 on the second except its first tile,
    /// of colour 1.
    fn band() -> Vec<u8> {
        let mut data = vec![0xFF; 20 * 16];
        data.extend(vec![0; 20 * 16]);
        for row in 0..8 {
            data[20 * 16 + row * 2] = 0xFF;
        }
        data
    }

    #[test]
    fn answers_status_and_detects() {
        let mut printer = Printer::new();
        assert_eq!(send(&mut printer, 0x0F, false, &[]), [0x81, 0x00]);
    }

    #[test]
    fn prints_a_page_with_the_palette() {
        let mut printer = Printer::new();
        assert_eq!(send(&mut printer, 0x01, false, &[]), [0x81, 0x00]);
        assert_eq!(send(&mut printer, 0x04, false, &band()), [0x81, 0x00]);
        assert_eq!(send(&mut printer, 0x0F, false, &[]), [0x81, 0x08]);
        assert_eq!(send(&mut printer, 0x04, false, &[]), [0x81, 0x08]);
        // One sheet, no feed before, 3 after; palette $E4 (colour n → shade n).
        assert_eq!(
            send(&mut printer, 0x02, false, &[1, 0x03, 0xE4, 0x40]),
            [0x81, 0x08]
        );
        assert_eq!(send(&mut printer, 0x0F, false, &[]), [0x81, 0x06]);
        printer.tick(16 * CYCLES_PER_ROW + 24 * CYCLES_PER_ROW);
        assert_eq!(send(&mut printer, 0x0F, false, &[]), [0x81, 0x04]);
        let pages = printer.take_pages();
        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        assert_eq!(page.height(), 16 + 3 * FEED_ROWS);
        assert_eq!(page.shades[0], 3);
        assert_eq!(page.shades[8 * PAPER_WIDTH], 1);
        assert_eq!(page.shades[8 * PAPER_WIDTH + 8], 0);
        assert!(printer.paper().is_empty());
    }

    #[test]
    fn parts_without_feeds_make_one_page() {
        let mut printer = Printer::new();
        for margins in [0x10, 0x00, 0x03] {
            send(&mut printer, 0x01, false, &[]);
            send(&mut printer, 0x04, false, &band());
            send(&mut printer, 0x04, false, &[]);
            send(&mut printer, 0x02, false, &[1, margins, 0xE4, 0x40]);
            if margins != 0x03 {
                assert!(printer.take_pages().is_empty());
            }
        }
        let pages = printer.take_pages();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].height(), 3 * 16 + 3 * FEED_ROWS);
    }

    #[test]
    fn decompresses_runs_and_literals() {
        let mut printer = Printer::new();
        send(&mut printer, 0x01, false, &[]);
        // 640 bytes: a literal $FF, runs of 319 more $FF, then 320 zeros as runs.
        let mut data = vec![0x00, 0xFF];
        let mut left = 319;
        while left > 0 {
            let n = left.min(129);
            data.extend([0x80 | (n - 2) as u8, 0xFF]);
            left -= n;
        }
        let mut left = 320;
        while left > 0 {
            let n = left.min(129);
            data.extend([0x80 | (n - 2) as u8, 0x00]);
            left -= n;
        }
        send(&mut printer, 0x04, true, &data);
        assert_eq!(printer.buffer.len(), 640);
        assert!(printer.buffer[..320].iter().all(|&b| b == 0xFF));
        assert!(printer.buffer[320..].iter().all(|&b| b == 0));
    }

    #[test]
    fn bad_checksum_is_reported_and_ignored() {
        let mut printer = Printer::new();
        for byte in [0x88, 0x33, 0x04, 0, 1, 0, 0xAA, 0x00, 0x00] {
            printer.exchange(byte);
        }
        printer.exchange(0);
        printer.exchange(0);
        assert!(printer.buffer.is_empty());
        assert_eq!(send(&mut printer, 0x0F, false, &[]), [0x81, 0x01]);
    }

    #[test]
    fn forgets_a_broken_packet() {
        let mut printer = Printer::new();
        printer.exchange(0x88);
        printer.exchange(0x33);
        printer.exchange(0x04);
        printer.tick(TIMEOUT_CYCLES);
        assert_eq!(send(&mut printer, 0x0F, false, &[]), [0x81, 0x00]);
    }
}
