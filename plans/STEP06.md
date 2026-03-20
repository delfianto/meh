# STEP 06 — Stream Processor (Text + Thinking Parsing)

## Objective
Implement the `StreamProcessor` that sits between the raw provider stream and the agent/controller. It accumulates partial tool call JSON, assembles thinking blocks, and batches rapid text updates for smooth TUI rendering.

## Prerequisites
- STEP 01-05 complete

## Detailed Instructions

### 6.1 Tool Parser (`src/streaming/tool_parser.rs`)

The tool parser handles the challenge of receiving tool call arguments as incremental JSON fragments. It must:
1. Track multiple in-flight tool calls by ID
2. Accumulate partial JSON strings
3. Attempt parsing after each fragment (for early preview)
4. Provide a regex-based fallback for extracting partial fields from incomplete JSON

```rust
//! Incremental JSON parser for streaming tool call arguments.

use std::collections::HashMap;

/// Tracks the state of a partially-received tool call.
#[derive(Debug)]
pub struct PartialToolCall {
    pub id: String,
    pub name: String,
    accumulated_json: String,
    pub complete: bool,
    pub parsed_args: Option<serde_json::Value>,
}

impl PartialToolCall {
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            accumulated_json: String::new(),
            complete: false,
            parsed_args: None,
        }
    }

    /// Append a JSON delta fragment. Tries to parse after each append
    /// so that `parsed_args` is available as early as possible.
    pub fn append(&mut self, delta: &str) {
        self.accumulated_json.push_str(delta);
        // Optimistic parse — may succeed before the stream is complete
        // if the JSON happens to be valid at this point
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&self.accumulated_json) {
            self.parsed_args = Some(value);
        }
    }

    /// Mark as complete and do final parse.
    /// Returns the parsed JSON value or an error if the JSON is malformed.
    pub fn finalize(&mut self) -> anyhow::Result<serde_json::Value> {
        self.complete = true;
        serde_json::from_str(&self.accumulated_json)
            .map_err(|e| anyhow::anyhow!("Failed to parse tool arguments: {e}"))
    }

    /// Extract partial fields from incomplete JSON using regex.
    /// This is a best-effort extraction for UI preview purposes.
    /// Returns key-value pairs found so far.
    pub fn partial_fields(&self) -> HashMap<String, String> {
        extract_partial_json_fields(&self.accumulated_json)
    }

    /// Get the raw accumulated JSON string.
    pub fn raw_json(&self) -> &str {
        &self.accumulated_json
    }
}
```

The regex-based partial field extractor:
```rust
/// Extract key-value pairs from potentially incomplete JSON.
///
/// Uses regex to find `"key": "value"` patterns, which works even
/// when the JSON is truncated mid-stream. Only extracts string values.
///
/// Example:
/// ```text
/// Input:  {"path": "/src/main.rs", "line
/// Output: {"path" => "/src/main.rs"}
/// ```
fn extract_partial_json_fields(partial: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    // Match "key": "value" patterns (value may be incomplete)
    let re = regex::Regex::new(r#""(\w+)"\s*:\s*"([^"]*)"?"#)
        .expect("regex is valid at compile time");
    for cap in re.captures_iter(partial) {
        if let (Some(key), Some(val)) = (cap.get(1), cap.get(2)) {
            fields.insert(key.as_str().to_string(), val.as_str().to_string());
        }
    }
    fields
}
```

The tracker that manages multiple concurrent tool calls:
```rust
/// Manages all in-flight tool calls during a streaming response.
///
/// Tool calls are identified by their `id` (assigned by the provider).
/// Multiple tool calls can be in-flight simultaneously (though Anthropic
/// typically sends them sequentially within a single response).
pub struct ToolCallTracker {
    active: HashMap<String, PartialToolCall>,
}

impl ToolCallTracker {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
        }
    }

    /// Start tracking a new tool call.
    pub fn start_tool_call(&mut self, id: String, name: String) {
        self.active
            .insert(id.clone(), PartialToolCall::new(id, name));
    }

    /// Append a JSON delta to a tracked tool call.
    /// Returns a reference to the updated tool call, or None if the ID is unknown.
    pub fn append_delta(&mut self, id: &str, delta: &str) -> Option<&PartialToolCall> {
        if let Some(tc) = self.active.get_mut(id) {
            tc.append(delta);
            Some(tc)
        } else {
            None
        }
    }

    /// Finalize a tool call and remove it from tracking.
    /// Returns `Some(Ok((id, name, args)))` on success,
    /// `Some(Err(...))` on parse failure, or `None` if the ID is unknown.
    pub fn finalize(
        &mut self,
        id: &str,
    ) -> Option<anyhow::Result<(String, String, serde_json::Value)>> {
        self.active
            .remove(id)
            .map(|mut tc| tc.finalize().map(|args| (tc.id, tc.name, args)))
    }

    /// Get a reference to a tracked tool call by ID.
    pub fn get(&self, id: &str) -> Option<&PartialToolCall> {
        self.active.get(id)
    }

    /// Clear all tracked tool calls.
    pub fn clear(&mut self) {
        self.active.clear();
    }

    /// Number of active tool calls being tracked.
    pub fn len(&self) -> usize {
        self.active.len()
    }

    /// Whether there are no active tool calls.
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
}
```

### 6.2 Thinking Parser (`src/streaming/thinking_parser.rs`)

The thinking parser accumulates reasoning content from the model's extended thinking feature.

```rust
//! Accumulates reasoning/thinking blocks from streaming chunks.

/// Tracks accumulated thinking content during streaming.
///
/// Lifecycle:
/// 1. `append()` called for each thinking_delta chunk
/// 2. `set_signature()` called when signature_delta arrives
/// 3. `finalize()` called when thinking block ends (text starts or stream ends)
/// 4. `reset()` called to prepare for next thinking block
#[derive(Debug, Default)]
pub struct ThinkingAccumulator {
    content: String,
    signature: Option<String>,
    is_redacted: bool,
    is_active: bool,
}

impl ThinkingAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a thinking delta.
    pub fn append(&mut self, delta: &str) {
        self.is_active = true;
        self.content.push_str(delta);
    }

    /// Set the signature (used for multi-turn thinking verification).
    pub fn set_signature(&mut self, sig: String) {
        self.signature = Some(sig);
    }

    /// Mark as redacted (the API redacted the reasoning content).
    pub fn set_redacted(&mut self) {
        self.is_redacted = true;
    }

    /// Get the current accumulated content.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Whether thinking is currently being streamed.
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// Get the current length of accumulated content.
    pub fn len(&self) -> usize {
        self.content.len()
    }

    /// Whether no content has been accumulated.
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Finalize and return the complete thinking block.
    /// Clears the active flag but preserves content until `reset()`.
    pub fn finalize(&mut self) -> ThinkingBlock {
        self.is_active = false;
        ThinkingBlock {
            content: std::mem::take(&mut self.content),
            signature: self.signature.take(),
            redacted: self.is_redacted,
        }
    }

    /// Reset all state for the next thinking block.
    pub fn reset(&mut self) {
        self.content.clear();
        self.signature = None;
        self.is_redacted = false;
        self.is_active = false;
    }
}

/// A complete thinking block (finalized accumulator output).
#[derive(Debug, Clone)]
pub struct ThinkingBlock {
    pub content: String,
    pub signature: Option<String>,
    pub redacted: bool,
}
```

### 6.3 Chunk Batcher (`src/streaming/chunk_batcher.rs`)

The chunk batcher prevents TUI flicker by coalescing rapid text updates within a configurable time window.

```rust
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
    /// Create a new batcher with the given flush interval.
    ///
    /// A good default for TUI rendering is 16ms (~60fps).
    pub fn new(flush_interval: Duration) -> Self {
        Self {
            buffer: String::new(),
            last_flush: Instant::now(),
            flush_interval,
        }
    }

    /// Add text to the batch buffer.
    pub fn push(&mut self, text: &str) {
        self.buffer.push_str(text);
    }

    /// Check if the batch should be flushed based on the time interval.
    /// Returns true if there is content AND the interval has elapsed.
    pub fn should_flush(&self) -> bool {
        !self.buffer.is_empty() && self.last_flush.elapsed() >= self.flush_interval
    }

    /// Flush the batch if the interval has elapsed.
    /// Returns `Some(text)` with the accumulated content, or `None` if empty.
    pub fn flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }
        if self.last_flush.elapsed() < self.flush_interval {
            return None;
        }
        self.last_flush = Instant::now();
        Some(std::mem::take(&mut self.buffer))
    }

    /// Force flush regardless of timing.
    /// Use this at end-of-stream or when switching from text to tool calls.
    pub fn force_flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            None
        } else {
            self.last_flush = Instant::now();
            Some(std::mem::take(&mut self.buffer))
        }
    }

    /// Whether the buffer has pending (unflushed) content.
    pub fn has_pending(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Get the current buffer length.
    pub fn pending_len(&self) -> usize {
        self.buffer.len()
    }
}
```

### 6.4 Stream Processor (`src/streaming/mod.rs`)

The `StreamProcessor` is the main integration point. It consumes raw `StreamChunk`s from the provider and produces `ProcessedEvent`s for the controller/agent.

```rust
//! Stream processing — sits between provider stream and agent/controller.
//!
//! The StreamProcessor provides:
//! - Text batching for smooth TUI rendering
//! - Thinking block accumulation and finalization
//! - Tool call JSON accumulation and parsing
//! - Automatic state transitions (e.g., finalizing thinking when text starts)

pub mod tool_parser;
pub mod thinking_parser;
pub mod chunk_batcher;

use crate::provider::StreamChunk;
use tool_parser::ToolCallTracker;
use thinking_parser::ThinkingAccumulator;
use chunk_batcher::ChunkBatcher;
use std::time::Duration;

/// Events emitted by the StreamProcessor.
/// These are higher-level than raw StreamChunks.
#[derive(Debug, Clone)]
pub enum ProcessedEvent {
    /// Batched text content for UI display.
    TextBatch(String),

    /// Incremental thinking content update (for live display).
    ThinkingDelta(String),

    /// Thinking block completed (for message history).
    ThinkingComplete(thinking_parser::ThinkingBlock),

    /// Partial tool call preview (for UI display during streaming).
    ToolCallPreview {
        id: String,
        name: String,
        partial_args: String,
    },

    /// Tool call fully received and parsed (ready for execution).
    ToolCallReady {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },

    /// Token usage information.
    Usage(crate::provider::UsageInfo),

    /// Stream completed successfully.
    Done,

    /// Error during streaming.
    Error(String),
}

/// Processes raw StreamChunks into higher-level ProcessedEvents.
///
/// Usage:
/// ```rust,ignore
/// let mut processor = StreamProcessor::new();
/// while let Some(chunk) = stream.next().await {
///     for event in processor.process(chunk?) {
///         handle_event(event);
///     }
/// }
/// for event in processor.finish() {
///     handle_event(event);
/// }
/// ```
pub struct StreamProcessor {
    tool_tracker: ToolCallTracker,
    thinking: ThinkingAccumulator,
    text_batcher: ChunkBatcher,
}

impl StreamProcessor {
    pub fn new() -> Self {
        Self {
            tool_tracker: ToolCallTracker::new(),
            thinking: ThinkingAccumulator::new(),
            text_batcher: ChunkBatcher::new(Duration::from_millis(16)),
        }
    }

    /// Create with a custom flush interval for the text batcher.
    pub fn with_flush_interval(flush_interval: Duration) -> Self {
        Self {
            tool_tracker: ToolCallTracker::new(),
            thinking: ThinkingAccumulator::new(),
            text_batcher: ChunkBatcher::new(flush_interval),
        }
    }

    /// Process a single StreamChunk and return zero or more ProcessedEvents.
    ///
    /// State transitions:
    /// - Thinking -> Text: auto-finalizes thinking block
    /// - Thinking -> ToolCall: auto-finalizes thinking block
    /// - Text -> ToolCall: force-flushes text buffer
    pub fn process(&mut self, chunk: StreamChunk) -> Vec<ProcessedEvent> {
        let mut events = Vec::new();

        match chunk {
            StreamChunk::Text { delta } => {
                // If thinking was active, finalize it first
                if self.thinking.is_active() {
                    events.push(ProcessedEvent::ThinkingComplete(
                        self.thinking.finalize(),
                    ));
                }
                self.text_batcher.push(&delta);
                if self.text_batcher.should_flush() {
                    if let Some(batch) = self.text_batcher.flush() {
                        events.push(ProcessedEvent::TextBatch(batch));
                    }
                }
            }

            StreamChunk::Thinking {
                delta,
                signature,
                redacted,
            } => {
                if redacted {
                    self.thinking.set_redacted();
                }
                if let Some(sig) = signature {
                    self.thinking.set_signature(sig);
                }
                if !delta.is_empty() {
                    self.thinking.append(&delta);
                    events.push(ProcessedEvent::ThinkingDelta(delta));
                }
            }

            StreamChunk::ToolCallDelta {
                id,
                name,
                arguments_delta,
            } => {
                // Flush any pending text first
                if let Some(batch) = self.text_batcher.force_flush() {
                    events.push(ProcessedEvent::TextBatch(batch));
                }
                // If thinking was active, finalize it
                if self.thinking.is_active() {
                    events.push(ProcessedEvent::ThinkingComplete(
                        self.thinking.finalize(),
                    ));
                }

                // Start tracking if this is a new tool call
                if self.tool_tracker.get(&id).is_none() {
                    self.tool_tracker
                        .start_tool_call(id.clone(), name.clone());
                }
                self.tool_tracker.append_delta(&id, &arguments_delta);

                events.push(ProcessedEvent::ToolCallPreview {
                    id,
                    name,
                    partial_args: arguments_delta,
                });
            }

            StreamChunk::ToolCallComplete { id, name, arguments } => {
                // The provider already parsed the complete arguments
                self.tool_tracker.clear();
                events.push(ProcessedEvent::ToolCallReady {
                    id,
                    name,
                    arguments,
                });
            }

            StreamChunk::Usage(usage) => {
                events.push(ProcessedEvent::Usage(usage));
            }

            StreamChunk::Done => {
                // Flush everything remaining
                if let Some(batch) = self.text_batcher.force_flush() {
                    events.push(ProcessedEvent::TextBatch(batch));
                }
                if self.thinking.is_active() {
                    events.push(ProcessedEvent::ThinkingComplete(
                        self.thinking.finalize(),
                    ));
                }
                events.push(ProcessedEvent::Done);
            }

            StreamChunk::Error(e) => {
                // Flush any pending content before the error
                if let Some(batch) = self.text_batcher.force_flush() {
                    events.push(ProcessedEvent::TextBatch(batch));
                }
                events.push(ProcessedEvent::Error(e));
            }
        }

        events
    }

    /// Call at the end of stream processing to get any remaining buffered events.
    /// This handles the case where the stream ends without a `Done` chunk.
    pub fn finish(&mut self) -> Vec<ProcessedEvent> {
        let mut events = Vec::new();
        if let Some(batch) = self.text_batcher.force_flush() {
            events.push(ProcessedEvent::TextBatch(batch));
        }
        if self.thinking.is_active() {
            events.push(ProcessedEvent::ThinkingComplete(
                self.thinking.finalize(),
            ));
        }
        events
    }

    /// Reset all internal state for a new stream.
    /// Call this before processing a new API response.
    pub fn reset(&mut self) {
        self.tool_tracker.clear();
        self.thinking.reset();
        let _ = self.text_batcher.force_flush();
    }
}
```

## Tests

### Tool parser tests
```rust
#[cfg(test)]
mod tool_parser_tests {
    use super::tool_parser::*;

    #[test]
    fn test_partial_tool_call_new() {
        let tc = PartialToolCall::new("tc1".to_string(), "read_file".to_string());
        assert_eq!(tc.id, "tc1");
        assert_eq!(tc.name, "read_file");
        assert!(!tc.complete);
        assert!(tc.parsed_args.is_none());
    }

    #[test]
    fn test_partial_tool_call_accumulation() {
        let mut tc = PartialToolCall::new("tc1".to_string(), "read_file".to_string());
        tc.append(r#"{"pa"#);
        assert!(tc.parsed_args.is_none()); // Incomplete JSON
        tc.append(r#"th": "/src/main.rs"}"#);
        // After complete JSON, parsed_args should be populated
        assert!(tc.parsed_args.is_some());
        let result = tc.finalize().unwrap();
        assert_eq!(result["path"], "/src/main.rs");
        assert!(tc.complete);
    }

    #[test]
    fn test_partial_tool_call_complex_json() {
        let mut tc = PartialToolCall::new("tc2".to_string(), "write_file".to_string());
        tc.append(r#"{"path": "/test.rs", "#);
        tc.append(r#""content": "fn main() {}\n", "#);
        tc.append(r#""create_dirs": true}"#);
        let result = tc.finalize().unwrap();
        assert_eq!(result["path"], "/test.rs");
        assert_eq!(result["content"], "fn main() {}\n");
        assert_eq!(result["create_dirs"], true);
    }

    #[test]
    fn test_partial_tool_call_invalid_json() {
        let mut tc = PartialToolCall::new("tc1".to_string(), "read_file".to_string());
        tc.append(r#"{"broken"#);
        assert!(tc.finalize().is_err());
    }

    #[test]
    fn test_partial_tool_call_empty() {
        let mut tc = PartialToolCall::new("tc1".to_string(), "test".to_string());
        assert!(tc.finalize().is_err()); // Empty string isn't valid JSON
    }

    #[test]
    fn test_extract_partial_fields_complete() {
        let partial = r#"{"path": "/src/main.rs", "content": "hello"}"#;
        let fields = extract_partial_json_fields(partial);
        assert_eq!(fields.get("path").unwrap(), "/src/main.rs");
        assert_eq!(fields.get("content").unwrap(), "hello");
    }

    #[test]
    fn test_extract_partial_fields_truncated() {
        let partial = r#"{"path": "/src/main.rs", "line"#;
        let fields = extract_partial_json_fields(partial);
        assert_eq!(fields.get("path").unwrap(), "/src/main.rs");
        assert!(!fields.contains_key("line")); // No value yet
    }

    #[test]
    fn test_extract_partial_fields_empty() {
        let fields = extract_partial_json_fields("");
        assert!(fields.is_empty());
    }

    #[test]
    fn test_extract_partial_fields_no_strings() {
        let partial = r#"{"count": 42, "flag": true}"#;
        let fields = extract_partial_json_fields(partial);
        // Only extracts string values
        assert!(fields.is_empty());
    }

    #[test]
    fn test_partial_fields_method() {
        let mut tc = PartialToolCall::new("tc1".to_string(), "read_file".to_string());
        tc.append(r#"{"path": "/src/main.rs", "line"#);
        let fields = tc.partial_fields();
        assert_eq!(fields.get("path").unwrap(), "/src/main.rs");
    }

    #[test]
    fn test_tool_call_tracker_lifecycle() {
        let mut tracker = ToolCallTracker::new();
        assert!(tracker.is_empty());

        tracker.start_tool_call("t1".to_string(), "read_file".to_string());
        assert_eq!(tracker.len(), 1);
        assert!(!tracker.is_empty());

        tracker.append_delta("t1", r#"{"path":""#);
        tracker.append_delta("t1", r#"/main.rs"}"#);

        let result = tracker.finalize("t1").unwrap().unwrap();
        assert_eq!(result.0, "t1");
        assert_eq!(result.1, "read_file");
        assert_eq!(result.2["path"], "/main.rs");

        assert!(tracker.is_empty());
    }

    #[test]
    fn test_tool_call_tracker_unknown_id() {
        let mut tracker = ToolCallTracker::new();
        assert!(tracker.append_delta("unknown", "data").is_none());
        assert!(tracker.finalize("unknown").is_none());
        assert!(tracker.get("unknown").is_none());
    }

    #[test]
    fn test_tool_call_tracker_multiple() {
        let mut tracker = ToolCallTracker::new();
        tracker.start_tool_call("t1".to_string(), "read_file".to_string());
        tracker.start_tool_call("t2".to_string(), "write_file".to_string());
        assert_eq!(tracker.len(), 2);

        tracker.append_delta("t1", r#"{"path": "/a.rs"}"#);
        tracker.append_delta("t2", r#"{"path": "/b.rs", "content": "test"}"#);

        let r1 = tracker.finalize("t1").unwrap().unwrap();
        assert_eq!(r1.2["path"], "/a.rs");

        let r2 = tracker.finalize("t2").unwrap().unwrap();
        assert_eq!(r2.2["path"], "/b.rs");
    }

    #[test]
    fn test_tool_call_tracker_clear() {
        let mut tracker = ToolCallTracker::new();
        tracker.start_tool_call("t1".to_string(), "test".to_string());
        tracker.clear();
        assert!(tracker.is_empty());
    }
}
```

### Thinking parser tests
```rust
#[cfg(test)]
mod thinking_parser_tests {
    use super::thinking_parser::*;

    #[test]
    fn test_thinking_new() {
        let acc = ThinkingAccumulator::new();
        assert!(!acc.is_active());
        assert!(acc.is_empty());
        assert_eq!(acc.len(), 0);
    }

    #[test]
    fn test_thinking_accumulation() {
        let mut acc = ThinkingAccumulator::new();
        assert!(!acc.is_active());
        acc.append("Let me ");
        assert!(acc.is_active());
        assert!(!acc.is_empty());
        acc.append("think about this.");
        assert_eq!(acc.content(), "Let me think about this.");
        assert_eq!(acc.len(), 25);
    }

    #[test]
    fn test_thinking_finalize() {
        let mut acc = ThinkingAccumulator::new();
        acc.append("Reasoning here.");
        acc.set_signature("sig123".to_string());
        let block = acc.finalize();
        assert_eq!(block.content, "Reasoning here.");
        assert_eq!(block.signature, Some("sig123".to_string()));
        assert!(!block.redacted);
        assert!(!acc.is_active());
        // Content is moved out after finalize
        assert!(acc.content().is_empty());
    }

    #[test]
    fn test_thinking_redacted() {
        let mut acc = ThinkingAccumulator::new();
        acc.set_redacted();
        let block = acc.finalize();
        assert!(block.redacted);
        assert!(block.content.is_empty());
    }

    #[test]
    fn test_thinking_reset() {
        let mut acc = ThinkingAccumulator::new();
        acc.append("data");
        acc.set_signature("sig".to_string());
        acc.reset();
        assert!(!acc.is_active());
        assert!(acc.content().is_empty());
        assert!(acc.is_empty());
    }

    #[test]
    fn test_thinking_multiple_cycles() {
        let mut acc = ThinkingAccumulator::new();

        // First thinking block
        acc.append("First thought.");
        let block1 = acc.finalize();
        assert_eq!(block1.content, "First thought.");

        // Reset for next block
        acc.reset();
        assert!(!acc.is_active());

        // Second thinking block
        acc.append("Second thought.");
        let block2 = acc.finalize();
        assert_eq!(block2.content, "Second thought.");
    }

    #[test]
    fn test_thinking_finalize_empty() {
        let mut acc = ThinkingAccumulator::new();
        let block = acc.finalize();
        assert!(block.content.is_empty());
        assert!(block.signature.is_none());
        assert!(!block.redacted);
    }
}
```

### Chunk batcher tests
```rust
#[cfg(test)]
mod chunk_batcher_tests {
    use super::chunk_batcher::*;
    use std::time::Duration;

    #[test]
    fn test_batcher_accumulates() {
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
    fn test_batcher_empty_flush() {
        let mut batcher = ChunkBatcher::new(Duration::from_millis(16));
        assert!(batcher.flush().is_none());
        assert!(batcher.force_flush().is_none());
        assert!(!batcher.has_pending());
    }

    #[test]
    fn test_batcher_timing_blocks_flush() {
        let mut batcher = ChunkBatcher::new(Duration::from_secs(10)); // Very long interval
        batcher.push("text");
        // flush() should return None because interval hasn't elapsed
        assert!(batcher.flush().is_none());
        // But force_flush ignores timing
        assert_eq!(batcher.force_flush().unwrap(), "text");
    }

    #[test]
    fn test_batcher_zero_interval() {
        let mut batcher = ChunkBatcher::new(Duration::from_millis(0));
        batcher.push("text");
        // With 0ms interval, should_flush is always true when content exists
        assert!(batcher.should_flush());
        assert_eq!(batcher.flush().unwrap(), "text");
    }

    #[test]
    fn test_batcher_multiple_flushes() {
        let mut batcher = ChunkBatcher::new(Duration::from_millis(0));
        batcher.push("first");
        assert_eq!(batcher.force_flush().unwrap(), "first");
        batcher.push("second");
        assert_eq!(batcher.force_flush().unwrap(), "second");
        assert!(batcher.force_flush().is_none()); // Nothing left
    }
}
```

### StreamProcessor integration tests
```rust
#[cfg(test)]
mod stream_processor_tests {
    use super::*;
    use crate::provider::{StreamChunk, UsageInfo};

    #[test]
    fn test_process_text_done_sequence() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_millis(0));
        let events1 = proc.process(StreamChunk::Text {
            delta: "Hello ".to_string(),
        });
        // With 0ms interval, should flush immediately
        assert!(events1.iter().any(|e| matches!(e, ProcessedEvent::TextBatch(t) if t == "Hello ")));

        let events2 = proc.process(StreamChunk::Text {
            delta: "world".to_string(),
        });
        assert!(events2.iter().any(|e| matches!(e, ProcessedEvent::TextBatch(t) if t == "world")));

        let events3 = proc.process(StreamChunk::Done);
        assert!(events3.iter().any(|e| matches!(e, ProcessedEvent::Done)));
    }

    #[test]
    fn test_process_text_batching() {
        // With a long interval, text should be batched
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_secs(10));
        let events1 = proc.process(StreamChunk::Text {
            delta: "Hello ".to_string(),
        });
        assert!(events1.is_empty()); // Buffered, not flushed yet

        let events2 = proc.process(StreamChunk::Text {
            delta: "world".to_string(),
        });
        assert!(events2.is_empty()); // Still buffered

        // Done forces flush
        let events3 = proc.process(StreamChunk::Done);
        assert!(events3.iter().any(|e| matches!(e, ProcessedEvent::TextBatch(t) if t == "Hello world")));
        assert!(events3.iter().any(|e| matches!(e, ProcessedEvent::Done)));
    }

    #[test]
    fn test_process_thinking_then_text() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_millis(0));

        let events1 = proc.process(StreamChunk::Thinking {
            delta: "Let me think...".to_string(),
            signature: None,
            redacted: false,
        });
        assert!(events1
            .iter()
            .any(|e| matches!(e, ProcessedEvent::ThinkingDelta(d) if d == "Let me think...")));

        // When text starts, thinking should auto-finalize
        let events2 = proc.process(StreamChunk::Text {
            delta: "Answer: 42".to_string(),
        });
        assert!(events2
            .iter()
            .any(|e| matches!(e, ProcessedEvent::ThinkingComplete(b) if b.content == "Let me think...")));
        assert!(events2
            .iter()
            .any(|e| matches!(e, ProcessedEvent::TextBatch(_))));
    }

    #[test]
    fn test_process_thinking_with_signature() {
        let mut proc = StreamProcessor::new();

        proc.process(StreamChunk::Thinking {
            delta: "Reasoning...".to_string(),
            signature: None,
            redacted: false,
        });
        proc.process(StreamChunk::Thinking {
            delta: String::new(),
            signature: Some("sig-abc".to_string()),
            redacted: false,
        });

        let events = proc.finish();
        let thinking_complete = events
            .iter()
            .find(|e| matches!(e, ProcessedEvent::ThinkingComplete(_)));
        assert!(thinking_complete.is_some());
        if let ProcessedEvent::ThinkingComplete(block) = thinking_complete.unwrap() {
            assert_eq!(block.content, "Reasoning...");
            assert_eq!(block.signature, Some("sig-abc".to_string()));
        }
    }

    #[test]
    fn test_process_tool_call_delta_and_complete() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_millis(0));

        let events1 = proc.process(StreamChunk::ToolCallDelta {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            arguments_delta: r#"{"path":"#.to_string(),
        });
        assert!(events1
            .iter()
            .any(|e| matches!(e, ProcessedEvent::ToolCallPreview { .. })));

        let events2 = proc.process(StreamChunk::ToolCallComplete {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/src/main.rs"}),
        });
        assert!(events2
            .iter()
            .any(|e| matches!(e, ProcessedEvent::ToolCallReady { name, .. } if name == "read_file")));
    }

    #[test]
    fn test_process_tool_call_flushes_text() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_secs(10));

        // Buffer some text
        proc.process(StreamChunk::Text {
            delta: "Before tool".to_string(),
        });

        // Tool call should force-flush the text
        let events = proc.process(StreamChunk::ToolCallDelta {
            id: "t1".to_string(),
            name: "test".to_string(),
            arguments_delta: "{}".to_string(),
        });
        assert!(events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::TextBatch(t) if t == "Before tool")));
    }

    #[test]
    fn test_process_tool_call_finalizes_thinking() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_millis(0));

        proc.process(StreamChunk::Thinking {
            delta: "Thinking...".to_string(),
            signature: None,
            redacted: false,
        });

        let events = proc.process(StreamChunk::ToolCallDelta {
            id: "t1".to_string(),
            name: "test".to_string(),
            arguments_delta: "{}".to_string(),
        });
        assert!(events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::ThinkingComplete(_))));
    }

    #[test]
    fn test_process_usage() {
        let mut proc = StreamProcessor::new();
        let events = proc.process(StreamChunk::Usage(UsageInfo {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        }));
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ProcessedEvent::Usage(u) if u.input_tokens == 100));
    }

    #[test]
    fn test_process_error() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_secs(10));

        // Buffer some text
        proc.process(StreamChunk::Text {
            delta: "partial".to_string(),
        });

        // Error should flush text then emit error
        let events = proc.process(StreamChunk::Error("network error".to_string()));
        assert!(events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::TextBatch(t) if t == "partial")));
        assert!(events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::Error(e) if e == "network error")));
    }

    #[test]
    fn test_finish_flushes_remaining() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_secs(10));

        proc.process(StreamChunk::Text {
            delta: "buffered text".to_string(),
        });
        proc.process(StreamChunk::Thinking {
            delta: "buffered thinking".to_string(),
            signature: None,
            redacted: false,
        });

        // Note: thinking started after text was buffered, so text is still in batcher
        // and thinking is active
        let events = proc.finish();

        // Should have both text and thinking
        assert!(events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::TextBatch(_))));
        assert!(events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::ThinkingComplete(_))));
    }

    #[test]
    fn test_reset_clears_state() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_millis(0));

        proc.process(StreamChunk::Thinking {
            delta: "old data".to_string(),
            signature: None,
            redacted: false,
        });
        proc.process(StreamChunk::Text {
            delta: "old text".to_string(),
        });

        proc.reset();

        // After reset, processing text should NOT produce ThinkingComplete
        let events = proc.process(StreamChunk::Text {
            delta: "fresh".to_string(),
        });
        assert!(!events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::ThinkingComplete(_))));
    }

    #[test]
    fn test_full_conversation_flow() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_millis(0));
        let mut all_events = Vec::new();

        // Simulate a full response: thinking -> text -> tool call -> done
        all_events.extend(proc.process(StreamChunk::Thinking {
            delta: "I need to read the file.".to_string(),
            signature: None,
            redacted: false,
        }));
        all_events.extend(proc.process(StreamChunk::Thinking {
            delta: String::new(),
            signature: Some("thinking-sig".to_string()),
            redacted: false,
        }));
        all_events.extend(proc.process(StreamChunk::Text {
            delta: "Let me read that file for you.".to_string(),
        }));
        all_events.extend(proc.process(StreamChunk::ToolCallDelta {
            id: "tc-1".to_string(),
            name: "read_file".to_string(),
            arguments_delta: r#"{"path": "/src/main.rs"}"#.to_string(),
        }));
        all_events.extend(proc.process(StreamChunk::ToolCallComplete {
            id: "tc-1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/src/main.rs"}),
        }));
        all_events.extend(proc.process(StreamChunk::Usage(UsageInfo {
            input_tokens: 100,
            output_tokens: 200,
            ..Default::default()
        })));
        all_events.extend(proc.process(StreamChunk::Done));

        // Verify event sequence
        assert!(all_events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::ThinkingDelta(_))));
        assert!(all_events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::ThinkingComplete(b) if b.signature == Some("thinking-sig".to_string()))));
        assert!(all_events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::TextBatch(_))));
        assert!(all_events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::ToolCallPreview { .. })));
        assert!(all_events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::ToolCallReady { .. })));
        assert!(all_events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::Usage(_))));
        assert!(all_events
            .iter()
            .any(|e| matches!(e, ProcessedEvent::Done)));
    }
}
```

## Acceptance Criteria
- [x] `ToolCallTracker` correctly accumulates partial JSON fragments and parses on finalize
- [x] `PartialToolCall::partial_fields()` extracts key-value pairs from incomplete JSON via regex
- [x] `ThinkingAccumulator` tracks content, signatures, and redacted state across append/finalize/reset
- [x] `ChunkBatcher` debounces text updates within configurable time interval
- [x] `ChunkBatcher::force_flush()` always returns content regardless of timing
- [x] `StreamProcessor` produces correct event sequence for: text-only, thinking-then-text, tool calls, errors
- [x] Thinking auto-finalizes when text or tool call starts
- [x] Text buffer auto-flushes when tool call starts
- [x] `StreamProcessor::finish()` returns all remaining buffered events
- [x] `StreamProcessor::reset()` clears all state for reuse
- [x] Multiple tool calls can be tracked simultaneously
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes all unit and integration tests
- [x] No unnecessary allocations in the hot path beyond string accumulation

**Completed**: PR #3
