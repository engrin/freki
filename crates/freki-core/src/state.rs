//! Save states and rewind.
//!
//! A save state is the complete hardware state (CPU, memory, PPU, APU, timers,
//! cartridge RAM and MBC registers) encoded with `postcard`, behind a small header.
//! The ROM itself is not stored; it is taken from the cartridge the state is loaded into.

use std::collections::VecDeque;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Identifies a freki save state.
const MAGIC: [u8; 8] = *b"FREKISS\0";
/// Bump whenever the serialized layout of any hardware struct changes.
pub const STATE_VERSION: u32 = 18;

/// Written before the hardware state; checked before anything is decoded.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Header {
    magic: [u8; 8],
    version: u32,
    /// Cartridge identity: title plus header and global checksums.
    title: String,
    checksums: [u8; 3],
}

impl Header {
    pub(crate) fn new(title: String, checksums: [u8; 3]) -> Self {
        Self {
            magic: MAGIC,
            version: STATE_VERSION,
            title,
            checksums,
        }
    }

    /// Checks that a state with this header can be loaded into the given cartridge.
    pub(crate) fn check(&self, title: &str, checksums: [u8; 3]) -> Result<(), StateError> {
        if self.magic != MAGIC {
            return Err(StateError::NotAState);
        }
        if self.version != STATE_VERSION {
            return Err(StateError::Version(self.version));
        }
        if self.title != title || self.checksums != checksums {
            return Err(StateError::OtherGame(self.title.clone()));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum StateError {
    NotAState,
    /// Saved by a freki version with a different state layout.
    Version(u32),
    /// Saved from a different game (its title is included).
    OtherGame(String),
    Corrupt(postcard::Error),
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAState => write!(f, "not a freki save state"),
            Self::Version(v) => write!(
                f,
                "save state format v{v} is not supported (expected v{STATE_VERSION})"
            ),
            Self::OtherGame(title) => write!(f, "save state belongs to another game ({title:?})"),
            Self::Corrupt(e) => write!(f, "save state is corrupt: {e}"),
        }
    }
}

impl std::error::Error for StateError {}

impl From<postcard::Error> for StateError {
    fn from(e: postcard::Error) -> Self {
        Self::Corrupt(e)
    }
}

/// A bounded history of save states for rewinding. When full, the oldest is dropped.
///
/// Only the newest snapshot is kept whole. Each older one is stored as its difference to
/// the next newer one (XOR, with runs of zeros compressed): consecutive states differ in
/// only a few bytes, so this takes a fraction of the memory. Both `push` and `pop` touch
/// only the newest snapshot and one difference.
pub struct Rewind {
    newest: Option<Vec<u8>>,
    /// `older.back()` turns `newest` into the snapshot before it, and so on backwards.
    older: VecDeque<Vec<u8>>,
    capacity: usize,
}

impl Rewind {
    pub fn new(capacity: usize) -> Self {
        Self {
            newest: None,
            older: VecDeque::new(),
            capacity,
        }
    }

    pub fn push(&mut self, state: Vec<u8>) {
        if self.capacity == 0 {
            return;
        }
        if let Some(previous) = self.newest.take() {
            self.older.push_back(diff(&state, &previous));
            if self.older.len() >= self.capacity {
                self.older.pop_front();
            }
        }
        self.newest = Some(state);
    }

    /// Removes and returns the most recent snapshot.
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        let state = self.newest.take()?;
        if let Some(diff) = self.older.pop_back() {
            self.newest = Some(apply(&state, &diff));
        }
        Some(state)
    }

    pub fn len(&self) -> usize {
        self.older.len() + usize::from(self.newest.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.newest.is_none()
    }

    pub fn clear(&mut self) {
        self.newest = None;
        self.older.clear();
    }

    /// Total bytes held, for diagnostics.
    pub fn memory_used(&self) -> usize {
        self.newest.as_ref().map_or(0, Vec::len) + self.older.iter().map(Vec::len).sum::<usize>()
    }
}

/// Encodes how to turn `base` into `target`: target's length, then the bytes of
/// `target XOR base` (base padded with zeros) as alternating (zero run, literal run)
/// pairs, lengths as LEB128 varints.
fn diff(base: &[u8], target: &[u8]) -> Vec<u8> {
    let byte_at = |i: usize| target[i] ^ base.get(i).copied().unwrap_or(0);
    let mut out = Vec::new();
    write_varint(&mut out, target.len());
    let mut i = 0;
    while i < target.len() {
        let zeros_start = i;
        while i < target.len() && byte_at(i) == 0 {
            i += 1;
        }
        let literal_start = i;
        // A literal run ends at the next stretch of at least 4 zero bytes.
        while i < target.len() && (i..(i + 4).min(target.len())).any(|j| byte_at(j) != 0) {
            i += 1;
        }
        write_varint(&mut out, literal_start - zeros_start);
        write_varint(&mut out, i - literal_start);
        out.extend((literal_start..i).map(byte_at));
    }
    out
}

/// Rebuilds the target of [`diff`] from `base`.
fn apply(base: &[u8], diff: &[u8]) -> Vec<u8> {
    let mut pos = 0;
    let len = read_varint(diff, &mut pos);
    let mut out: Vec<u8> = (0..len)
        .map(|i| base.get(i).copied().unwrap_or(0))
        .collect();
    let mut i = 0;
    while i < len {
        i += read_varint(diff, &mut pos);
        let literal = read_varint(diff, &mut pos);
        for byte in &mut out[i..i + literal] {
            *byte ^= diff[pos];
            pos += 1;
        }
        i += literal;
    }
    out
}

fn write_varint(out: &mut Vec<u8>, mut value: usize) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_varint(data: &[u8], pos: &mut usize) -> usize {
    let (mut value, mut shift) = (0usize, 0);
    loop {
        let byte = data[*pos];
        *pos += 1;
        value |= ((byte & 0x7F) as usize) << shift;
        if byte < 0x80 {
            return value;
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewind_drops_oldest_when_full_and_pops_newest_first() {
        let mut r = Rewind::new(2);
        r.push(vec![1]);
        r.push(vec![2]);
        r.push(vec![3]);
        assert_eq!(r.len(), 2);
        assert_eq!(r.pop(), Some(vec![3]));
        assert_eq!(r.pop(), Some(vec![2]));
        assert_eq!(r.pop(), None);
    }

    #[test]
    fn diffs_round_trip_including_length_changes() {
        let base: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        let mut target = base.clone();
        target[3] ^= 0xFF;
        target[500..510].fill(0x42);
        for t in [
            target.clone(),
            target[..700].to_vec(),
            [&target[..], &[1, 2, 3]].concat(),
        ] {
            let d = diff(&base, &t);
            assert_eq!(apply(&base, &d), t);
        }
        assert!(diff(&base, &target).len() < 40, "small changes, small diff");
        assert_eq!(apply(&base, &diff(&base, &[])), Vec::<u8>::new());
    }

    #[test]
    fn rewind_returns_every_snapshot_and_saves_memory() {
        let mut r = Rewind::new(100);
        let mut state = vec![0u8; 50_000];
        let mut states = Vec::new();
        for frame in 0..60u32 {
            state[(frame as usize * 811) % 50_000] = frame as u8;
            state[123] = frame as u8;
            states.push(state.clone());
            r.push(state.clone());
        }
        assert!(r.memory_used() < 60_000, "{} bytes", r.memory_used());
        for expected in states.iter().rev() {
            assert_eq!(r.pop().as_ref(), Some(expected));
        }
        assert!(r.is_empty());
    }

    #[test]
    fn header_rejects_other_games_and_versions() {
        let h = Header::new("TETRIS".into(), [1, 2, 3]);
        assert!(h.check("TETRIS", [1, 2, 3]).is_ok());
        assert!(matches!(
            h.check("ZELDA", [1, 2, 3]),
            Err(StateError::OtherGame(_))
        ));
        let mut old = Header::new("TETRIS".into(), [1, 2, 3]);
        old.version = 0;
        assert!(matches!(
            old.check("TETRIS", [1, 2, 3]),
            Err(StateError::Version(0))
        ));
    }
}
