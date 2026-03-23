//! Token counting utilities using tiktoken-rs.
//!
//! Provides accurate token counting for messages, conversations, and
//! arbitrary text. Falls back to `cl100k_base` for unknown models.
//! Includes formatting helpers for human-readable display.

use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

/// Returns a BPE encoder, lazily initialized.
///
/// Uses `cl100k_base` which covers GPT-4, Claude, and most modern models.
/// The encoder is shared across all callers for the lifetime of the process.
fn get_encoder() -> &'static CoreBPE {
    static ENCODER: OnceLock<CoreBPE> = OnceLock::new();
    ENCODER.get_or_init(|| {
        tiktoken_rs::cl100k_base().unwrap_or_else(|e| {
            tracing::error!(error = %e, "Failed to load cl100k_base tokenizer");
            panic!("Tokenizer initialization failed: {e}");
        })
    })
}

/// Count tokens in a string.
pub fn count_tokens(text: &str) -> usize {
    get_encoder().encode_ordinary(text).len()
}

/// Estimate tokens for a single message (content + role/formatting overhead).
pub fn estimate_message_tokens(content: &str) -> usize {
    let overhead = 4;
    count_tokens(content) + overhead
}

/// Estimate total tokens for a full conversation.
#[allow(clippy::cast_precision_loss)]
pub fn estimate_conversation_tokens(
    system_prompt: &str,
    messages: &[crate::provider::Message],
) -> usize {
    let mut total = count_tokens(system_prompt) + 4;
    for msg in messages {
        for block in &msg.content {
            match block {
                crate::provider::ContentBlock::Text(text) => {
                    total += estimate_message_tokens(text);
                }
                crate::provider::ContentBlock::Thinking { text, .. } => {
                    total += count_tokens(text);
                }
                crate::provider::ContentBlock::ToolUse { input, .. } => {
                    total += count_tokens(&serde_json::to_string(input).unwrap_or_default()) + 10;
                }
                crate::provider::ContentBlock::ToolResult { content, .. } => {
                    total += estimate_message_tokens(content);
                }
                crate::provider::ContentBlock::Image { .. } => {
                    total += 85;
                }
            }
        }
    }
    total
}

/// Format a token count for human-readable display.
#[allow(clippy::cast_precision_loss)]
pub fn format_tokens(count: u64) -> String {
    if count < 1_000 {
        format!("{count}")
    } else if count < 1_000_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    }
}

/// Calculate context window utilization as a percentage.
#[allow(clippy::cast_precision_loss)]
pub fn context_utilization(used_tokens: u64, context_window: u32) -> f64 {
    if context_window == 0 {
        return 0.0;
    }
    (used_tokens as f64 / f64::from(context_window)) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_tokens_basic() {
        let count = count_tokens("Hello, world!");
        assert!(count > 0);
        assert!(count < 10);
    }

    #[test]
    fn count_tokens_empty() {
        assert_eq!(count_tokens(""), 0);
    }

    #[test]
    fn count_tokens_longer_text() {
        let short = count_tokens("Hi");
        let long = count_tokens("This is a longer sentence with more tokens in it.");
        assert!(long > short);
    }

    #[test]
    fn estimate_message_includes_overhead() {
        let raw = count_tokens("Hello");
        let estimated = estimate_message_tokens("Hello");
        assert_eq!(estimated, raw + 4);
    }

    #[test]
    fn format_tokens_units() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1.0k");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(45_200), "45.2k");
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(1_500_000), "1.5M");
    }

    #[test]
    fn context_utilization_basic() {
        let pct = context_utilization(10_000, 200_000);
        assert!((pct - 5.0).abs() < 0.01);
    }

    #[test]
    fn context_utilization_half() {
        let pct = context_utilization(100_000, 200_000);
        assert!((pct - 50.0).abs() < 0.01);
    }

    #[test]
    fn context_utilization_zero_window() {
        assert!((context_utilization(1000, 0)).abs() < f64::EPSILON);
    }

    #[test]
    fn estimate_conversation_tokens_basic() {
        let messages = vec![crate::provider::Message {
            role: crate::provider::MessageRole::User,
            content: vec![crate::provider::ContentBlock::Text("Hello".to_string())],
        }];
        let total = estimate_conversation_tokens("You are helpful.", &messages);
        assert!(total > 0);
    }

    #[test]
    fn estimate_conversation_tokens_with_tool_use() {
        let messages = vec![crate::provider::Message {
            role: crate::provider::MessageRole::Assistant,
            content: vec![crate::provider::ContentBlock::ToolUse {
                id: "tc1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "/src/main.rs"}),
            }],
        }];
        let total = estimate_conversation_tokens("system", &messages);
        assert!(total > 10);
    }

    #[test]
    fn estimate_conversation_tokens_with_thinking() {
        let messages = vec![crate::provider::Message {
            role: crate::provider::MessageRole::Assistant,
            content: vec![crate::provider::ContentBlock::Thinking {
                text: "Let me reason about this carefully.".to_string(),
                signature: None,
            }],
        }];
        let total = estimate_conversation_tokens("system", &messages);
        assert!(total > count_tokens("system"));
    }

    #[test]
    fn estimate_conversation_empty() {
        let total = estimate_conversation_tokens("You are helpful.", &[]);
        let sys_tokens = count_tokens("You are helpful.") + 4;
        assert_eq!(total, sys_tokens);
    }
}
