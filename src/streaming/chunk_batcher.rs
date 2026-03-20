//! Batches rapid streaming updates to prevent TUI flicker.
//!
//! When the LLM streams text at high speed (hundreds of chunks per second),
//! rendering each chunk individually causes visible flicker. The batcher
//! accumulates text within a time window and flushes it as a single update.

use std::time::{Duration, Instant};

/// Batches text deltas within a configurable time window.
pub struct ChunkBatcher {
    buffer: String,
    last_flush: Instant,
    flush_interval: Duration,
}

impl ChunkBatcher {
    /// Creates a new batcher with the given flush interval (16ms ≈ 60fps is a good default).
    pub fn new(flush_interval: Duration) -> Self {
        Self {
            buffer: String::new(),
            last_flush: Instant::now(),
            flush_interval,
        }
    }

    /// Adds text to the batch buffer.
    pub fn push(&mut self, text: &str) {
        self.buffer.push_str(text);
    }

    /// Returns whether the batch should be flushed based on the time interval.
    pub fn should_flush(&self) -> bool {
        !self.buffer.is_empty() && self.last_flush.elapsed() >= self.flush_interval
    }

    /// Flushes the batch if the interval has elapsed.
    pub fn flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() || self.last_flush.elapsed() < self.flush_interval {
            return None;
        }
        self.last_flush = Instant::now();
        Some(std::mem::take(&mut self.buffer))
    }

    /// Force flushes regardless of timing (use at end-of-stream or state transitions).
    pub fn force_flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            None
        } else {
            self.last_flush = Instant::now();
            Some(std::mem::take(&mut self.buffer))
        }
    }

    /// Returns whether the buffer has pending (unflushed) content.
    pub fn has_pending(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Returns the current buffer length.
    pub fn pending_len(&self) -> usize {
        self.buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batcher_accumulates() {
        let mut batcher = ChunkBatcher::new(Duration::from_millis(100));
        batcher.push("hello ");
        batcher.push("world");
        assert!(batcher.has_pending());
        assert_eq!(batcher.pending_len(), 11);
        let flushed = batcher.force_flush().unwrap();
        assert_eq!(flushed, "hello world");
        assert!(!batcher.has_pending());
        assert_eq!(batcher.pending_len(), 0);
    }

    #[test]
    fn batcher_empty_flush() {
        let mut batcher = ChunkBatcher::new(Duration::from_millis(16));
        assert!(batcher.flush().is_none());
        assert!(batcher.force_flush().is_none());
        assert!(!batcher.has_pending());
    }

    #[test]
    fn batcher_timing_blocks_flush() {
        let mut batcher = ChunkBatcher::new(Duration::from_secs(10));
        batcher.push("text");
        assert!(batcher.flush().is_none());
        assert_eq!(batcher.force_flush().unwrap(), "text");
    }

    #[test]
    fn batcher_zero_interval() {
        let mut batcher = ChunkBatcher::new(Duration::from_millis(0));
        batcher.push("text");
        assert!(batcher.should_flush());
        assert_eq!(batcher.flush().unwrap(), "text");
    }

    #[test]
    fn batcher_multiple_flushes() {
        let mut batcher = ChunkBatcher::new(Duration::from_millis(0));
        batcher.push("first");
        assert_eq!(batcher.force_flush().unwrap(), "first");
        batcher.push("second");
        assert_eq!(batcher.force_flush().unwrap(), "second");
        assert!(batcher.force_flush().is_none());
    }
}
