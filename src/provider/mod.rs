//! LLM provider abstraction and implementations.
//!
//! All providers implement a common `Provider` trait that returns a
//! `Stream<Item = Result<StreamChunk>>`. This normalizes the differences
//! between provider APIs behind a uniform interface.
//!
//! ```text
//!   Agent
//!     │
//!     ▼
//!   dyn Provider::create_message()
//!     │
//!     ├── AnthropicProvider  ──► Messages API (SSE)
//!     ├── OpenAiProvider     ──► Chat Completions / Responses API
//!     ├── GeminiProvider     ──► streamGenerateContent (SSE)
//!     └── OpenRouterProvider ──► OpenAI-compatible API + extras
//!     │
//!     ▼
//!   Stream<Item = Result<StreamChunk>>
//!     │
//!     ├── StreamChunk::Text { delta }
//!     ├── StreamChunk::Thinking { delta, signature }
//!     ├── StreamChunk::ToolCall { id, name, arguments_delta }
//!     ├── StreamChunk::Usage { input_tokens, output_tokens, ... }
//!     └── StreamChunk::Done
//! ```
//!
//! The `common` module provides shared HTTP client setup, retry logic
//! with exponential backoff, and error types used across all providers.

pub mod anthropic;
pub mod common;
pub mod gemini;
pub mod openai;
pub mod openrouter;
