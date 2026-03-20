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
//!         ├── ThinkingParser
//!         │     └── accumulates thinking chunks, tracks signatures
//!         │
//!         ├── ToolParser
//!         │     └── incremental JSON parsing for tool call arguments,
//!         │         emits ToolCallComplete when closing brace received
//!         │
//!         └── ChunkBatcher
//!               └── debounces rapid text deltas into 16ms windows
//!                   to prevent rendering every single token
//!         │
//!         ▼
//!   Structured events ──► Agent / Controller

pub mod chunk_batcher;
pub mod thinking_parser;
pub mod tool_parser;
