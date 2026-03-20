//! `search_files` tool — regex search across files.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;

/// Handler for searching file contents with regex patterns.
pub struct SearchFilesHandler;

#[async_trait]
impl ToolHandler for SearchFilesHandler {
    fn name(&self) -> &str {
        "search_files"
    }

    fn description(&self) -> &str {
        "Search for a regex pattern across files in the specified directory. Returns matching lines with context."
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
            "required": ["path", "regex"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory path to search in (relative to the working directory)"
                },
                "regex": {
                    "type": "string",
                    "description": "The regular expression pattern to search for"
                },
                "file_pattern": {
                    "type": "string",
                    "description": "Optional glob pattern to filter files (e.g., \"*.rs\")"
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
            "search_files not yet implemented".to_string(),
        ))
    }
}
