# STEP 25 — Token Counting and Display

## Objective
Implement accurate token counting for messages using tiktoken-rs, display real-time token usage in the TUI status bar, and track context window utilization.

## Prerequisites
- STEP 03, 07 complete (TUI + agent wiring)

## Detailed Instructions

### 25.1 Token counting utility (`src/util/tokens.rs`)

```rust
//! Token counting utilities.

use tiktoken_rs::{get_bpe_from_model, CoreBPE};
use std::sync::OnceLock;

/// Get a token encoder for a given model.
/// Falls back to cl100k_base if model not recognized.
fn get_encoder(model: &str) -> &'static CoreBPE {
    static ENCODER: OnceLock<CoreBPE> = OnceLock::new();
    ENCODER.get_or_init(|| {
        get_bpe_from_model(model)
            .unwrap_or_else(|_| tiktoken_rs::cl100k_base().expect("Failed to load tokenizer"))
    })
}

/// Count tokens in a string.
pub fn count_tokens(text: &str, model: &str) -> usize {
    let encoder = get_encoder(model);
    encoder.encode_ordinary(text).len()
}

/// Estimate tokens for a message (text + overhead for role/formatting).
pub fn estimate_message_tokens(role: &str, content: &str, model: &str) -> usize {
    // Each message has ~4 tokens of overhead (role markers, separators)
    let overhead = 4;
    count_tokens(content, model) + overhead
}

/// Estimate total tokens for a conversation.
pub fn estimate_conversation_tokens(
    system_prompt: &str,
    messages: &[crate::provider::Message],
    model: &str,
) -> usize {
    let mut total = count_tokens(system_prompt, model) + 4; // system prompt + overhead
    for msg in messages {
        let role = match msg.role {
            crate::provider::MessageRole::User => "user",
            crate::provider::MessageRole::Assistant => "assistant",
        };
        for block in &msg.content {
            match block {
                crate::provider::ContentBlock::Text(text) => {
                    total += estimate_message_tokens(role, text, model);
                }
                crate::provider::ContentBlock::Thinking { text, .. } => {
                    total += count_tokens(text, model);
                }
                crate::provider::ContentBlock::ToolUse { input, .. } => {
                    total += count_tokens(&serde_json::to_string(input).unwrap_or_default(), model) + 10;
                }
                crate::provider::ContentBlock::ToolResult { content, .. } => {
                    total += estimate_message_tokens("tool", content, model);
                }
                _ => {}
            }
        }
    }
    total
}

/// Format a token count for display.
pub fn format_tokens(count: u64) -> String {
    if count < 1_000 {
        format!("{count}")
    } else if count < 1_000_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    }
}

/// Calculate context window utilization percentage.
pub fn context_utilization(used_tokens: u64, context_window: u32) -> f64 {
    if context_window == 0 { return 0.0; }
    (used_tokens as f64 / context_window as f64) * 100.0
}
```

### 25.2 Track tokens in TaskState

Update `TaskState` to track per-call and cumulative tokens:
```rust
pub struct TaskState {
    // ... existing fields ...
    pub context_tokens: u64,          // Current context window usage (estimated)
    pub context_window: u32,          // Model's context window size
    pub last_input_tokens: u64,       // Input tokens from last API call
    pub last_output_tokens: u64,      // Output tokens from last API call
}
```

### 25.3 Display in status bar

Update `StatusBarState` and rendering:
```
[ACT] anthropic/claude-sonnet-4  ·  ctx: 12.4k/200k (6%)  ·  total: 45.2k  ·  $0.135
```

- `ctx`: current context window utilization (input tokens for next call)
- `total`: cumulative tokens across all API calls
- Cost: cumulative cost

### 25.4 Wire token updates

When `StreamChunk::Usage` is received in the controller:
```rust
StreamChunk::Usage(usage) => {
    self.task_state.record_usage(usage.input_tokens, usage.output_tokens, usage.total_cost.unwrap_or(0.0));
    self.task_state.last_input_tokens = usage.input_tokens;
    self.task_state.last_output_tokens = usage.output_tokens;

    let _ = self.ui_tx.send(UiUpdate::StatusUpdate {
        tokens: Some(self.task_state.total_input_tokens + self.task_state.total_output_tokens),
        cost: Some(self.task_state.total_cost),
        ..Default::default()
    });
}
```

## Tests

```rust
#[cfg(test)]
mod token_tests {
    use super::*;

    #[test]
    fn test_count_tokens_basic() {
        let count = count_tokens("Hello, world!", "gpt-4");
        assert!(count > 0);
        assert!(count < 10); // Should be ~4 tokens
    }

    #[test]
    fn test_count_tokens_empty() {
        assert_eq!(count_tokens("", "gpt-4"), 0);
    }

    #[test]
    fn test_estimate_message_tokens() {
        let tokens = estimate_message_tokens("user", "Hello", "gpt-4");
        assert!(tokens > count_tokens("Hello", "gpt-4")); // Should include overhead
    }

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(1_500_000), "1.5M");
    }

    #[test]
    fn test_context_utilization() {
        assert!((context_utilization(10_000, 200_000) - 5.0).abs() < 0.01);
        assert!((context_utilization(100_000, 200_000) - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_conversation_token_estimate() {
        let messages = vec![
            crate::provider::Message {
                role: crate::provider::MessageRole::User,
                content: vec![crate::provider::ContentBlock::Text("Hello".to_string())],
            },
        ];
        let total = estimate_conversation_tokens("You are helpful.", &messages, "gpt-4");
        assert!(total > 0);
    }
}
```

## Acceptance Criteria
- [ ] Token counting uses tiktoken-rs for accurate counts
- [ ] Fallback encoder for unknown models
- [ ] Context window utilization displayed as percentage
- [ ] Token counts formatted readably (k, M suffixes)
- [ ] Status bar shows real-time token usage
- [ ] Total tokens and cost tracked across API calls
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass
