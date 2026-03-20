//! `write_file` tool — create or overwrite files.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;

/// Handler for writing file contents.
pub struct WriteFileHandler;

#[async_trait]
impl ToolHandler for WriteFileHandler {
    fn name(&self) -> &str {
        "write_to_file"
    }

    fn description(&self) -> &str {
        "Write content to a file at the specified path. Creates parent directories if they don't exist."
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::FileWrite
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path", "content"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to write to (relative to the working directory)"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
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
            "write_to_file not yet implemented".to_string(),
        ))
    }
}
