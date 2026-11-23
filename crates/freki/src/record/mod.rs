//! Video recording to WebM: AV1 video (rav1e) and Opus sound (rusty-opus), both pure
//! Rust, in a WebM file written by [`webm`].
//!
//! The picture is recorded as shown (Super Game Boy border, linked screens and frame
//! blending included) and scaled up without smoothing, so each Game Boy pixel becomes a
//! block of at least 2×2 pixels: the AV1 colour format that every player supports
//! (4:2:0, one colour sample per 2×2 pixels) then keeps colours exact at pixel edges.
//! The sound of each emulated frame is resampled to exactly one frame's worth of 48 kHz
//! audio, so picture and sound cannot drift apart. Encoding runs on its own thread.

mod webm;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use freki_core::{CLOCK_HZ, CYCLES_PER_FRAME};
use rav1e::prelude::{
    ChromaSampling, ColorDescription, ColorPrimaries, Config, Context, EncoderConfig,
    EncoderStatus, FrameType, MatrixCoefficients, PixelRange, Rational, TransferCharacteristics,
};
use rusty_opus::{Application, OpusEncoder};

use webm::{AUDIO_TRACK, Tracks, VIDEO_TRACK, WebmWriter};

/// Each Game Boy pixel becomes a SCALE×SCALE block.
pub const SCALE: usize = 2;
const SAMPLE_RATE: u64 = 48_000;
/// Opus frames of 20 ms.
const OPUS_FRAME: usize = 960;
/// The encoder delay libopus and rusty-opus have for music (`OpusHead` pre-skip).
const OPUS_PRE_SKIP: u16 = 312;
const OPUS_BITRATE: i32 = 128_000;

enum Message {
    Frame { rgba: Vec<u8>, audio: Vec<f32> },
}

/// What a finished recording came to.
pub struct Summary {
    pub path: PathBuf,
    pub frames: u64,
    pub bytes: u64,
}

/// A recording in progress.
pub struct Recorder {
    tx: Sender<Message>,
    thread: JoinHandle<Result<Summary, String>>,
    size: [usize; 2],
    frames: u64,
    pub path: PathBuf,
    clock: u32,
}

impl Recorder {
    /// Starts recording pictures of `size` (width, height) to `path`.
    /// `clock`: the machine's clock in Hz ([`freki_core::GameBoy::clock_hz`]), which sets
    /// how long a frame lasts.
    pub fn start(path: &Path, size: [usize; 2], clock: u32) -> Result<Self, String> {
        let (tx, rx) = channel();
        let file = path.to_path_buf();
        let thread = std::thread::Builder::new()
            .name("freki-recorder".into())
            .spawn(move || encode(&file, size, rx, clock))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx,
            thread,
            size,
            frames: 0,
            path: path.to_path_buf(),
            clock,
        })
    }

    /// Adds one emulated frame: its picture (RGBA, `size`) and the sound it produced
    /// (interleaved stereo, at whatever rate). Fails if the picture changed size.
    pub fn frame(&mut self, rgba: &[u8], size: [usize; 2], audio: &[f32]) -> Result<(), String> {
        if size != self.size {
            return Err("the picture changed size".into());
        }
        self.frames += 1;
        self.tx
            .send(Message::Frame {
                rgba: rgba.to_vec(),
                audio: audio.to_vec(),
            })
            .map_err(|_| "the video encoder stopped".to_string())
    }

    /// Seconds recorded so far.
    pub fn seconds(&self) -> f64 {
        self.frames as f64 * f64::from(CYCLES_PER_FRAME) / f64::from(self.clock)
    }

    /// Ends the recording. The encoder finishes in the background; join the handle for
    /// the result.
    pub fn stop(self) -> JoinHandle<Result<Summary, String>> {
        drop(self.tx);
        self.thread
    }
}

/// Milliseconds at which frame `n` is shown.
fn frame_ms(n: u64, clock: u32) -> u64 {
    n * 1000 * u64::from(CYCLES_PER_FRAME) / u64::from(clock)
}

/// 48 kHz sample frames from the start to the end of emulated frame `n`.
fn samples_until(n: u64, clock: u32) -> u64 {
    n * SAMPLE_RATE * u64::from(CYCLES_PER_FRAME) / u64::from(clock)
}

/// Frames per piece of video encoded on its own (2 seconds, starting with a key frame).
const CHUNK: usize = 120;
/// Quantizer (0–255): lower is better and bigger. Pixel art compresses well.
const QUANTIZER: usize = 100;

/// One encoded video frame: (frame number, key frame, data).
type VideoPacket = (u64, bool, Vec<u8>);

fn encode(
    path: &Path,
    size: [usize; 2],
    rx: Receiver<Message>,
    clock: u32,
) -> Result<Summary, String> {
    let (width, height) = (size[0] * SCALE, size[1] * SCALE);
    let mut opus = OpusEncoder::new(SAMPLE_RATE as i32, 2, Application::Audio)
        .map_err(|e| format!("sound encoder: {e:?}"))?;
    opus.bitrate_bps = OPUS_BITRATE;
    let tracks = Tracks {
        width: width as u32,
        height: height as u32,
        av1_config: video_encoder(width, height)?.container_sequence_header(),
        opus_pre_skip: OPUS_PRE_SKIP,
    };
    let mut out = WebmWriter::create(path, &tracks).map_err(|e| e.to_string())?;
    let pool = ChunkPool::new(width, height);
    let mut resampler = Resampler::default();
    let mut pcm: Vec<f32> = Vec::new();
    let mut packet = vec![0u8; 4000];
    let mut opus_frames = 0u64;
    let mut frames = 0u64;
    let mut yuv = Yuv::new(width, height);
    let mut chunk: Vec<[Vec<u8>; 3]> = Vec::with_capacity(CHUNK);
    let mut chunks_sent = 0;
    let mut written = Reorder {
        clock,
        next: 0,
        waiting: Default::default(),
    };

    let mut write_audio =
        |pcm: &mut Vec<f32>, out: &mut WebmWriter, all: bool| -> Result<(), String> {
            while pcm.len() >= OPUS_FRAME * 2 || (all && !pcm.is_empty()) {
                if pcm.len() < OPUS_FRAME * 2 {
                    pcm.resize(OPUS_FRAME * 2, 0.0);
                }
                let len = opus
                    .encode(&pcm[..OPUS_FRAME * 2], OPUS_FRAME, &mut packet)
                    .map_err(|e| format!("sound encoder: {e:?}"))?;
                pcm.drain(..OPUS_FRAME * 2);
                out.add(AUDIO_TRACK, opus_frames * 20, true, packet[..len].to_vec())
                    .map_err(|e| e.to_string())?;
                opus_frames += 1;
            }
            Ok(())
        };

    for message in rx {
        let Message::Frame { rgba, audio } = message;
        yuv.fill(&rgba, size);
        chunk.push([yuv.y.clone(), yuv.u.clone(), yuv.v.clone()]);
        if chunk.len() == CHUNK {
            pool.send(chunks_sent, std::mem::take(&mut chunk))?;
            chunks_sent += 1;
        }
        let count = (samples_until(frames + 1, clock) - samples_until(frames, clock)) as usize;
        resampler.resample(&audio, count, &mut pcm);
        write_audio(&mut pcm, &mut out, false)?;
        while let Some(result) = pool.try_receive() {
            written.add(result?, &mut out)?;
        }
        frames += 1;
    }
    if !chunk.is_empty() {
        pool.send(chunks_sent, chunk)?;
        chunks_sent += 1;
    }
    for result in pool.finish() {
        written.add(result?, &mut out)?;
    }
    if written.next != chunks_sent {
        return Err("a piece of the video is missing".into());
    }
    write_audio(&mut pcm, &mut out, true)?;
    out.finish(frame_ms(frames, clock))
        .map_err(|e| e.to_string())?;
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(Summary {
        path: path.to_path_buf(),
        frames,
        bytes,
    })
}

/// Finished pieces, written to the file in order.
struct Reorder {
    clock: u32,
    next: usize,
    waiting: std::collections::BTreeMap<usize, Vec<VideoPacket>>,
}

impl Reorder {
    fn add(
        &mut self,
        (index, packets): (usize, Vec<VideoPacket>),
        out: &mut WebmWriter,
    ) -> Result<(), String> {
        self.waiting.insert(index, packets);
        while let Some(packets) = self.waiting.remove(&self.next) {
            for (frame, key, data) in packets {
                out.add(VIDEO_TRACK, frame_ms(frame, self.clock), key, data)
                    .map_err(|e| e.to_string())?;
            }
            self.next += 1;
        }
        Ok(())
    }
}

type ChunkResult = Result<(usize, Vec<VideoPacket>), String>;
/// A piece of video to encode: its number and its frames (Y, U and V planes each).
type Chunk = (usize, Vec<[Vec<u8>; 3]>);

/// Worker threads that each encode whole pieces of video with their own encoder. The
/// encoders share one configuration, so the pieces join into one stream.
struct ChunkPool {
    jobs: Option<Sender<Chunk>>,
    results: Receiver<ChunkResult>,
    workers: Vec<JoinHandle<()>>,
}

impl ChunkPool {
    fn new(width: usize, height: usize) -> Self {
        let (jobs, job_rx) = channel::<Chunk>();
        let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
        let (result_tx, results) = channel();
        let count = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 16));
        let workers = (0..count)
            .map(|_| {
                let job_rx = job_rx.clone();
                let result_tx = result_tx.clone();
                std::thread::spawn(move || {
                    loop {
                        let job = job_rx.lock().map(|rx| rx.recv());
                        let Ok(Ok((index, frames))) = job else { return };
                        let first = (index * CHUNK) as u64;
                        let result = encode_chunk(width, height, &frames).map(|packets| {
                            let packets = packets
                                .into_iter()
                                .map(|(n, key, data)| (first + n, key, data))
                                .collect();
                            (index, packets)
                        });
                        if result_tx.send(result).is_err() {
                            return;
                        }
                    }
                })
            })
            .collect();
        Self {
            jobs: Some(jobs),
            results,
            workers,
        }
    }

    fn send(&self, index: usize, frames: Vec<[Vec<u8>; 3]>) -> Result<(), String> {
        self.jobs
            .as_ref()
            .and_then(|jobs| jobs.send((index, frames)).ok())
            .ok_or_else(|| "the video encoder stopped".to_string())
    }

    fn try_receive(&self) -> Option<ChunkResult> {
        self.results.try_recv().ok()
    }

    /// Waits for every piece; returns the results not yet received.
    fn finish(mut self) -> Vec<ChunkResult> {
        self.jobs = None;
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        self.results.try_iter().collect()
    }
}

/// Encodes one piece of video (planes Y, U, V per frame). Frame numbers count from 0.
fn encode_chunk(
    width: usize,
    height: usize,
    frames: &[[Vec<u8>; 3]],
) -> Result<Vec<VideoPacket>, String> {
    let mut video = video_encoder(width, height)?;
    let mut packets = Vec::new();
    let mut receive = |video: &mut Context<u8>| -> Result<(), String> {
        loop {
            match video.receive_packet() {
                Ok(packet) => {
                    let mut data = packet.data;
                    // WebM leaves out AV1's temporal delimiter (OBU type 2, empty).
                    if data.starts_with(&[0x12, 0x00]) {
                        data.drain(..2);
                    }
                    packets.push((
                        packet.input_frameno,
                        packet.frame_type == FrameType::KEY,
                        data,
                    ));
                }
                Err(EncoderStatus::Encoded) => {}
                Err(EncoderStatus::NeedMoreData | EncoderStatus::LimitReached) => {
                    return Ok(());
                }
                Err(e) => return Err(format!("video encoder: {e:?}")),
            }
        }
    };
    for planes in frames {
        let mut frame = video.new_frame();
        for (plane, data) in frame.planes.iter_mut().zip(planes) {
            let stride = plane.cfg.width;
            plane.copy_from_raw_u8(data, stride, 1);
        }
        video
            .send_frame(frame)
            .map_err(|e| format!("video encoder: {e:?}"))?;
        receive(&mut video)?;
    }
    video.flush();
    receive(&mut video)?;
    Ok(packets)
}

fn video_encoder(width: usize, height: usize) -> Result<Context<u8>, String> {
    let mut enc = EncoderConfig::with_speed_preset(10);
    enc.width = width;
    enc.height = height;
    // One tick per Game Boy frame (70224 of its 4194304 clock cycles per second).
    enc.time_base = Rational::new(u64::from(CYCLES_PER_FRAME), u64::from(CLOCK_HZ));
    enc.chroma_sampling = ChromaSampling::Cs420;
    enc.pixel_range = PixelRange::Full;
    enc.color_description = Some(ColorDescription {
        color_primaries: ColorPrimaries::BT709,
        transfer_characteristics: TransferCharacteristics::SRGB,
        matrix_coefficients: MatrixCoefficients::BT709,
    });
    // Each piece is one stretch from a key frame.
    enc.min_key_frame_interval = CHUNK as u64;
    enc.max_key_frame_interval = CHUNK as u64;
    enc.quantizer = QUANTIZER;
    enc.bitrate = 0;
    Config::new()
        .with_encoder_config(enc)
        .with_threads(1)
        .new_context()
        .map_err(|e| format!("video encoder: {e}"))
}

/// A picture in YUV 4:2:0 (full range, BT.709), scaled up by [`SCALE`].
struct Yuv {
    width: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl Yuv {
    fn new(width: usize, height: usize) -> Self {
        let chroma = width.div_ceil(2) * height.div_ceil(2);
        Self {
            width,
            y: vec![0; width * height],
            u: vec![128; chroma],
            v: vec![128; chroma],
        }
    }

    /// Converts `rgba` (`size`, unscaled) into the planes.
    fn fill(&mut self, rgba: &[u8], size: [usize; 2]) {
        let [w, h] = size;
        let chroma_width = self.width.div_ceil(2);
        for sy in 0..h {
            for sx in 0..w {
                let px = &rgba[(sy * w + sx) * 4..][..3];
                let (r, g, b) = (f32::from(px[0]), f32::from(px[1]), f32::from(px[2]));
                let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                let u = (b - y) / 1.8556 + 128.0;
                let v = (r - y) / 1.5748 + 128.0;
                let byte = |x: f32| x.round().clamp(0.0, 255.0) as u8;
                for dy in 0..SCALE {
                    let row = (sy * SCALE + dy) * self.width + sx * SCALE;
                    self.y[row..row + SCALE].fill(byte(y));
                }
                // SCALE is even: each source pixel covers whole 2×2 chroma blocks.
                for dy in 0..SCALE / 2 {
                    let row = (sy * SCALE / 2 + dy) * chroma_width + sx * SCALE / 2;
                    self.u[row..row + SCALE / 2].fill(byte(u));
                    self.v[row..row + SCALE / 2].fill(byte(v));
                }
            }
        }
    }
}

/// Turns each frame's sound (any number of stereo samples) into exactly the number of
/// 48 kHz samples that frame lasts, by linear interpolation.
#[derive(Default)]
struct Resampler {
    /// The last input sample of the previous frame, to interpolate from.
    last: [f32; 2],
}

impl Resampler {
    fn resample(&mut self, input: &[f32], count: usize, out: &mut Vec<f32>) {
        let frames = input.len() / 2;
        if frames == 0 {
            // No sound produced (e.g. the APU is not running): silence.
            out.resize(out.len() + count * 2, 0.0);
            return;
        }
        let at = |i: isize| -> [f32; 2] {
            if i < 0 {
                self.last
            } else {
                let i = (i as usize).min(frames - 1);
                [input[i * 2], input[i * 2 + 1]]
            }
        };
        for k in 0..count {
            // Position of output sample k in input samples, with the previous frame's
            // last sample at -1.
            let pos = (k + 1) as f64 * frames as f64 / count as f64 - 1.0;
            let i = pos.floor() as isize;
            let t = (pos - i as f64) as f32;
            let (a, b) = (at(i), at(i + 1));
            out.push(a[0] + (b[0] - a[0]) * t);
            out.push(a[1] + (b[1] - a[1]) * t);
        }
        self.last = at(frames as isize - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_of_sound_add_up_to_48_khz() {
        let one_minute = (60.0 * f64::from(CLOCK_HZ) / f64::from(CYCLES_PER_FRAME)) as u64;
        let samples = samples_until(one_minute, CLOCK_HZ);
        assert!((samples as i64 - 60 * 48_000).abs() < 1000, "{samples}");
        assert_eq!(
            frame_ms(60, CLOCK_HZ),
            1004,
            "60 frames last just over a second"
        );
    }

    #[test]
    fn resampling_keeps_the_count_and_the_level() {
        let mut r = Resampler { last: [0.5, -0.5] };
        let input: Vec<f32> = [0.5, -0.5].repeat(735);
        let mut out = Vec::new();
        r.resample(&input, 804, &mut out);
        assert_eq!(out.len(), 804 * 2);
        assert!(out.chunks(2).all(|s| s == [0.5, -0.5]));
    }

    #[test]
    fn colours_scale_up_into_whole_chroma_blocks() {
        let mut yuv = Yuv::new(2 * SCALE, SCALE);
        // A white and a black pixel.
        yuv.fill(&[255, 255, 255, 255, 0, 0, 0, 255], [2, 1]);
        assert!(yuv.y[..SCALE].iter().all(|&y| y == 255));
        assert!(yuv.y[SCALE..2 * SCALE].iter().all(|&y| y == 0));
        assert!(
            yuv.u.iter().chain(&yuv.v).all(|&c| c == 128),
            "grey has no colour"
        );
    }
}
