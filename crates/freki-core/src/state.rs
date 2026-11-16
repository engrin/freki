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
pub const STATE_VERSION: u32 = 1;

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
pub struct Rewind {
    snapshots: VecDeque<Vec<u8>>,
    capacity: usize,
}

impl Rewind {
    pub fn new(capacity: usize) -> Self {
        Self {
            snapshots: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, state: Vec<u8>) {
        if self.capacity == 0 {
            return;
        }
        if self.snapshots.len() == self.capacity {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back(state);
    }

    /// Removes and returns the most recent snapshot.
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        self.snapshots.pop_back()
    }

    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    pub fn clear(&mut self) {
        self.snapshots.clear();
    }

    /// Total bytes held, for diagnostics.
    pub fn memory_used(&self) -> usize {
        self.snapshots.iter().map(Vec::len).sum()
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
