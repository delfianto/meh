//! `plan_mode_respond` tool — present plan and request mode switch.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;

/// Handler for presenting a plan in plan mode.
pub struct PlanModeRespondHandler;

#[async_trait]
impl ToolHandler for PlanModeRespondHandler {
    fn name(&self) -> &str {
        "plan_mode_respond"
    }

    fn description(&self) -> &str {
        "Present a plan to the user and optionally request switching to act mode to execute it."
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
            "required": ["response"],
            "properties": {
                "response": {
                    "type": "string",
                    "description": "The plan or response to present to the user"
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
            "plan_mode_respond not yet implemented".to_string(),
        ))
    }
}
