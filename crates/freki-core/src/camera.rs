//! The Game Boy Camera (Pocket Camera, header type $FC): an MBC3-like controller with
//! 128 KiB RAM, and the M64282FP sensor behind registers at $A000–$A035 (RAM bank $10 and
//! up). The picture goes through exposure, the sensor's edge modes and the controller's
//! 4×4 dithering matrix, and is written as 14×16 tiles to RAM at $A100.
//!
//! Source: Pan Docs' "Game Boy Camera" chapter (CC0), compiled by Antonio Niño Díaz from
//! his reverse engineering (<https://github.com/AntonioND/gbcam-rev-engineer>); the
//! processing in `process` follows that chapter's sample code for emulators.
//!
//! The picture itself comes from the frontend ([`Camera::set_image`]): greyscale, sensor
//! sized ([`SENSOR_WIDTH`] × [`SENSOR_HEIGHT`]); without one the camera sees a test
//! pattern.

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

/// The sensor's picture: 128 × 120 (the controller keeps the middle 112 rows).
pub const SENSOR_WIDTH: usize = 128;
pub const SENSOR_HEIGHT: usize = 120;
/// The picture the game gets: 128 × 112, as 16 × 14 tiles.
const WIDTH: usize = 128;
const HEIGHT: usize = 112;
/// Where the picture goes in RAM bank 0, and its size in bytes (2 bytes per tile row).
const PICTURE_OFFSET: usize = 0x100;
const PICTURE_BYTES: usize = WIDTH * HEIGHT / 4;
/// Registers: A000 (trigger), A001–A005 (sensor), A006–A035 (dithering matrix).
const REGISTERS: usize = 0x36;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Camera {
    pub(crate) ram_enabled: bool,
    /// $00–$3F; bank 0 can be mapped at $4000 too.
    pub(crate) rom_bank: u8,
    /// $00–$0F selects a RAM bank; bit 4 set selects the registers.
    pub(crate) ram_bank: u8,
    #[serde(with = "BigArray")]
    regs: [u8; REGISTERS],
    /// T-cycles until the capture in progress is done (0: idle).
    busy: u32,
    /// The picture being taken, written to RAM when the capture ends.
    pending: Vec<u8>,
    /// What the sensor sees (not part of save states).
    #[serde(skip)]
    image: Option<Box<[u8]>>,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            ram_enabled: false,
            rom_bank: 1,
            ram_bank: 0,
            regs: [0; REGISTERS],
            busy: 0,
            pending: Vec::new(),
            image: None,
        }
    }
}

impl Camera {
    /// What the sensor sees from now on: `SENSOR_WIDTH × SENSOR_HEIGHT` grey levels
    /// (0 black, 255 white), row by row; `None` for the test pattern. Other sizes are
    /// ignored.
    pub fn set_image(&mut self, image: Option<&[u8]>) {
        match image {
            Some(image) if image.len() == SENSOR_WIDTH * SENSOR_HEIGHT => {
                self.image = Some(image.into());
            }
            Some(_) => {}
            None => self.image = None,
        }
    }

    pub fn has_image(&self) -> bool {
        self.image.is_some()
    }

    /// Keeps the frontend's picture when a save state replaces the cartridge.
    pub(crate) fn adopt_image_from(&mut self, old: &mut Camera) {
        self.image = old.image.take();
    }

    pub(crate) fn registers_selected(&self) -> bool {
        self.ram_bank & 0x10 != 0
    }

    pub(crate) fn busy(&self) -> bool {
        self.busy > 0
    }

    /// A read of $A000–$BFFF while the registers are selected: only A000 answers.
    pub(crate) fn read_register(&self, addr: u16) -> u8 {
        if addr & 0x7F == 0 {
            (self.regs[0] & 0x06) | u8::from(self.busy())
        } else {
            0x00
        }
    }

    /// A write to the registers (mirrored every $80 bytes). Writing bit 0 of A000 starts a
    /// capture; clearing it stops one.
    pub(crate) fn write_register(&mut self, addr: u16, value: u8) {
        let reg = usize::from(addr & 0x7F);
        if reg == 0 {
            self.regs[0] = value & 0x07;
            if value & 1 == 0 {
                self.busy = 0;
                self.pending.clear();
            } else if !self.busy() {
                self.capture();
            }
        } else if reg < REGISTERS {
            self.regs[reg] = value;
        }
    }

    /// Runs the capture for `cycles` T-cycles; when it ends, the picture lands in `ram`.
    pub(crate) fn tick(&mut self, cycles: u32, ram: &mut [u8]) -> bool {
        if self.busy == 0 {
            return false;
        }
        self.busy = self.busy.saturating_sub(cycles);
        if self.busy > 0 {
            return false;
        }
        if let Some(dest) = ram.get_mut(PICTURE_OFFSET..PICTURE_OFFSET + PICTURE_BYTES) {
            dest.copy_from_slice(&self.pending);
        }
        self.pending.clear();
        true
    }

    /// Takes the picture with the current registers and starts the capture timer.
    fn capture(&mut self) {
        let regs = &self.regs;
        let n_bit = regs[1] & 0x80 != 0;
        let exposure = u32::from(u16::from_be_bytes([regs[2], regs[3]]));
        // Pan Docs: 32446 + N + 16 × exposure M-cycles.
        self.busy = 4 * (32_446 + if n_bit { 0 } else { 512 } + 16 * exposure);
        let image = match &self.image {
            Some(image) => image.to_vec(),
            None => test_pattern(),
        };
        self.pending = tiles(&process(&image, regs));
    }
}

/// The sensor and controller processing, after GiiBiiAdvance (Pan Docs' sample code).
/// Returns the 2-bit colour (0 white … 3 black) of each of the 128 × 112 pixels.
fn process(image: &[u8], regs: &[u8; REGISTERS]) -> Vec<u8> {
    let (w, h) = (SENSOR_WIDTH, SENSOR_HEIGHT);
    let at = |x: usize, y: usize| y * w + x;
    // Register 0 (A000 bits 1–2): which of the pixel (P) and the one below (M) the 1-D
    // filter adds and subtracts.
    let (p_bits, m_bits) = match (regs[0] >> 1) & 3 {
        0 => (0b01, 0b01),
        1 => (0b01, 0b00),
        _ => (0b01, 0b10),
    };
    let n_bit = u32::from(regs[1] >> 7);
    let vh = u32::from((regs[1] >> 5) & 3);
    let exposure = i32::from(u16::from_be_bytes([regs[2], regs[3]]));
    const EDGE_RATIO: [f32; 8] = [0.50, 0.75, 1.00, 1.25, 2.00, 3.00, 4.00, 5.00];
    let alpha = EDGE_RATIO[usize::from((regs[4] >> 4) & 7)];
    let e3 = u32::from(regs[4] >> 7);
    let invert = regs[4] & 0x08 != 0;

    // Exposure and the sensor's output range, then signed around 128.
    let mut px: Vec<i32> = image
        .iter()
        .map(|&v| {
            let v = i32::from(v) * exposure / 0x0300;
            let v = (128 + (v - 128) / 8).clamp(0, 255);
            let v = if invert { 255 - v } else { v };
            v - 128
        })
        .collect();

    let one_d = |src: &[i32]| -> Vec<i32> {
        let mut out = vec![0; w * h];
        for y in 0..h {
            for x in 0..w {
                let p = src[at(x, y)];
                let m = src[at(x, (y + 1).min(h - 1))];
                let mut v = 0;
                if p_bits & 1 != 0 {
                    v += p;
                }
                if p_bits & 2 != 0 {
                    v += m;
                }
                if m_bits & 1 != 0 {
                    v -= p;
                }
                if m_bits & 2 != 0 {
                    v -= m;
                }
                out[at(x, y)] = v.clamp(-128, 127);
            }
        }
        out
    };
    let edge = |src: &[i32], x: usize, y: usize, two_d: bool| -> i32 {
        let p = src[at(x, y)];
        let west = src[at(x.saturating_sub(1), y)];
        let east = src[at((x + 1).min(w - 1), y)];
        let around = if two_d {
            let north = src[at(x, y.saturating_sub(1))];
            let south = src[at(x, (y + 1).min(h - 1))];
            4 * p - west - east - north - south
        } else {
            2 * p - west - east
        };
        (p + (around as f32 * alpha) as i32).clamp(-128, 127)
    };
    match (n_bit << 3) | (vh << 1) | e3 {
        0x0 => px = one_d(&px),
        0x2 => {
            let enhanced: Vec<i32> = (0..w * h).map(|i| edge(&px, i % w, i / w, false)).collect();
            px = one_d(&enhanced);
        }
        0xE => px = (0..w * h).map(|i| edge(&px, i % w, i / w, true)).collect(),
        // Undocumented; on a real cartridge it gives one flat colour.
        0x1 => px.fill(0),
        // Other modes: no filtering.
        _ => {}
    }

    // The controller's dithering matrix on the middle 112 rows.
    let skip = (SENSOR_HEIGHT - HEIGHT) / 2;
    let mut colours = Vec::with_capacity(WIDTH * HEIGHT);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let value = px[at(x, y + skip)] + 128;
            let base = 6 + ((y & 3) * 4 + (x & 3)) * 3;
            let level = regs[base..base + 3]
                .iter()
                .take_while(|&&t| value >= i32::from(t))
                .count() as u8;
            colours.push(3 - level);
        }
    }
    colours
}

/// 2-bit colours (128 × 112) as Game Boy tiles, row of tiles by row of tiles.
fn tiles(colours: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; PICTURE_BYTES];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let colour = colours[y * WIDTH + x];
            let tile = (y / 8) * (WIDTH / 8) + x / 8;
            let row = tile * 16 + (y % 8) * 2;
            let bit = 0x80 >> (x % 8);
            if colour & 1 != 0 {
                out[row] |= bit;
            }
            if colour & 2 != 0 {
                out[row + 1] |= bit;
            }
        }
    }
    out
}

/// What the camera sees without a picture: a light-to-dark gradient with a dark disc and
/// a light frame, so every grey level and edge mode shows something.
fn test_pattern() -> Vec<u8> {
    let mut image = vec![0u8; SENSOR_WIDTH * SENSOR_HEIGHT];
    for y in 0..SENSOR_HEIGHT {
        for x in 0..SENSOR_WIDTH {
            let (dx, dy) = (x as i32 - 64, y as i32 - 60);
            let inside =
                (6..SENSOR_WIDTH - 6).contains(&x) && (10..SENSOR_HEIGHT - 10).contains(&y);
            let value = if !inside {
                240
            } else if dx * dx + dy * dy < 30 * 30 {
                30
            } else {
                (230 - x * 200 / SENSOR_WIDTH) as u8
            };
            image[y * SENSOR_WIDTH + x] = value;
        }
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A matrix that splits 0–255 evenly (no dithering), and exposure $0300 (neutral).
    fn plain_registers(camera: &mut Camera) {
        camera.write_register(0xA002, 0x03);
        camera.write_register(0xA003, 0x00);
        for cell in 0..16u16 {
            for (i, threshold) in [0x7Au16, 0x80, 0x86].iter().enumerate() {
                camera.write_register(0xA006 + cell * 3 + i as u16, *threshold as u8);
            }
        }
    }

    #[test]
    fn a_capture_takes_its_time_and_lands_in_ram() {
        let mut camera = Camera::default();
        plain_registers(&mut camera);
        // Left half black, right half white.
        let image: Vec<u8> = (0..SENSOR_WIDTH * SENSOR_HEIGHT)
            .map(|i| if i % SENSOR_WIDTH < 64 { 0 } else { 255 })
            .collect();
        camera.set_image(Some(&image));
        // Bits 1–2 = 1: the 1-D filter passes the pixel through, as the Camera ROM sets it.
        camera.write_register(0xA000, 0x03);
        assert_eq!(camera.read_register(0xA000) & 1, 1, "busy");
        let mut ram = vec![0u8; 0x20000];
        assert!(!camera.tick(1000, &mut ram));
        assert!(ram[0x100..0x100 + PICTURE_BYTES].iter().all(|&b| b == 0));
        // 4 × (32446 + 512 + 16 × $0300) T-cycles in all.
        assert!(camera.tick(4 * (32_446 + 512 + 16 * 0x300), &mut ram));
        assert_eq!(camera.read_register(0xA000) & 1, 0, "done");
        // Top-left tile: black (both bit planes set); a tile on the right: white.
        assert_eq!(&ram[0x100..0x102], &[0xFF, 0xFF]);
        let right = 0x100 + 15 * 16;
        assert_eq!(&ram[right..right + 2], &[0x00, 0x00]);
    }

    #[test]
    fn registers_read_back_only_at_a000() {
        let mut camera = Camera::default();
        camera.write_register(0xA000, 0x06);
        camera.write_register(0xA001, 0x55);
        assert_eq!(camera.read_register(0xA000), 0x06);
        assert_eq!(camera.read_register(0xA001), 0x00);
        assert_eq!(camera.read_register(0xA080), 0x06, "mirrored every $80");
    }
}
