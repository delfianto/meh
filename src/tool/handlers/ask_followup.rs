//! `ask_followup_question` tool — ask user for clarification.
//!
//! The tool handler validates params and returns the question text.
//! The agent loop detects this tool and handles user interaction:
//! sends the question to the TUI, waits for user input, and returns
//! the user's response as the tool result.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;

/// Handler for asking the user a followup question.
pub struct AskFollowupHandler;

#[async_trait]
impl ToolHandler for AskFollowupHandler {
    fn name(&self) -> &str {
        "ask_followup_question"
    }

    fn description(&self) -> &str {
        "Ask the user a question to gather additional information needed to complete the task."
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Informational
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["question"],
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                }
            }
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        let question = params
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: question"))?;

        Ok(ToolResponse::success(question.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext {
            cwd: "/tmp".to_string(),
            auto_approved: false,
        }
    }

    #[tokio::test]
    async fn test_ask_followup() {
        let handler = AskFollowupHandler;
        let result = handler
            .execute(
                serde_json::json!({"question": "What file should I edit?"}),
                &ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("What file should I edit?"));
    }

    #[tokio::test]
    async fn test_ask_followup_missing_question() {
        let handler = AskFollowupHandler;
        let result = handler.execute(serde_json::json!({}), &ctx()).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_ask_followup_no_approval_required() {
        let handler = AskFollowupHandler;
        assert!(!handler.requires_approval());
        assert_eq!(handler.category(), ToolCategory::Informational);
    }

    #[test]
    fn test_ask_followup_metadata() {
        let handler = AskFollowupHandler;
        assert_eq!(handler.name(), "ask_followup_question");
    }
}
