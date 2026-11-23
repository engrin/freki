//! Pages from the Game Boy Printer as pictures and PNG files.

use std::error::Error;
use std::fs;
use std::io::BufWriter;
use std::path::Path;

use freki_core::PAPER_WIDTH;

/// Grey of each shade on the printer's paper: 0 is the paper itself.
pub const GREYS: [u8; 4] = [0xFF, 0xAA, 0x55, 0x00];

/// `shades` (0–3, [`PAPER_WIDTH`] per row) as RGBA pixels.
pub fn rgba(shades: &[u8]) -> Vec<u8> {
    shades
        .iter()
        .flat_map(|&s| {
            let g = GREYS[usize::from(s & 3)];
            [g, g, g, 0xFF]
        })
        .collect()
}

/// Saves `shades` as a greyscale PNG.
pub fn save_png(path: &Path, shades: &[u8]) -> Result<(), Box<dyn Error>> {
    let height = shades.len() / PAPER_WIDTH;
    let file =
        fs::File::create(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), PAPER_WIDTH as u32, height as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let grey: Vec<u8> = shades.iter().map(|&s| GREYS[usize::from(s & 3)]).collect();
    encoder.write_header()?.write_image_data(&grey)?;
    Ok(())
}
