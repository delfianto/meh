//! `ask_followup_question` tool — ask user for clarification.

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
        _params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        Ok(ToolResponse::error(
            "ask_followup_question not yet implemented".to_string(),
        ))
    }
}
