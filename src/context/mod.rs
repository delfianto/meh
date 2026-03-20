//! Context window management — tracks token budget and triggers summarization.
//!
//! LLMs have finite context windows. As conversations grow, the message
//! history must be managed to stay within budget. This module provides
//! two strategies that work together:
//!
//! ```text
//!   Messages[]  ──►  ContextManager  ──►  Messages[] (trimmed)
//!                         │
//!                    ┌────┴────┐
//!                    │         │
//!                truncation  summarizer
//!                    │         │
//!                 drop old   LLM call to
//!                 messages   condense history
//! ```
//!
//! **Truncation** drops the oldest messages (preserving the system prompt
//! and the most recent turns) when the context approaches the model's
//! token limit.
//!
//! **Summarization** uses a separate LLM call to condense the conversation
//! history into a compact summary, replacing many messages with a single
//! summary message. This preserves more semantic content than raw
//! truncation at the cost of an extra API call.

pub mod summarizer;
pub mod truncation;
