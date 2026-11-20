//! A webcam as the Game Boy Camera's view: a thread captures frames, turns them grey and
//! fits them to the sensor ([`camera_picture::fit`]); the app takes the newest one each frame.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::camera_picture;

/// The pixel layouts webcams deliver (nokhwa's frame formats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "webcam"), allow(dead_code))]
pub enum Layout {
    Mjpeg,
    Yuyv,
    Nv12,
    Gray,
    Rgb,
    Bgr,
}

/// The grey picture in a frame of `width` × `height` pixels, or `None` if `data` is too short
/// or not a JPEG.
#[cfg_attr(not(feature = "webcam"), allow(dead_code))]
pub fn to_grey(layout: Layout, width: u32, height: u32, data: &[u8]) -> Option<image::GrayImage> {
    let pixels = width as usize * height as usize;
    let luma = |r: u8, g: u8, b: u8| ((r as u32 * 77 + g as u32 * 150 + b as u32 * 29) >> 8) as u8;
    let grey = match layout {
        Layout::Mjpeg => {
            return image::load_from_memory_with_format(data, image::ImageFormat::Jpeg)
                .ok()
                .map(|p| p.into_luma8());
        }
        // Y0 U Y1 V: every other byte is a brightness.
        Layout::Yuyv => data.get(..pixels * 2)?.iter().step_by(2).copied().collect(),
        // A plane of brightness first, then the colour.
        Layout::Nv12 | Layout::Gray => data.get(..pixels)?.to_vec(),
        Layout::Rgb => data
            .get(..pixels * 3)?
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[r, g, b]| luma(r, g, b))
            .collect(),
        Layout::Bgr => data
            .get(..pixels * 3)?
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[b, g, r]| luma(r, g, b))
            .collect(),
    };
    image::GrayImage::from_raw(width, height, grey)
}

/// What the capture thread hands over.
#[derive(Default)]
struct Shared {
    /// The newest view, not yet taken.
    view: Option<Vec<u8>>,
    /// Why capturing stopped.
    error: Option<String>,
    /// The webcam's name, once it's open.
    opened: Option<String>,
}

/// A running webcam; dropping it stops the capture.
pub struct Webcam {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
}

/// What [`Webcam::poll`] found.
pub enum Event {
    /// The webcam named here started.
    Opened(String),
    /// A new view for the camera.
    View(Vec<u8>),
    /// The capture stopped with this error.
    Failed(String),
}

impl Webcam {
    /// Starts capturing from the webcam called `name` (`None` or not found: the first one).
    pub fn start(name: Option<String>) -> Webcam {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, st) = (shared.clone(), stop.clone());
        let spawned = std::thread::Builder::new()
            .name("webcam".into())
            .spawn(move || {
                if let Err(e) = backend::capture(name, &s, &st) {
                    s.lock().unwrap().error = Some(e);
                }
            });
        if let Err(e) = spawned {
            shared.lock().unwrap().error = Some(format!("cannot start the webcam: {e}"));
        }
        Webcam { shared, stop }
    }

    /// What happened since the last call, the most important first.
    pub fn poll(&self) -> Option<Event> {
        let mut shared = self.shared.lock().unwrap();
        if let Some(e) = shared.error.take() {
            return Some(Event::Failed(e));
        }
        if let Some(name) = shared.opened.take() {
            return Some(Event::Opened(name));
        }
        shared.view.take().map(Event::View)
    }
}

impl Drop for Webcam {
    fn drop(&mut self) {
        // The thread notices after its current frame and closes the webcam.
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The names of the webcams found.
pub fn list() -> Result<Vec<String>, String> {
    backend::list()
}

#[cfg(feature = "webcam")]
mod backend {
    use super::*;
    use nokhwa::utils::{
        ApiBackend, CameraFormat, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
    };

    /// Formats freki decodes, the cheapest first.
    const FORMATS: [FrameFormat; 6] = [
        FrameFormat::YUYV,
        FrameFormat::NV12,
        FrameFormat::GRAY,
        FrameFormat::MJPEG,
        FrameFormat::RAWRGB,
        FrameFormat::RAWBGR,
    ];

    /// Asks for camera access where the system wants that (macOS); `true` if granted.
    fn initialize() -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        nokhwa::nokhwa_initialize(move |granted| {
            let _ = tx.lock().unwrap().send(granted);
        });
        rx.recv_timeout(std::time::Duration::from_secs(60))
            .unwrap_or(false)
    }

    fn cameras() -> Result<Vec<nokhwa::utils::CameraInfo>, String> {
        if !initialize() {
            return Err("freki may not use the webcam (see the system's privacy settings)".into());
        }
        nokhwa::query(ApiBackend::Auto).map_err(|e| format!("cannot look for webcams: {e}"))
    }

    pub fn list() -> Result<Vec<String>, String> {
        Ok(cameras()?.iter().map(|c| c.human_name()).collect())
    }

    pub fn capture(
        name: Option<String>,
        shared: &Mutex<Shared>,
        stop: &AtomicBool,
    ) -> Result<(), String> {
        let found = cameras()?;
        let info = name
            .and_then(|n| found.iter().find(|c| c.human_name() == n))
            .or(found.first())
            .ok_or("no webcam found")?;
        // Small frames are plenty for 128 × 120 and cheap to decode.
        let small = CameraFormat::new(Resolution::new(320, 240), FrameFormat::YUYV, 30);
        let open = |kind| {
            nokhwa::Camera::new(
                info.index().clone(),
                RequestedFormat::with_formats(kind, &FORMATS),
            )
        };
        let mut camera = open(RequestedFormatType::Closest(small))
            .or_else(|_| open(RequestedFormatType::AbsoluteHighestFrameRate))
            .map_err(|e| format!("cannot open {}: {e}", info.human_name()))?;
        camera
            .open_stream()
            .map_err(|e| format!("cannot start {}: {e}", info.human_name()))?;
        shared.lock().unwrap().opened = Some(info.human_name());
        let mut failures = 0;
        while !stop.load(Ordering::Relaxed) {
            let frame = camera
                .frame()
                .map_err(|e| format!("{} stopped: {e}", info.human_name()))?;
            let layout = match frame.source_frame_format() {
                FrameFormat::MJPEG => Layout::Mjpeg,
                FrameFormat::YUYV => Layout::Yuyv,
                FrameFormat::NV12 => Layout::Nv12,
                FrameFormat::GRAY => Layout::Gray,
                FrameFormat::RAWRGB => Layout::Rgb,
                FrameFormat::RAWBGR => Layout::Bgr,
            };
            let res = frame.resolution();
            match to_grey(layout, res.width(), res.height(), frame.buffer()) {
                Some(grey) => {
                    failures = 0;
                    shared.lock().unwrap().view = Some(camera_picture::fit(&grey));
                }
                // A broken frame now and then is fine; only a webcam that never works isn't.
                None if failures < 30 => failures += 1,
                None => {
                    return Err(format!(
                        "cannot read the pictures of {} ({layout:?})",
                        info.human_name()
                    ));
                }
            }
        }
        let _ = camera.stop_stream();
        Ok(())
    }
}

#[cfg(not(feature = "webcam"))]
mod backend {
    use super::*;

    const MISSING: &str = "this freki was built without webcam support";

    pub fn list() -> Result<Vec<String>, String> {
        Err(MISSING.into())
    }

    pub fn capture(_: Option<String>, _: &Mutex<Shared>, _: &AtomicBool) -> Result<(), String> {
        let _ = camera_picture::fit;
        Err(MISSING.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_turn_grey() {
        // 2 × 1 pixels.
        let yuyv = [10, 128, 200, 128];
        assert_eq!(
            to_grey(Layout::Yuyv, 2, 1, &yuyv).unwrap().into_raw(),
            [10, 200]
        );
        let nv12 = [10, 200, 128, 128];
        assert_eq!(
            to_grey(Layout::Nv12, 2, 1, &nv12).unwrap().into_raw(),
            [10, 200]
        );
        let rgb = [255, 255, 255, 255, 0, 0];
        assert_eq!(
            to_grey(Layout::Rgb, 2, 1, &rgb).unwrap().into_raw(),
            [255, 76]
        );
        let bgr = [0, 0, 255, 0, 0, 0];
        assert_eq!(
            to_grey(Layout::Bgr, 2, 1, &bgr).unwrap().into_raw(),
            [76, 0]
        );
    }

    #[test]
    fn short_or_broken_frames_are_skipped() {
        assert!(to_grey(Layout::Yuyv, 2, 1, &[1, 2, 3]).is_none());
        assert!(to_grey(Layout::Rgb, 2, 2, &[0; 11]).is_none());
        assert!(to_grey(Layout::Mjpeg, 2, 2, b"not a jpeg").is_none());
    }

    #[test]
    fn jpeg_frames_are_decoded() {
        let picture = image::GrayImage::from_pixel(16, 8, image::Luma([180]));
        let mut jpeg = Vec::new();
        image::DynamicImage::ImageLuma8(picture)
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let grey = to_grey(Layout::Mjpeg, 16, 8, &jpeg).unwrap();
        assert_eq!(grey.dimensions(), (16, 8));
        assert!(grey.pixels().all(|p| p.0[0].abs_diff(180) <= 2));
    }
}
