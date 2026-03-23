//! Batches rapid UI updates across text, thinking, and status channels.
//!
//! When the LLM streams at high speed, rendering every individual chunk
//! causes flicker. The `UiBatcher` accumulates updates within a frame
//! interval and flushes them as combined batches, ensuring the TUI
//! renders at a consistent frame rate.

use crate::controller::messages::UiUpdate;
use std::time::{Duration, Instant};

/// Batches UI updates for frame-rate-limited rendering.
pub struct UiBatcher {
    text_buffer: String,
    thinking_buffer: String,
    pending_tokens: Option<u64>,
    pending_cost: Option<f64>,
    pending_context_tokens: Option<u64>,
    last_flush: Instant,
    flush_interval: Duration,
}

impl UiBatcher {
    /// Creates a new batcher targeting the given frames per second.
    #[allow(clippy::cast_possible_truncation)]
    pub fn new(target_fps: u32) -> Self {
        Self {
            text_buffer: String::new(),
            thinking_buffer: String::new(),
            pending_tokens: None,
            pending_cost: None,
            pending_context_tokens: None,
            last_flush: Instant::now(),
            flush_interval: Duration::from_millis(1000 / u64::from(target_fps)),
        }
    }

    /// Buffer a text delta.
    pub fn push_text(&mut self, delta: &str) {
        self.text_buffer.push_str(delta);
    }

    /// Buffer a thinking delta.
    pub fn push_thinking(&mut self, delta: &str) {
        self.thinking_buffer.push_str(delta);
    }

    /// Buffer a status update (latest wins for each field).
    pub const fn push_status(&mut self, tokens: Option<u64>, cost: Option<f64>, context: Option<u64>) {
        if let Some(t) = tokens {
            self.pending_tokens = Some(t);
        }
        if let Some(c) = cost {
            self.pending_cost = Some(c);
        }
        if let Some(ct) = context {
            self.pending_context_tokens = Some(ct);
        }
    }

    /// Whether any content is buffered.
    pub fn has_pending(&self) -> bool {
        !self.text_buffer.is_empty()
            || !self.thinking_buffer.is_empty()
            || self.pending_tokens.is_some()
            || self.pending_cost.is_some()
    }

    /// Check if it's time to flush based on the frame interval.
    pub fn should_flush(&self) -> bool {
        self.has_pending() && self.last_flush.elapsed() >= self.flush_interval
    }

    /// Flush all batched updates, returning `UiUpdate`s to send.
    pub fn flush(&mut self) -> Vec<UiUpdate> {
        self.last_flush = Instant::now();
        let mut updates = Vec::new();

        if !self.thinking_buffer.is_empty() {
            updates.push(UiUpdate::ThinkingContent {
                delta: std::mem::take(&mut self.thinking_buffer),
            });
        }

        if !self.text_buffer.is_empty() {
            updates.push(UiUpdate::StreamContent {
                delta: std::mem::take(&mut self.text_buffer),
            });
        }

        if self.pending_tokens.is_some() || self.pending_cost.is_some() {
            updates.push(UiUpdate::StatusUpdate {
                mode: None,
                tokens: self.pending_tokens.take(),
                cost: self.pending_cost.take(),
                is_streaming: None,
                is_yolo: None,
                context_tokens: self.pending_context_tokens.take(),
                context_window: None,
            });
        }

        updates
    }

    /// Force flush everything (call at stream end).
    pub fn force_flush(&mut self) -> Vec<UiUpdate> {
        self.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batcher_accumulates_text() {
        let mut batcher = UiBatcher::new(60);
        batcher.push_text("hello ");
        batcher.push_text("world");
        assert!(batcher.has_pending());
        let updates = batcher.force_flush();
        assert_eq!(updates.len(), 1);
        assert!(matches!(&updates[0], UiUpdate::StreamContent { delta } if delta == "hello world"));
    }

    #[test]
    fn batcher_separate_channels() {
        let mut batcher = UiBatcher::new(60);
        batcher.push_thinking("think");
        batcher.push_text("text");
        let updates = batcher.force_flush();
        assert_eq!(updates.len(), 2);
        assert!(matches!(&updates[0], UiUpdate::ThinkingContent { .. }));
        assert!(matches!(&updates[1], UiUpdate::StreamContent { .. }));
    }

    #[test]
    fn batcher_empty_flush() {
        let mut batcher = UiBatcher::new(60);
        let updates = batcher.force_flush();
        assert!(updates.is_empty());
    }

    #[test]
    fn batcher_status_latest_wins() {
        let mut batcher = UiBatcher::new(60);
        batcher.push_status(Some(100), Some(0.01), None);
        batcher.push_status(Some(200), Some(0.02), None);
        let updates = batcher.force_flush();
        assert_eq!(updates.len(), 1);
        match &updates[0] {
            UiUpdate::StatusUpdate { tokens, cost, .. } => {
                assert_eq!(*tokens, Some(200));
                assert_eq!(*cost, Some(0.02));
            }
            other => panic!("Expected StatusUpdate, got {other:?}"),
        }
    }

    #[test]
    fn batcher_status_with_context() {
        let mut batcher = UiBatcher::new(60);
        batcher.push_status(Some(500), None, Some(1000));
        let updates = batcher.force_flush();
        assert_eq!(updates.len(), 1);
        match &updates[0] {
            UiUpdate::StatusUpdate {
                tokens,
                context_tokens,
                ..
            } => {
                assert_eq!(*tokens, Some(500));
                assert_eq!(*context_tokens, Some(1000));
            }
            other => panic!("Expected StatusUpdate, got {other:?}"),
        }
    }

    #[test]
    fn should_flush_timing() {
        let mut batcher = UiBatcher::new(60);
        batcher.push_text("data");
        std::thread::sleep(Duration::from_millis(20));
        assert!(batcher.should_flush());
    }

    #[test]
    fn should_flush_empty_returns_false() {
        let batcher = UiBatcher::new(60);
        assert!(!batcher.should_flush());
    }

    #[test]
    fn flush_clears_buffers() {
        let mut batcher = UiBatcher::new(60);
        batcher.push_text("text");
        batcher.push_thinking("think");
        batcher.push_status(Some(100), None, None);
        let _ = batcher.force_flush();
        assert!(!batcher.has_pending());
        let updates = batcher.force_flush();
        assert!(updates.is_empty());
    }

    #[test]
    fn combined_text_thinking_status() {
        let mut batcher = UiBatcher::new(60);
        batcher.push_thinking("reason");
        batcher.push_text("answer");
        batcher.push_status(Some(500), Some(0.05), None);
        let updates = batcher.force_flush();
        assert_eq!(updates.len(), 3);
        assert!(matches!(&updates[0], UiUpdate::ThinkingContent { .. }));
        assert!(matches!(&updates[1], UiUpdate::StreamContent { .. }));
        assert!(matches!(&updates[2], UiUpdate::StatusUpdate { .. }));
    }
}
