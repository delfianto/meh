//! Conversation summarization using LLM to condense history.
//!
//! When the context window approaches its limit, older messages are
//! summarized into a compact description that preserves key decisions,
//! file modifications, and task progress. The summary replaces the
//! original messages, freeing token space.

use crate::provider::{ContentBlock, Message, MessageRole};
use std::fmt::Write as _;

/// Result of a summarization operation.
pub struct SummarizationResult {
    /// Summary text to insert as the first user message.
    pub summary: String,
    /// Range of original messages to remove `[start, end)`.
    pub remove_range: (usize, usize),
    /// Approximate tokens freed by removing those messages.
    pub tokens_freed: u64,
}

/// Build the summarization prompt for the LLM.
pub fn build_summary_prompt(messages: &[Message]) -> String {
    let mut conversation = String::new();
    for msg in messages {
        let role = match msg.role {
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
        };
        for block in &msg.content {
            match block {
                ContentBlock::Text(t) => {
                    let _ = writeln!(conversation, "{role}: {t}");
                }
                ContentBlock::ToolUse { name, .. } => {
                    let _ = writeln!(conversation, "{role}: [called tool: {name}]");
                }
                ContentBlock::ToolResult { content, .. } => {
                    let preview = if content.len() > 200 {
                        format!("{}...", &content[..200])
                    } else {
                        content.clone()
                    };
                    let _ = writeln!(conversation, "{role}: [tool result: {preview}]");
                }
                _ => {}
            }
        }
    }

    format!(
        "Summarize the following conversation concisely, preserving:\n\
         - Key decisions made\n\
         - Files modified and why\n\
         - Current task state and progress\n\
         - Any unresolved issues\n\n\
         Conversation:\n{conversation}\n\
         Summary:"
    )
}

/// Apply a summarization result to the message history.
///
/// Removes the specified range and inserts a summary message at the start.
pub fn apply_summarization(messages: &mut Vec<Message>, result: &SummarizationResult) {
    let (start, end) = result.remove_range;
    let end = end.min(messages.len());
    if start < end {
        messages.drain(start..end);
    }

    messages.insert(
        0,
        Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text(format!(
                "[Previous conversation summary]\n{}",
                result.summary
            ))],
        },
    );
}

/// Calculate the number of messages from the start that need summarizing
/// to free at least `tokens_to_free` tokens.
#[allow(clippy::cast_possible_truncation)]
pub fn messages_to_summarize(messages: &[Message], tokens_to_free: u64) -> (usize, usize) {
    let mut accumulated: u64 = 0;
    let start = 0;
    let mut end = 0;

    for (i, msg) in messages.iter().enumerate() {
        if accumulated >= tokens_to_free {
            break;
        }
        let mut msg_tokens: u64 = 4;
        for block in &msg.content {
            msg_tokens += match block {
                ContentBlock::Text(t) => crate::util::tokens::count_tokens(t) as u64,
                ContentBlock::ToolResult { content, .. } => {
                    crate::util::tokens::count_tokens(content) as u64
                }
                ContentBlock::ToolUse { input, .. } => crate::util::tokens::count_tokens(
                    &serde_json::to_string(input).unwrap_or_default(),
                ) as u64,
                _ => 0,
            };
        }
        accumulated += msg_tokens;
        end = i + 1;
    }

    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(role: MessageRole, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text(text.to_string())],
        }
    }

    #[test]
    fn build_summary_prompt_includes_messages() {
        let messages = vec![
            make_msg(MessageRole::User, "Fix the bug"),
            make_msg(MessageRole::Assistant, "I'll look at main.rs"),
        ];
        let prompt = build_summary_prompt(&messages);
        assert!(prompt.contains("Fix the bug"));
        assert!(prompt.contains("main.rs"));
        assert!(prompt.contains("Key decisions"));
    }

    #[test]
    fn build_summary_prompt_includes_tool_calls() {
        let messages = vec![Message {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tc1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "main.rs"}),
            }],
        }];
        let prompt = build_summary_prompt(&messages);
        assert!(prompt.contains("read_file"));
    }

    #[test]
    fn build_summary_prompt_truncates_long_tool_results() {
        let long = "x".repeat(500);
        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tc1".to_string(),
                content: long,
                is_error: false,
            }],
        }];
        let prompt = build_summary_prompt(&messages);
        assert!(prompt.contains("..."));
    }

    #[test]
    fn apply_summarization_replaces_messages() {
        let mut messages = vec![
            make_msg(MessageRole::User, "original 1"),
            make_msg(MessageRole::Assistant, "original 2"),
            make_msg(MessageRole::User, "original 3"),
        ];
        let result = SummarizationResult {
            summary: "Summary of the conversation.".to_string(),
            remove_range: (0, 2),
            tokens_freed: 100,
        };
        apply_summarization(&mut messages, &result);
        assert_eq!(messages.len(), 2);
        if let ContentBlock::Text(t) = &messages[0].content[0] {
            assert!(t.contains("Summary of the conversation"));
            assert!(t.contains("[Previous conversation summary]"));
        }
    }

    #[test]
    fn apply_summarization_empty_range() {
        let mut messages = vec![make_msg(MessageRole::User, "hello")];
        let result = SummarizationResult {
            summary: "Nothing happened.".to_string(),
            remove_range: (0, 0),
            tokens_freed: 0,
        };
        apply_summarization(&mut messages, &result);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn messages_to_summarize_basic() {
        let messages = vec![
            make_msg(MessageRole::User, "short"),
            make_msg(MessageRole::Assistant, "also short"),
        ];
        let (start, end) = messages_to_summarize(&messages, 1);
        assert_eq!(start, 0);
        assert!(end > 0);
    }

    #[test]
    fn messages_to_summarize_large_budget() {
        let messages = vec![
            make_msg(MessageRole::User, "a"),
            make_msg(MessageRole::Assistant, "b"),
        ];
        let (start, end) = messages_to_summarize(&messages, 100_000);
        assert_eq!(start, 0);
        assert_eq!(end, 2);
    }

    #[test]
    fn messages_to_summarize_empty() {
        let messages: Vec<Message> = vec![];
        let (start, end) = messages_to_summarize(&messages, 100);
        assert_eq!(start, 0);
        assert_eq!(end, 0);
    }
}
