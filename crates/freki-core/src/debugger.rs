//! Breakpoints and watchpoints.
//!
//! - A **breakpoint** stops before the instruction at its address executes.
//! - A **watchpoint** stops after an instruction that reads or writes a watched address
//!   (only CPU accesses count; debugger reads, cheats and DMA do not).

use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// A watched address range, triggering on reads, writes or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watchpoint {
    pub start: u16,
    pub end: u16,
    pub read: bool,
    pub write: bool,
}

impl Watchpoint {
    pub fn new(start: u16, end: u16, read: bool, write: bool) -> Self {
        Self {
            start: start.min(end),
            end: start.max(end),
            read,
            write,
        }
    }

    fn matches(&self, addr: u16, access: Access) -> bool {
        (self.start..=self.end).contains(&addr)
            && match access {
                Access::Read => self.read,
                Access::Write => self.write,
            }
    }
}

/// The access that triggered a watchpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchHit {
    pub addr: u16,
    pub value: u8,
    pub access: Access,
}

/// Why execution stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// About to execute the instruction at `pc`.
    Breakpoint { pc: u16 },
    /// The instruction at `pc` accessed a watched address.
    Watch { pc: u16, hit: WatchHit },
}

/// Breakpoints and watchpoints of a machine. Not part of save states.
#[derive(Debug, Default, Clone)]
pub struct Breakpoints {
    pub pc: BTreeSet<u16>,
    pub watches: Vec<Watchpoint>,
    hit: Option<WatchHit>,
}

impl Breakpoints {
    pub fn is_empty(&self) -> bool {
        self.pc.is_empty() && self.watches.is_empty()
    }

    /// Records a CPU memory access; the first matching one is kept until taken.
    pub(crate) fn access(&mut self, addr: u16, value: u8, access: Access) {
        if self.hit.is_none() && self.watches.iter().any(|w| w.matches(addr, access)) {
            self.hit = Some(WatchHit {
                addr,
                value,
                access,
            });
        }
    }

    pub(crate) fn take_hit(&mut self) -> Option<WatchHit> {
        self.hit.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchpoints_match_ranges_and_access_kinds() {
        let mut b = Breakpoints::default();
        b.watches.push(Watchpoint::new(0xC010, 0xC000, false, true)); // reversed range
        b.access(0xC005, 1, Access::Read);
        assert_eq!(b.take_hit(), None, "read-only access to a write watch");
        b.access(0xC005, 7, Access::Write);
        b.access(0xC006, 8, Access::Write);
        assert_eq!(
            b.take_hit(),
            Some(WatchHit {
                addr: 0xC005,
                value: 7,
                access: Access::Write
            }),
            "first hit wins"
        );
        assert_eq!(b.take_hit(), None);
    }
}
