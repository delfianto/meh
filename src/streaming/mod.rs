//! Stream processing — sits between the provider stream and the agent.
//!
//! Raw `StreamChunk`s from the provider are processed through a pipeline
//! that parses tool calls, accumulates thinking blocks, and batches
//! rapid updates to prevent TUI flicker.
//!
//! ```text
//!   Provider stream
//!         │
//!         ▼
//!   StreamProcessor
//!         │
//!         ├── ThinkingAccumulator
//!         │     └── accumulates thinking chunks, tracks signatures
//!         │
//!         ├── ToolCallTracker
//!         │     └── incremental JSON parsing for tool call arguments,
//!         │         emits ToolCallReady when finalized
//!         │
//!         └── ChunkBatcher
//!               └── debounces rapid text deltas into configurable windows
//!                   to prevent rendering every single token
//!         │
//!         ▼
//!   Vec<ProcessedEvent> ──► Agent / Controller
//! ```

pub mod chunk_batcher;
pub mod thinking_parser;
pub mod tool_parser;

use crate::provider::StreamChunk;
use chunk_batcher::ChunkBatcher;
use std::time::Duration;
use thinking_parser::ThinkingAccumulator;
use tool_parser::ToolCallTracker;

/// Events emitted by the [`StreamProcessor`] (higher-level than raw `StreamChunk`s).
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

/// Processes raw [`StreamChunk`]s into higher-level [`ProcessedEvent`]s.
///
/// Handles state transitions: thinking auto-finalizes when text or tool
/// calls start, and text buffers auto-flush when tool calls start.
pub struct StreamProcessor {
    tool_tracker: ToolCallTracker,
    thinking: ThinkingAccumulator,
    text_batcher: ChunkBatcher,
}

impl StreamProcessor {
    /// Creates a new processor with the default 16ms flush interval.
    pub fn new() -> Self {
        Self {
            tool_tracker: ToolCallTracker::new(),
            thinking: ThinkingAccumulator::new(),
            text_batcher: ChunkBatcher::new(Duration::from_millis(16)),
        }
    }

    /// Creates a processor with a custom flush interval for the text batcher.
    pub fn with_flush_interval(flush_interval: Duration) -> Self {
        Self {
            tool_tracker: ToolCallTracker::new(),
            thinking: ThinkingAccumulator::new(),
            text_batcher: ChunkBatcher::new(flush_interval),
        }
    }

    /// Processes a single `StreamChunk` and returns zero or more `ProcessedEvent`s.
    pub fn process(&mut self, chunk: StreamChunk) -> Vec<ProcessedEvent> {
        let mut events = Vec::new();

        match chunk {
            StreamChunk::Text { delta } => {
                self.handle_text(&delta, &mut events);
            }
            StreamChunk::Thinking {
                delta,
                signature,
                redacted,
            } => {
                self.handle_thinking(&delta, signature, redacted, &mut events);
            }
            StreamChunk::ToolCallDelta {
                id,
                name,
                arguments_delta,
            } => {
                self.handle_tool_delta(id, name, arguments_delta, &mut events);
            }
            StreamChunk::ToolCallComplete {
                id,
                name,
                arguments,
            } => {
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
                self.flush_remaining(&mut events);
                events.push(ProcessedEvent::Done);
            }
            StreamChunk::Error(e) => {
                if let Some(batch) = self.text_batcher.force_flush() {
                    events.push(ProcessedEvent::TextBatch(batch));
                }
                events.push(ProcessedEvent::Error(e));
            }
        }

        events
    }

    /// Returns any remaining buffered events (call at end of stream).
    pub fn finish(&mut self) -> Vec<ProcessedEvent> {
        let mut events = Vec::new();
        self.flush_remaining(&mut events);
        events
    }

    /// Resets all internal state for a new stream.
    pub fn reset(&mut self) {
        self.tool_tracker.clear();
        self.thinking.reset();
        let _ = self.text_batcher.force_flush();
    }

    /// Handles a text delta: auto-finalizes thinking, pushes to batcher, flushes if ready.
    fn handle_text(&mut self, delta: &str, events: &mut Vec<ProcessedEvent>) {
        if self.thinking.is_active() {
            events.push(ProcessedEvent::ThinkingComplete(self.thinking.finalize()));
        }
        self.text_batcher.push(delta);
        if self.text_batcher.should_flush() {
            if let Some(batch) = self.text_batcher.flush() {
                events.push(ProcessedEvent::TextBatch(batch));
            }
        }
    }

    /// Handles a thinking delta: tracks content, signature, and redacted state.
    fn handle_thinking(
        &mut self,
        delta: &str,
        signature: Option<String>,
        redacted: bool,
        events: &mut Vec<ProcessedEvent>,
    ) {
        if redacted {
            self.thinking.set_redacted();
        }
        if let Some(sig) = signature {
            self.thinking.set_signature(sig);
        }
        if !delta.is_empty() {
            self.thinking.append(delta);
            events.push(ProcessedEvent::ThinkingDelta(delta.to_string()));
        }
    }

    /// Handles a tool call delta: flushes text, finalizes thinking, tracks tool call.
    fn handle_tool_delta(
        &mut self,
        id: String,
        name: String,
        arguments_delta: String,
        events: &mut Vec<ProcessedEvent>,
    ) {
        if let Some(batch) = self.text_batcher.force_flush() {
            events.push(ProcessedEvent::TextBatch(batch));
        }
        if self.thinking.is_active() {
            events.push(ProcessedEvent::ThinkingComplete(self.thinking.finalize()));
        }

        if self.tool_tracker.get(&id).is_none() {
            self.tool_tracker.start_tool_call(id.clone(), name.clone());
        }
        self.tool_tracker.append_delta(&id, &arguments_delta);

        events.push(ProcessedEvent::ToolCallPreview {
            id,
            name,
            partial_args: arguments_delta,
        });
    }

    /// Flushes any remaining buffered text and active thinking.
    fn flush_remaining(&mut self, events: &mut Vec<ProcessedEvent>) {
        if let Some(batch) = self.text_batcher.force_flush() {
            events.push(ProcessedEvent::TextBatch(batch));
        }
        if self.thinking.is_active() {
            events.push(ProcessedEvent::ThinkingComplete(self.thinking.finalize()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{StreamChunk, UsageInfo};

    #[test]
    fn process_text_done_sequence() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_millis(0));
        let events1 = proc.process(StreamChunk::Text {
            delta: "Hello ".to_string(),
        });
        assert!(
            events1
                .iter()
                .any(|e| matches!(e, ProcessedEvent::TextBatch(t) if t == "Hello "))
        );

        let events2 = proc.process(StreamChunk::Text {
            delta: "world".to_string(),
        });
        assert!(
            events2
                .iter()
                .any(|e| matches!(e, ProcessedEvent::TextBatch(t) if t == "world"))
        );

        let events3 = proc.process(StreamChunk::Done);
        assert!(events3.iter().any(|e| matches!(e, ProcessedEvent::Done)));
    }

    #[test]
    fn process_text_batching() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_secs(10));
        let events1 = proc.process(StreamChunk::Text {
            delta: "Hello ".to_string(),
        });
        assert!(events1.is_empty());

        let events2 = proc.process(StreamChunk::Text {
            delta: "world".to_string(),
        });
        assert!(events2.is_empty());

        let events3 = proc.process(StreamChunk::Done);
        assert!(
            events3
                .iter()
                .any(|e| matches!(e, ProcessedEvent::TextBatch(t) if t == "Hello world"))
        );
        assert!(events3.iter().any(|e| matches!(e, ProcessedEvent::Done)));
    }

    #[test]
    fn process_thinking_then_text() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_millis(0));

        let events1 = proc.process(StreamChunk::Thinking {
            delta: "Let me think...".to_string(),
            signature: None,
            redacted: false,
        });
        assert!(
            events1
                .iter()
                .any(|e| matches!(e, ProcessedEvent::ThinkingDelta(d) if d == "Let me think..."))
        );

        let events2 = proc.process(StreamChunk::Text {
            delta: "Answer: 42".to_string(),
        });
        assert!(events2.iter().any(
            |e| matches!(e, ProcessedEvent::ThinkingComplete(b) if b.content == "Let me think...")
        ));
        assert!(
            events2
                .iter()
                .any(|e| matches!(e, ProcessedEvent::TextBatch(_)))
        );
    }

    #[test]
    fn process_thinking_with_signature() {
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
    fn process_tool_call_delta_and_complete() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_millis(0));

        let events1 = proc.process(StreamChunk::ToolCallDelta {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            arguments_delta: r#"{"path":"#.to_string(),
        });
        assert!(
            events1
                .iter()
                .any(|e| matches!(e, ProcessedEvent::ToolCallPreview { .. }))
        );

        let events2 = proc.process(StreamChunk::ToolCallComplete {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/src/main.rs"}),
        });
        assert!(events2.iter().any(
            |e| matches!(e, ProcessedEvent::ToolCallReady { name, .. } if name == "read_file")
        ));
    }

    #[test]
    fn process_tool_call_flushes_text() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_secs(10));

        proc.process(StreamChunk::Text {
            delta: "Before tool".to_string(),
        });

        let events = proc.process(StreamChunk::ToolCallDelta {
            id: "t1".to_string(),
            name: "test".to_string(),
            arguments_delta: "{}".to_string(),
        });
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProcessedEvent::TextBatch(t) if t == "Before tool"))
        );
    }

    #[test]
    fn process_tool_call_finalizes_thinking() {
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
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProcessedEvent::ThinkingComplete(_)))
        );
    }

    #[test]
    fn process_usage() {
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
    fn process_error() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_secs(10));

        proc.process(StreamChunk::Text {
            delta: "partial".to_string(),
        });

        let events = proc.process(StreamChunk::Error("network error".to_string()));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProcessedEvent::TextBatch(t) if t == "partial"))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProcessedEvent::Error(e) if e == "network error"))
        );
    }

    #[test]
    fn finish_flushes_remaining() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_secs(10));

        proc.process(StreamChunk::Text {
            delta: "buffered text".to_string(),
        });
        proc.process(StreamChunk::Thinking {
            delta: "buffered thinking".to_string(),
            signature: None,
            redacted: false,
        });

        let events = proc.finish();

        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProcessedEvent::TextBatch(_)))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProcessedEvent::ThinkingComplete(_)))
        );
    }

    #[test]
    fn reset_clears_state() {
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

        let events = proc.process(StreamChunk::Text {
            delta: "fresh".to_string(),
        });
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ProcessedEvent::ThinkingComplete(_)))
        );
    }

    #[test]
    fn full_conversation_flow() {
        let mut proc = StreamProcessor::with_flush_interval(Duration::from_millis(0));
        let mut all_events = Vec::new();

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

        assert!(
            all_events
                .iter()
                .any(|e| matches!(e, ProcessedEvent::ThinkingDelta(_)))
        );
        assert!(all_events.iter().any(|e| matches!(
            e,
            ProcessedEvent::ThinkingComplete(b) if b.signature == Some("thinking-sig".to_string())
        )));
        assert!(
            all_events
                .iter()
                .any(|e| matches!(e, ProcessedEvent::TextBatch(_)))
        );
        assert!(
            all_events
                .iter()
                .any(|e| matches!(e, ProcessedEvent::ToolCallPreview { .. }))
        );
        assert!(
            all_events
                .iter()
                .any(|e| matches!(e, ProcessedEvent::ToolCallReady { .. }))
        );
        assert!(
            all_events
                .iter()
                .any(|e| matches!(e, ProcessedEvent::Usage(_)))
        );
        assert!(all_events.iter().any(|e| matches!(e, ProcessedEvent::Done)));
    }
}
