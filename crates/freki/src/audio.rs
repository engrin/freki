//! Audio output through cpal.
//!
//! The emulator pushes interleaved stereo `f32` samples into a shared queue; the audio
//! callback pulls from it. The main loop throttles emulation on the queue length, so the
//! sound card's clock sets the emulation speed and audio never drifts out of sync.

use std::collections::VecDeque;
use std::error::Error;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};

type Queue = Arc<Mutex<VecDeque<f32>>>;

pub struct Audio {
    /// Keeps the stream alive; dropping it stops playback.
    _stream: Stream,
    queue: Queue,
    sample_rate: u32,
}

impl Audio {
    /// Opens the default output device at its preferred format.
    pub fn open() -> Result<Self, Box<dyn Error>> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no audio output device")?;
        let supported = device.default_output_config()?;
        let config = supported.config();
        let queue: Queue = Arc::default();

        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &config, queue.clone())?,
            SampleFormat::I16 => build::<i16>(&device, &config, queue.clone())?,
            SampleFormat::U16 => build::<u16>(&device, &config, queue.clone())?,
            SampleFormat::I32 => build::<i32>(&device, &config, queue.clone())?,
            other => return Err(format!("unsupported audio sample format {other}").into()),
        };
        stream.play()?;

        Ok(Self {
            _stream: stream,
            queue,
            sample_rate: config.sample_rate,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Appends interleaved stereo samples for playback.
    pub fn push(&self, samples: &[f32]) {
        self.queue.lock().unwrap().extend(samples);
    }

    /// Queues `frames` stereo frames of silence (to refill an empty buffer cleanly).
    pub fn push_silence(&self, frames: usize) {
        self.queue
            .lock()
            .unwrap()
            .extend(std::iter::repeat_n(0.0, frames * 2));
    }

    /// Number of stereo frames waiting to be played.
    pub fn queued_frames(&self) -> usize {
        self.queue.lock().unwrap().len() / 2
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    queue: Queue,
) -> Result<Stream, Box<dyn Error>>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let stream = device.build_output_stream::<T, _, _>(
        *config,
        move |data: &mut [T], _| {
            let mut queue = queue.lock().unwrap();
            for frame in data.chunks_mut(channels) {
                // On underrun, play silence rather than stale data.
                let (left, right) = match (queue.pop_front(), queue.pop_front()) {
                    (Some(l), Some(r)) => (l, r),
                    _ => (0.0, 0.0),
                };
                match frame {
                    [mono] => *mono = T::from_sample((left + right) / 2.0),
                    [l, r, rest @ ..] => {
                        *l = T::from_sample(left);
                        *r = T::from_sample(right);
                        for s in rest {
                            *s = T::from_sample(0.0);
                        }
                    }
                    [] => {}
                }
            }
        },
        |err| eprintln!("freki: audio error: {err}"),
        None,
    )?;
    Ok(stream)
}
