//! `execute_command` tool — run shell commands with timeout.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;

/// Handler for executing shell commands.
pub struct ExecuteCommandHandler;

#[async_trait]
impl ToolHandler for ExecuteCommandHandler {
    fn name(&self) -> &str {
        "execute_command"
    }

    fn description(&self) -> &str {
        "Execute a shell command in the working directory. The command runs with a timeout and returns stdout/stderr."
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Command
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
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
            "execute_command not yet implemented".to_string(),
        ))
    }
}
