//! `attempt_completion` tool — signal task completion.
//!
//! The tool handler validates params and returns the completion message.
//! The agent loop detects this tool and handles user confirmation:
//! displays the result, optionally suggests a verification command,
//! and asks the user if the task is complete.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;
use std::fmt::Write;

/// Handler for signaling that the task is complete.
pub struct AttemptCompletionHandler;

#[async_trait]
impl ToolHandler for AttemptCompletionHandler {
    fn name(&self) -> &str {
        "attempt_completion"
    }

    fn description(&self) -> &str {
        "Signal that the task is complete and present the result to the user."
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
            "required": ["result"],
            "properties": {
                "result": {
                    "type": "string",
                    "description": "The result of the task to present to the user"
                },
                "command": {
                    "type": "string",
                    "description": "Optional CLI command to demonstrate the result"
                }
            }
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        let result_text = params
            .get("result")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: result"))?;

        let command = params.get("command").and_then(|v| v.as_str());

        let mut output = result_text.to_string();

        if let Some(cmd) = command {
            let _ = write!(output, "\n\nVerify with: {cmd}");
        }

        Ok(ToolResponse::success(output))
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
    async fn test_attempt_completion() {
        let handler = AttemptCompletionHandler;
        let result = handler
            .execute(
                serde_json::json!({"result": "Fixed the bug in main.rs"}),
                &ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Fixed the bug"));
    }

    #[tokio::test]
    async fn test_attempt_completion_with_command() {
        let handler = AttemptCompletionHandler;
        let result = handler
            .execute(
                serde_json::json!({"result": "Fixed it", "command": "cargo test"}),
                &ctx(),
            )
            .await
            .unwrap();
        assert!(result.content.contains("cargo test"));
        assert!(result.content.contains("Verify with"));
    }

    #[tokio::test]
    async fn test_attempt_completion_missing_result() {
        let handler = AttemptCompletionHandler;
        let result = handler.execute(serde_json::json!({}), &ctx()).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_completion_no_approval() {
        let handler = AttemptCompletionHandler;
        assert!(!handler.requires_approval());
        assert_eq!(handler.category(), ToolCategory::Informational);
    }

    #[test]
    fn test_completion_metadata() {
        let handler = AttemptCompletionHandler;
        assert_eq!(handler.name(), "attempt_completion");
    }
}
