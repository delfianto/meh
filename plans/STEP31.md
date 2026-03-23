# STEP 31 — Context Window Management

## Objective
Implement context window tracking and automatic conversation summarization/truncation when approaching the model's context limit. This prevents API errors from oversized requests and maintains conversation quality. Cline calls this "condensation."

## Prerequisites
- STEP 25 (token counting)
- STEP 07 (agent loop)

## Detailed Instructions

### 31.1 ContextManager (`src/context/mod.rs`)

```rust
//! Context window management — tracks token budget and triggers summarization.

pub mod summarizer;
pub mod truncation;

use crate::provider::ModelInfo;
use crate::util::tokens;

pub struct ContextManager {
    /// Model's total context window (tokens).
    context_window: u32,
    /// Reserve this many tokens for the model's response.
    response_reserve: u32,
    /// Reserve for system prompt + tools (calculated once).
    system_reserve: u32,
    /// Threshold percentage to trigger summarization (e.g., 0.85 = 85%).
    summarize_threshold: f64,
    /// Current estimated token usage.
    current_usage: u64,
    /// Range of messages that have been deleted/summarized.
    deleted_range: Option<(usize, usize)>,
}

impl ContextManager {
    pub fn new(model_info: &ModelInfo) -> Self {
        Self {
            context_window: model_info.context_window,
            response_reserve: model_info.max_tokens,
            system_reserve: 0,
            summarize_threshold: 0.85,
            current_usage: 0,
            deleted_range: None,
        }
    }

    /// Set the system prompt token count (call once after building prompt).
    pub fn set_system_reserve(&mut self, system_tokens: u32) {
        self.system_reserve = system_tokens;
    }

    /// Available tokens for conversation messages.
    pub fn available_tokens(&self) -> u64 {
        let total = self.context_window as u64;
        let reserved = (self.response_reserve + self.system_reserve) as u64;
        total.saturating_sub(reserved)
    }

    /// Update current usage estimate.
    pub fn update_usage(&mut self, tokens: u64) {
        self.current_usage = tokens;
    }

    /// Check if summarization should be triggered.
    pub fn needs_summarization(&self) -> bool {
        let available = self.available_tokens();
        if available == 0 { return true; }
        (self.current_usage as f64 / available as f64) >= self.summarize_threshold
    }

    /// Calculate how many tokens to free (target: reduce to 60% of available).
    pub fn tokens_to_free(&self) -> u64 {
        let target = (self.available_tokens() as f64 * 0.60) as u64;
        self.current_usage.saturating_sub(target)
    }

    /// Record that messages in range [start, end) were deleted.
    pub fn record_deletion(&mut self, start: usize, end: usize) {
        self.deleted_range = Some((start, end));
    }

    /// Get the deleted range for persistence.
    pub fn deleted_range(&self) -> Option<(usize, usize)> {
        self.deleted_range
    }

    /// Context utilization as percentage.
    pub fn utilization_percent(&self) -> f64 {
        let available = self.available_tokens();
        if available == 0 { return 100.0; }
        (self.current_usage as f64 / available as f64) * 100.0
    }
}
```

### 31.2 Conversation Summarizer (`src/context/summarizer.rs`)

```rust
//! Summarize older conversation messages to free context space.

use crate::provider::{Message, ContentBlock, MessageRole, Provider, ModelConfig, ProviderStream};

/// Strategy for summarizing conversation.
pub enum SummarizationStrategy {
    /// Ask the LLM to summarize the conversation so far.
    LlmSummary,
    /// Simply truncate older messages (keep last N).
    Truncate { keep_last: usize },
}

/// Summarize a conversation to free tokens.
pub async fn summarize_conversation(
    provider: &dyn Provider,
    messages: &[Message],
    tokens_to_free: u64,
    model_config: &ModelConfig,
) -> anyhow::Result<SummarizationResult> {
    // 1. Calculate how many messages from the start need summarizing
    //    (accumulate token counts from oldest until tokens_to_free reached)
    // 2. Build a summarization prompt:
    //    "Summarize the following conversation concisely, preserving:
    //     - Key decisions made
    //     - Files modified and why
    //     - Current task state and progress
    //     - Any unresolved issues"
    // 3. Call provider with summarization prompt + messages to summarize
    // 4. Return summary text + range of messages to remove
}

pub struct SummarizationResult {
    /// Summary text to prepend as a system/user message.
    pub summary: String,
    /// Range of original messages to remove [start, end).
    pub remove_range: (usize, usize),
    /// Tokens freed by removing those messages.
    pub tokens_freed: u64,
}

/// Apply summarization result to message history.
pub fn apply_summarization(
    messages: &mut Vec<Message>,
    result: &SummarizationResult,
) {
    // 1. Remove messages in remove_range
    // 2. Insert summary as first user message:
    //    "[Previous conversation summary]\n{summary}"
    // 3. Ensure message alternation is preserved (user/assistant/user/...)
}
```

### 31.3 Truncation strategies (`src/context/truncation.rs`)

```rust
/// Simple truncation — remove oldest messages, keep last N.
pub fn truncate_keep_last(messages: &mut Vec<Message>, keep: usize) -> usize {
    if messages.len() <= keep { return 0; }
    let removed = messages.len() - keep;
    messages.drain(0..removed);
    removed
}

/// Smart truncation — remove tool results first (they're large), then old messages.
pub fn truncate_smart(messages: &mut Vec<Message>, tokens_to_free: u64, model: &str) -> u64 {
    // 1. First pass: replace large tool results with "[truncated - N tokens]"
    // 2. If not enough: remove oldest message pairs (user+assistant)
    // 3. Never remove the initial user message (task description)
    // 4. Return actual tokens freed
}
```

### 31.4 Integration with Agent loop

In `TaskAgent::run()`, before each API call:
```rust
// Estimate tokens for current context
let usage = tokens::estimate_conversation_tokens(&self.system_prompt, &self.messages, &self.config.model_id);
self.context_manager.update_usage(usage as u64);

if self.context_manager.needs_summarization() {
    tracing::info!(utilization = %self.context_manager.utilization_percent(), "Context window nearing limit, summarizing");
    let result = summarize_conversation(
        self.provider.as_ref(), &self.messages,
        self.context_manager.tokens_to_free(), &self.config,
    ).await?;
    apply_summarization(&mut self.messages, &result);
    self.context_manager.record_deletion(result.remove_range.0, result.remove_range.1);
    // Notify TUI
    let _ = self.ctrl_tx.send(ControllerMessage::StreamChunk(
        StreamChunk::Text { delta: "\n[Conversation summarized to free context space]\n".to_string() }
    ));
}
```

## Tests

```rust
#[cfg(test)]
mod context_manager_tests {
    use super::*;

    #[test]
    fn test_available_tokens() {
        let mut cm = ContextManager::new(&ModelInfo {
            context_window: 200_000, max_tokens: 8192, ..Default::default()
        });
        cm.set_system_reserve(5000);
        assert_eq!(cm.available_tokens(), 200_000 - 8192 - 5000);
    }

    #[test]
    fn test_needs_summarization_under_threshold() {
        let mut cm = ContextManager::new(&ModelInfo {
            context_window: 200_000, max_tokens: 8192, ..Default::default()
        });
        cm.update_usage(50_000);
        assert!(!cm.needs_summarization());
    }

    #[test]
    fn test_needs_summarization_over_threshold() {
        let mut cm = ContextManager::new(&ModelInfo {
            context_window: 200_000, max_tokens: 8192, ..Default::default()
        });
        cm.update_usage(170_000);
        assert!(cm.needs_summarization());
    }

    #[test]
    fn test_tokens_to_free() {
        let mut cm = ContextManager::new(&ModelInfo {
            context_window: 100_000, max_tokens: 5000, ..Default::default()
        });
        cm.update_usage(80_000); // 80k used of ~95k available
        let to_free = cm.tokens_to_free();
        assert!(to_free > 0);
    }

    #[test]
    fn test_utilization_percent() {
        let mut cm = ContextManager::new(&ModelInfo {
            context_window: 100_000, max_tokens: 0, ..Default::default()
        });
        cm.update_usage(50_000);
        assert!((cm.utilization_percent() - 50.0).abs() < 0.1);
    }
}

#[cfg(test)]
mod truncation_tests {
    use super::truncation::*;

    #[test]
    fn test_truncate_keep_last() {
        let mut msgs = vec![make_msg("a"), make_msg("b"), make_msg("c"), make_msg("d")];
        let removed = truncate_keep_last(&mut msgs, 2);
        assert_eq!(removed, 2);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn test_truncate_keep_last_no_op() {
        let mut msgs = vec![make_msg("a")];
        let removed = truncate_keep_last(&mut msgs, 5);
        assert_eq!(removed, 0);
    }
}
```

## Acceptance Criteria
- [x] ContextManager tracks token budget vs model context window
- [x] Summarization triggers at 85% utilization
- [ ] LLM-based summarization preserves key decisions and file changes
- [x] Truncation frees tool results first, then oldest messages
- [x] Initial user message (task description) never removed
- [x] Summary injected as first message after deletion
- [x] Deleted range tracked for persistence
- [ ] TUI notified when summarization occurs
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass
