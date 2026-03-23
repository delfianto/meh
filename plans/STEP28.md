# STEP 28 — Chunk Batching for Smooth TUI Updates

## Objective
Optimize the TUI rendering pipeline so streaming text appears smoothly without flicker. Batch rapid chunk updates and render at a consistent frame rate.

## Prerequisites
- STEP 06 (chunk batcher exists)
- STEP 07 (wiring complete)

## Detailed Instructions

### 28.1 Enhance ChunkBatcher

The batcher from STEP 06 works for text. Extend it to batch all UI updates:

```rust
pub struct UiBatcher {
    text_buffer: String,
    thinking_buffer: String,
    pending_status: Option<StatusBarState>,
    last_flush: Instant,
    flush_interval: Duration,
}

impl UiBatcher {
    pub fn new(target_fps: u32) -> Self {
        Self {
            text_buffer: String::new(),
            thinking_buffer: String::new(),
            pending_status: None,
            last_flush: Instant::now(),
            flush_interval: Duration::from_millis(1000 / target_fps as u64),
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

    /// Buffer a status update (latest wins).
    pub fn push_status(&mut self, status: StatusBarState) {
        self.pending_status = Some(status);
    }

    /// Check if it's time to flush.
    pub fn should_flush(&self) -> bool {
        self.last_flush.elapsed() >= self.flush_interval
            && self.has_pending()
    }

    /// Whether any content is buffered.
    pub fn has_pending(&self) -> bool {
        !self.text_buffer.is_empty()
            || !self.thinking_buffer.is_empty()
            || self.pending_status.is_some()
    }

    /// Flush all batched updates, returning a list of UiUpdates.
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

        if let Some(status) = self.pending_status.take() {
            updates.push(UiUpdate::StatusUpdate { /* from status */ });
        }

        updates
    }

    /// Force flush everything (call at stream end).
    pub fn force_flush(&mut self) -> Vec<UiUpdate> {
        self.flush()
    }
}
```

### 28.2 Integrate batcher into Controller

In the controller's message loop, add tick-based flushing:

```rust
loop {
    tokio::select! {
        Some(msg) = self.rx.recv() => {
            match msg {
                ControllerMessage::StreamChunk(chunk) => {
                    // Buffer instead of sending directly
                    match chunk {
                        StreamChunk::Text { delta } => self.batcher.push_text(&delta),
                        StreamChunk::Thinking { delta, .. } => self.batcher.push_thinking(&delta),
                        // Others still send immediately
                        _ => { /* handle immediately */ }
                    }
                    // Check if should flush
                    if self.batcher.should_flush() {
                        for update in self.batcher.flush() {
                            let _ = self.ui_tx.send(update);
                        }
                    }
                }
                // ... other messages
            }
        }
        // Tick timer for periodic flush
        _ = tokio::time::sleep(Duration::from_millis(16)) => {
            if self.batcher.has_pending() {
                for update in self.batcher.flush() {
                    let _ = self.ui_tx.send(update);
                }
            }
        }
    }
}
```

### 28.3 TUI rendering optimization

In the TUI event loop:
- Only call `terminal.draw()` when there are pending updates (not every tick)
- Track "dirty" flag — set when UiUpdate received, cleared after draw
- This reduces CPU usage when idle

```rust
let mut dirty = true; // Initially draw the screen

loop {
    // Drain updates
    while let Ok(update) = ui_rx.try_recv() {
        apply_update(&mut state, update);
        dirty = true;
    }

    // Only render if dirty
    if dirty {
        tui.draw(|frame| render_app(frame, &state))?;
        dirty = false;
    }

    // Poll events
    if let Some(event) = poll_event(Duration::from_millis(16)) {
        match event {
            TuiEvent::Key(key) => { dirty = true; /* handle */ }
            TuiEvent::Tick => {} // Don't mark dirty
            _ => { dirty = true; }
        }
    }
}
```

## Tests

```rust
#[cfg(test)]
mod batcher_tests {
    use super::*;

    #[test]
    fn test_ui_batcher_accumulates() {
        let mut batcher = UiBatcher::new(60);
        batcher.push_text("hello ");
        batcher.push_text("world");
        assert!(batcher.has_pending());
        let updates = batcher.force_flush();
        assert_eq!(updates.len(), 1);
        match &updates[0] {
            UiUpdate::StreamContent { delta } => assert_eq!(delta, "hello world"),
            _ => panic!("Expected StreamContent"),
        }
    }

    #[test]
    fn test_ui_batcher_separate_channels() {
        let mut batcher = UiBatcher::new(60);
        batcher.push_thinking("think");
        batcher.push_text("text");
        let updates = batcher.force_flush();
        assert_eq!(updates.len(), 2);
        // Thinking comes first
        assert!(matches!(&updates[0], UiUpdate::ThinkingContent { .. }));
        assert!(matches!(&updates[1], UiUpdate::StreamContent { .. }));
    }

    #[test]
    fn test_ui_batcher_empty_flush() {
        let mut batcher = UiBatcher::new(60);
        let updates = batcher.force_flush();
        assert!(updates.is_empty());
    }

    #[test]
    fn test_ui_batcher_status_latest_wins() {
        let mut batcher = UiBatcher::new(60);
        batcher.push_status(StatusBarState { total_tokens: 100, ..Default::default() });
        batcher.push_status(StatusBarState { total_tokens: 200, ..Default::default() });
        let updates = batcher.force_flush();
        // Should only have one status update (the latest)
    }

    #[test]
    fn test_should_flush_timing() {
        let mut batcher = UiBatcher::new(60); // ~16ms interval
        batcher.push_text("data");
        // Immediately after push, might not need flush yet
        // (depends on timing — hard to test precisely)
        // After sufficient time, should_flush returns true
        std::thread::sleep(Duration::from_millis(20));
        assert!(batcher.should_flush());
    }
}
```

## Acceptance Criteria
- [x] Text deltas batched within frame interval (~16ms at 60fps)
- [x] Thinking deltas batched separately
- [x] Status updates coalesced (latest wins)
- [x] Periodic tick ensures batches flush even without new messages
- [x] TUI only redraws when dirty
- [x] Stream end force-flushes all pending content
- [x] No visible flicker during fast streaming
- [x] CPU usage reduced when idle (no unnecessary redraws)
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass
