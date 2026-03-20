//! `read_file` tool — reads file contents with optional line range.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;

/// Handler for reading file contents.
pub struct ReadFileHandler;

#[async_trait]
impl ToolHandler for ReadFileHandler {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file at the specified path. Use the start_line and end_line parameters to read specific portions of large files."
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::ReadOnly
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path of the file to read (relative to the working directory)"
                },
                "start_line": {
                    "type": "integer",
                    "description": "The starting line number to read from (1-indexed, inclusive)"
                },
                "end_line": {
                    "type": "integer",
                    "description": "The ending line number to read to (1-indexed, inclusive)"
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
            "read_file not yet implemented".to_string(),
        ))
    }
}
