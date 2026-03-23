//! Message truncation strategies for freeing context window space.
//!
//! Two strategies are provided:
//! - **Simple truncation**: drop oldest messages, keep last N.
//! - **Smart truncation**: replace large tool results first, then drop oldest
//!   message pairs, preserving the initial task description.

use crate::provider::{ContentBlock, Message};
use crate::util::tokens;

/// Simple truncation — remove oldest messages, keep last `keep`.
///
/// Returns the number of messages removed.
pub fn truncate_keep_last(messages: &mut Vec<Message>, keep: usize) -> usize {
    if messages.len() <= keep {
        return 0;
    }
    let removed = messages.len() - keep;
    messages.drain(0..removed);
    removed
}

/// Smart truncation — free tokens by replacing tool results first, then
/// dropping oldest message pairs.
///
/// The initial user message (index 0) is never removed. Returns actual
/// tokens freed.
#[allow(clippy::cast_possible_truncation)]
pub fn truncate_smart(messages: &mut Vec<Message>, tokens_to_free: u64) -> u64 {
    let mut freed: u64 = 0;

    for msg in messages.iter_mut() {
        if freed >= tokens_to_free {
            break;
        }
        for block in &mut msg.content {
            if let ContentBlock::ToolResult { content, .. } = block {
                if content.len() > 200 {
                    let original_tokens = tokens::count_tokens(content) as u64;
                    let truncated_msg = format!("[truncated — {original_tokens} tokens]");
                    *content = truncated_msg;
                    let new_tokens = tokens::count_tokens(content) as u64;
                    freed += original_tokens.saturating_sub(new_tokens);
                }
            }
        }
    }

    if freed >= tokens_to_free {
        return freed;
    }

    let idx = 1;
    while idx < messages.len() && freed < tokens_to_free {
        let msg_tokens = estimate_message_tokens(&messages[idx]);
        freed += msg_tokens;
        messages.remove(idx);
    }

    freed
}

/// Estimate token count for a single message.
#[allow(clippy::cast_possible_truncation)]
fn estimate_message_tokens(msg: &Message) -> u64 {
    let mut total: u64 = 4;
    for block in &msg.content {
        total += match block {
            ContentBlock::Text(t) => tokens::count_tokens(t) as u64,
            ContentBlock::Thinking { text, .. } => tokens::count_tokens(text) as u64,
            ContentBlock::ToolUse { input, .. } => {
                tokens::count_tokens(&serde_json::to_string(input).unwrap_or_default()) as u64 + 10
            }
            ContentBlock::ToolResult { content, .. } => tokens::count_tokens(content) as u64 + 4,
            ContentBlock::Image { .. } => 85,
        };
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::MessageRole;

    fn make_msg(text: &str) -> Message {
        Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text(text.to_string())],
        }
    }

    #[test]
    fn truncate_keep_last_basic() {
        let mut msgs = vec![make_msg("a"), make_msg("b"), make_msg("c"), make_msg("d")];
        let removed = truncate_keep_last(&mut msgs, 2);
        assert_eq!(removed, 2);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn truncate_keep_last_no_op() {
        let mut msgs = vec![make_msg("a")];
        let removed = truncate_keep_last(&mut msgs, 5);
        assert_eq!(removed, 0);
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn truncate_keep_last_exact() {
        let mut msgs = vec![make_msg("a"), make_msg("b")];
        let removed = truncate_keep_last(&mut msgs, 2);
        assert_eq!(removed, 0);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn truncate_keep_last_zero() {
        let mut msgs = vec![make_msg("a"), make_msg("b")];
        let removed = truncate_keep_last(&mut msgs, 0);
        assert_eq!(removed, 2);
        assert!(msgs.is_empty());
    }

    #[test]
    fn truncate_smart_replaces_tool_results() {
        let large_content = "x".repeat(1000);
        let mut msgs = vec![
            make_msg("task description"),
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".to_string(),
                    content: large_content,
                    is_error: false,
                }],
            },
        ];
        let freed = truncate_smart(&mut msgs, 50);
        assert!(freed > 0);
        assert_eq!(msgs.len(), 2);
        if let ContentBlock::ToolResult { content, .. } = &msgs[1].content[0] {
            assert!(content.starts_with("[truncated"));
        }
    }

    #[test]
    fn truncate_smart_skips_small_tool_results() {
        let mut msgs = vec![
            make_msg("task"),
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tc1".to_string(),
                    content: "ok".to_string(),
                    is_error: false,
                }],
            },
        ];
        let freed = truncate_smart(&mut msgs, 50);
        assert_eq!(msgs.len(), 1);
        assert!(freed > 0);
    }

    #[test]
    fn truncate_smart_preserves_first_message() {
        let mut msgs = vec![make_msg("task"), make_msg("b"), make_msg("c")];
        let freed = truncate_smart(&mut msgs, 100_000);
        assert!(freed > 0);
        assert!(!msgs.is_empty());
        if let ContentBlock::Text(t) = &msgs[0].content[0] {
            assert_eq!(t, "task");
        }
    }

    #[test]
    fn estimate_message_tokens_text() {
        let msg = make_msg("Hello world");
        let tokens = estimate_message_tokens(&msg);
        assert!(tokens > 0);
        assert!(tokens < 20);
    }
}
