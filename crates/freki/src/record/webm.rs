//! A minimal WebM (Matroska) writer for one AV1 video track and one Opus audio track.
//!
//! Blocks are written in clusters as they come (sorted by time across the two tracks).
//! When the file is finished, the segment size, the duration and a seek head pointing to
//! the cues (one per cluster that starts with a video key frame) are filled in, so players
//! can show the length and seek.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

const EBML: u32 = 0x1A45_DFA3;
const EBML_VERSION: u32 = 0x4286;
const EBML_READ_VERSION: u32 = 0x42F7;
const EBML_MAX_ID_LENGTH: u32 = 0x42F2;
const EBML_MAX_SIZE_LENGTH: u32 = 0x42F3;
const DOC_TYPE: u32 = 0x4282;
const DOC_TYPE_VERSION: u32 = 0x4287;
const DOC_TYPE_READ_VERSION: u32 = 0x4285;
const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114D_9B74;
const SEEK: u32 = 0x4DBB;
const SEEK_ID: u32 = 0x53AB;
const SEEK_POSITION: u32 = 0x53AC;
const VOID: u32 = 0xEC;
const INFO: u32 = 0x1549_A966;
const TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const MUXING_APP: u32 = 0x4D80;
const WRITING_APP: u32 = 0x5741;
const DURATION: u32 = 0x4489;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_UID: u32 = 0x73C5;
const TRACK_TYPE: u32 = 0x83;
const FLAG_LACING: u32 = 0x9C;
const CODEC_ID: u32 = 0x86;
const CODEC_PRIVATE: u32 = 0x63A2;
const CODEC_DELAY: u32 = 0x56AA;
const SEEK_PRE_ROLL: u32 = 0x56BB;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const AUDIO: u32 = 0xE1;
const SAMPLING_FREQUENCY: u32 = 0xB5;
const CHANNELS: u32 = 0x9F;
const CLUSTER: u32 = 0x1F43_B675;
const TIMESTAMP: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const CUES: u32 = 0x1C53_BB6B;
const CUE_POINT: u32 = 0xBB;
const CUE_TIME: u32 = 0xB3;
const CUE_TRACK_POSITIONS: u32 = 0xB7;
const CUE_TRACK: u32 = 0xF7;
const CUE_CLUSTER_POSITION: u32 = 0xF1;

/// Track numbers.
pub const VIDEO_TRACK: u64 = 1;
pub const AUDIO_TRACK: u64 = 2;

/// Bytes kept free after the EBML header for the seek head written at the end.
const SEEK_HEAD_ROOM: usize = 96;
/// A new cluster starts at a video key frame or after this many milliseconds.
const CLUSTER_MS: u64 = 2000;

/// An EBML element ID, written as its bytes (the length marker is part of the ID).
fn id(out: &mut Vec<u8>, id: u32) {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().take_while(|&&b| b == 0).count();
    out.extend_from_slice(&bytes[skip..]);
}

/// An element size as an EBML variable-length integer of the shortest length.
fn size(out: &mut Vec<u8>, n: u64) {
    let mut len = 1;
    while len < 8 && n >= (1 << (7 * len)) - 1 {
        len += 1;
    }
    let marked = n | 1 << (7 * len);
    out.extend_from_slice(&marked.to_be_bytes()[8 - len..]);
}

fn element(out: &mut Vec<u8>, element_id: u32, body: &[u8]) {
    id(out, element_id);
    size(out, body.len() as u64);
    out.extend_from_slice(body);
}

fn uint(out: &mut Vec<u8>, element_id: u32, n: u64) {
    let bytes = n.to_be_bytes();
    let skip = bytes.iter().take_while(|&&b| b == 0).count().min(7);
    element(out, element_id, &bytes[skip..]);
}

fn float(out: &mut Vec<u8>, element_id: u32, x: f64) {
    element(out, element_id, &x.to_be_bytes());
}

fn master(out: &mut Vec<u8>, element_id: u32, body: impl FnOnce(&mut Vec<u8>)) {
    let mut inner = Vec::new();
    body(&mut inner);
    element(out, element_id, &inner);
}

/// The two tracks' setup.
pub struct Tracks {
    pub width: u32,
    pub height: u32,
    /// AV1 codec configuration record (`av1C`).
    pub av1_config: Vec<u8>,
    /// Samples the Opus decoder drops at the start (encoder delay).
    pub opus_pre_skip: u16,
}

struct Block {
    track: u64,
    time_ms: u64,
    key: bool,
    data: Vec<u8>,
}

pub struct WebmWriter {
    out: BufWriter<File>,
    /// File offset of the segment's data (positions in the seek head and cues count
    /// from here).
    segment_start: u64,
    /// File offset of the duration's 8-byte value.
    duration_at: u64,
    /// Blocks waiting for the other track to catch up, per track (in time order).
    video: Vec<Block>,
    audio: Vec<Block>,
    cluster: Vec<u8>,
    cluster_time: Option<u64>,
    /// (time, segment position) of clusters that start with a video key frame.
    cues: Vec<(u64, u64)>,
    cluster_starts_with_key: bool,
    last_time: u64,
}

impl WebmWriter {
    pub fn create(path: &Path, tracks: &Tracks) -> io::Result<Self> {
        let mut out = BufWriter::new(File::create(path)?);
        let mut head = Vec::new();
        master(&mut head, EBML, |b| {
            uint(b, EBML_VERSION, 1);
            uint(b, EBML_READ_VERSION, 1);
            uint(b, EBML_MAX_ID_LENGTH, 4);
            uint(b, EBML_MAX_SIZE_LENGTH, 8);
            element(b, DOC_TYPE, b"webm");
            uint(b, DOC_TYPE_VERSION, 4);
            uint(b, DOC_TYPE_READ_VERSION, 2);
        });
        // Segment with an 8-byte size, filled in at the end.
        id(&mut head, SEGMENT);
        head.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        let segment_start = head.len() as u64;
        // Room for the seek head.
        void(&mut head, SEEK_HEAD_ROOM);
        let mut info = Vec::new();
        uint(&mut info, TIMESTAMP_SCALE, 1_000_000);
        element(&mut info, MUXING_APP, b"freki");
        element(&mut info, WRITING_APP, b"freki");
        id(&mut info, DURATION);
        size(&mut info, 8);
        let duration_in_info = info.len();
        info.extend_from_slice(&0f64.to_be_bytes());
        id(&mut head, INFO);
        size(&mut head, info.len() as u64);
        let duration_at = (head.len() + duration_in_info) as u64;
        head.extend_from_slice(&info);
        master(&mut head, TRACKS, |b| {
            master(b, TRACK_ENTRY, |t| {
                uint(t, TRACK_NUMBER, VIDEO_TRACK);
                uint(t, TRACK_UID, VIDEO_TRACK);
                uint(t, TRACK_TYPE, 1);
                uint(t, FLAG_LACING, 0);
                element(t, CODEC_ID, b"V_AV1");
                element(t, CODEC_PRIVATE, &tracks.av1_config);
                master(t, VIDEO, |v| {
                    uint(v, PIXEL_WIDTH, u64::from(tracks.width));
                    uint(v, PIXEL_HEIGHT, u64::from(tracks.height));
                });
            });
            master(b, TRACK_ENTRY, |t| {
                uint(t, TRACK_NUMBER, AUDIO_TRACK);
                uint(t, TRACK_UID, AUDIO_TRACK);
                uint(t, TRACK_TYPE, 2);
                uint(t, FLAG_LACING, 0);
                element(t, CODEC_ID, b"A_OPUS");
                element(t, CODEC_PRIVATE, &opus_head(tracks.opus_pre_skip));
                uint(
                    t,
                    CODEC_DELAY,
                    u64::from(tracks.opus_pre_skip) * 1_000_000_000 / 48_000,
                );
                uint(t, SEEK_PRE_ROLL, 80_000_000);
                master(t, AUDIO, |a| {
                    float(a, SAMPLING_FREQUENCY, 48_000.0);
                    uint(a, CHANNELS, 2);
                });
            });
        });
        out.write_all(&head)?;
        Ok(Self {
            out,
            segment_start,
            duration_at,
            video: Vec::new(),
            audio: Vec::new(),
            cluster: Vec::new(),
            cluster_time: None,
            cues: Vec::new(),
            cluster_starts_with_key: false,
            last_time: 0,
        })
    }

    /// Adds an encoded frame of `track` shown at `time_ms`.
    pub fn add(&mut self, track: u64, time_ms: u64, key: bool, data: Vec<u8>) -> io::Result<()> {
        let block = Block {
            track,
            time_ms,
            key,
            data,
        };
        if track == VIDEO_TRACK {
            self.video.push(block);
        } else {
            self.audio.push(block);
        }
        // Write what can no longer be overtaken by the other track.
        while let (Some(v), Some(a)) = (self.video.first(), self.audio.first()) {
            let from_video = v.time_ms <= a.time_ms;
            let block = if from_video {
                self.video.remove(0)
            } else {
                self.audio.remove(0)
            };
            self.write_block(block)?;
        }
        Ok(())
    }

    fn write_block(&mut self, block: Block) -> io::Result<()> {
        let video_key = block.track == VIDEO_TRACK && block.key;
        let start_new = match self.cluster_time {
            None => true,
            Some(t) => video_key || block.time_ms >= t + CLUSTER_MS,
        };
        if start_new {
            self.flush_cluster()?;
            self.cluster_time = Some(block.time_ms);
            self.cluster_starts_with_key = video_key;
            uint(&mut self.cluster, TIMESTAMP, block.time_ms);
        }
        let relative = block.time_ms - self.cluster_time.unwrap_or(0);
        let mut body = Vec::with_capacity(block.data.len() + 4);
        size(&mut body, block.track);
        body.extend_from_slice(&(relative as i16).to_be_bytes());
        body.push(if block.key { 0x80 } else { 0 });
        body.extend_from_slice(&block.data);
        element(&mut self.cluster, SIMPLE_BLOCK, &body);
        self.last_time = self.last_time.max(block.time_ms);
        Ok(())
    }

    fn flush_cluster(&mut self) -> io::Result<()> {
        let Some(time) = self.cluster_time.take() else {
            return Ok(());
        };
        let position = self.out.stream_position()? - self.segment_start;
        if self.cluster_starts_with_key {
            self.cues.push((time, position));
        }
        let mut cluster = Vec::with_capacity(self.cluster.len() + 12);
        element(&mut cluster, CLUSTER, &self.cluster);
        self.out.write_all(&cluster)?;
        self.cluster.clear();
        Ok(())
    }

    /// Writes the rest and fills in the seek head, duration and segment size.
    /// `end_ms`: the time the recording ends.
    pub fn finish(mut self, end_ms: u64) -> io::Result<()> {
        let mut rest: Vec<Block> = self.video.drain(..).chain(self.audio.drain(..)).collect();
        rest.sort_by_key(|b| (b.time_ms, b.track));
        for block in rest {
            self.write_block(block)?;
        }
        self.flush_cluster()?;
        let cues_position = self.out.stream_position()? - self.segment_start;
        let mut cues = Vec::new();
        master(&mut cues, CUES, |c| {
            for &(time, position) in &self.cues {
                master(c, CUE_POINT, |p| {
                    uint(p, CUE_TIME, time);
                    master(p, CUE_TRACK_POSITIONS, |t| {
                        uint(t, CUE_TRACK, VIDEO_TRACK);
                        uint(t, CUE_CLUSTER_POSITION, position);
                    });
                });
            }
        });
        if !self.cues.is_empty() {
            self.out.write_all(&cues)?;
        }
        let end = self.out.stream_position()?;

        // Seek head: where the info, tracks and cues are.
        let mut seek_head = Vec::new();
        // The info element follows the seek head's room and ends with the duration;
        // the tracks follow it.
        let mut seeks = vec![
            (INFO, SEEK_HEAD_ROOM as u64),
            (TRACKS, self.duration_at + 8 - self.segment_start),
        ];
        if !self.cues.is_empty() {
            seeks.push((CUES, cues_position));
        }
        master(&mut seek_head, SEEK_HEAD, |h| {
            for &(element_id, position) in &seeks {
                master(h, SEEK, |s| {
                    let mut id_bytes = Vec::new();
                    id(&mut id_bytes, element_id);
                    element(s, SEEK_ID, &id_bytes);
                    uint(s, SEEK_POSITION, position);
                });
            }
        });
        let room = SEEK_HEAD_ROOM - seek_head.len();
        void(&mut seek_head, room);
        self.out.seek(SeekFrom::Start(self.segment_start))?;
        self.out.write_all(&seek_head)?;

        self.out.seek(SeekFrom::Start(self.duration_at))?;
        let duration = end_ms.max(self.last_time) as f64;
        self.out.write_all(&duration.to_be_bytes())?;

        self.out.seek(SeekFrom::Start(self.segment_start - 8))?;
        let segment_size = end - self.segment_start;
        self.out
            .write_all(&(segment_size | 1 << 56).to_be_bytes())?;
        self.out.flush()
    }
}

/// A Void element taking exactly `total` bytes (at least 2).
fn void(out: &mut Vec<u8>, total: usize) {
    id(out, VOID);
    // An 8-byte size keeps the arithmetic simple: 1 (ID) + 8 (size) + body.
    let body = total.saturating_sub(9);
    out.extend_from_slice(&((body as u64) | 1 << 56).to_be_bytes());
    out.resize(out.len() + body, 0);
}

/// The Opus identification header (`OpusHead`) for stereo at 48 kHz.
fn opus_head(pre_skip: u16) -> Vec<u8> {
    let mut head = b"OpusHead".to_vec();
    head.push(1); // version
    head.push(2); // channels
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&48_000u32.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes()); // output gain
    head.push(0); // channel mapping family
    head
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_variable_length_integers() {
        let enc = |n| {
            let mut v = Vec::new();
            size(&mut v, n);
            v
        };
        assert_eq!(enc(0), [0x80]);
        assert_eq!(enc(126), [0xFE]);
        assert_eq!(enc(127), [0x40, 0x7F], "all ones in one byte means unknown");
        assert_eq!(enc(300), [0x41, 0x2C]);
    }

    #[test]
    fn voids_fill_exactly() {
        let mut v = Vec::new();
        void(&mut v, SEEK_HEAD_ROOM);
        assert_eq!(v.len(), SEEK_HEAD_ROOM);
    }

    #[test]
    fn writes_a_file_with_blocks_in_time_order() {
        let dir = std::env::temp_dir().join(format!("freki-webm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.webm");
        let tracks = Tracks {
            width: 320,
            height: 288,
            av1_config: vec![0x81, 0, 0, 0],
            opus_pre_skip: 312,
        };
        let mut w = WebmWriter::create(&path, &tracks).unwrap();
        w.add(AUDIO_TRACK, 0, true, b"aud0".to_vec()).unwrap();
        w.add(AUDIO_TRACK, 20, true, b"aud1".to_vec()).unwrap();
        w.add(VIDEO_TRACK, 0, true, b"vid0".to_vec()).unwrap();
        w.add(VIDEO_TRACK, 17, false, b"vid1".to_vec()).unwrap();
        w.finish(40).unwrap();
        let data = std::fs::read(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(&data[..4], &EBML.to_be_bytes());
        // Blocks in time order (video first at equal times).
        let at = |p: &[u8]| data.windows(4).position(|w| w == p).unwrap();
        let order = [at(b"vid0"), at(b"aud0"), at(b"vid1"), at(b"aud1")];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
        // The segment size covers the rest of the file.
        let seg = data
            .windows(4)
            .position(|w| w == SEGMENT.to_be_bytes())
            .unwrap();
        let size = u64::from_be_bytes(data[seg + 4..seg + 12].try_into().unwrap()) & !(1 << 56);
        assert_eq!(seg as u64 + 12 + size, data.len() as u64);
    }
}
