//! `attempt_completion` tool — signal task completion.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;

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
        _params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        Ok(ToolResponse::error(
            "attempt_completion not yet implemented".to_string(),
        ))
    }
}
