use saint_harness_ai::canonical_blake3;
use serde_json::Value;
use std::collections::VecDeque;

pub const DOOM_RING_CAPACITY: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoomLoopStatus {
    Ok,
    Warn { repeats: usize },
    Halt { repeats: usize },
}

/// Ring-buffer detector over `(tool_name, blake3(canonical_args))`:
/// 3 identical calls emit a synthetic strategy-change warning; 5 identical calls halt the loop.
#[derive(Debug, Clone)]
pub struct DoomLoopDetector {
    warn_threshold: usize,
    halt_threshold: usize,
    ring: VecDeque<(String, String)>,
}

impl Default for DoomLoopDetector {
    fn default() -> Self {
        Self::new(3, 5)
    }
}

impl DoomLoopDetector {
    pub fn new(warn_threshold: usize, halt_threshold: usize) -> Self {
        Self {
            warn_threshold: warn_threshold.max(2),
            halt_threshold: halt_threshold.max(warn_threshold.max(2) + 1),
            ring: VecDeque::with_capacity(DOOM_RING_CAPACITY),
        }
    }

    pub fn record(&mut self, tool_name: &str, args: &Value) -> DoomLoopStatus {
        let hash = canonical_blake3(args).unwrap_or_else(|_| "err".to_string());
        let key = (tool_name.to_string(), hash);
        if self.ring.len() == DOOM_RING_CAPACITY {
            self.ring.pop_front();
        }
        self.ring.push_back(key.clone());

        let repeats = self.ring.iter().filter(|entry| **entry == key).count();
        if repeats >= self.halt_threshold {
            DoomLoopStatus::Halt { repeats }
        } else if repeats >= self.warn_threshold {
            DoomLoopStatus::Warn { repeats }
        } else {
            DoomLoopStatus::Ok
        }
    }
}
