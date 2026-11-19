//! Pictures for the Game Boy Camera: a PNG or JPEG file, made greyscale, cropped to the
//! sensor's shape and scaled to its 128 × 120 pixels.

use std::error::Error;
use std::path::Path;

use freki_core::camera::{SENSOR_HEIGHT, SENSOR_WIDTH};
use image::imageops::FilterType;

/// Loads `path` as the camera's view: [`SENSOR_WIDTH`] × [`SENSOR_HEIGHT`] grey levels.
pub fn load(path: &Path) -> Result<Vec<u8>, Box<dyn Error>> {
    let picture = image::open(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .into_luma8();
    Ok(fit(&picture))
}

/// The middle of `picture` in the sensor's aspect ratio, scaled to the sensor's size.
fn fit(picture: &image::GrayImage) -> Vec<u8> {
    let (w, h) = (picture.width(), picture.height());
    let (sw, sh) = (SENSOR_WIDTH as u32, SENSOR_HEIGHT as u32);
    // Crop the longer side so the picture has the sensor's shape.
    let (cw, ch) = if w * sh > h * sw {
        (h * sw / sh, h)
    } else {
        (w, w * sh / sw)
    };
    let cropped =
        image::imageops::crop_imm(picture, (w - cw) / 2, (h - ch) / 2, cw.max(1), ch.max(1));
    image::imageops::resize(&cropped.to_image(), sw, sh, FilterType::Triangle).into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_pictures_are_cropped_to_the_middle() {
        // 400 × 100: black on the left and right quarters, white in the middle.
        let picture = image::GrayImage::from_fn(400, 100, |x, _| {
            image::Luma([if (100..300).contains(&x) { 255 } else { 0 }])
        });
        let view = fit(&picture);
        assert_eq!(view.len(), SENSOR_WIDTH * SENSOR_HEIGHT);
        // The crop (about 107 px wide) lies inside the white middle.
        assert!(
            view.iter().all(|&v| v > 200),
            "only the white middle is seen"
        );
    }
}
