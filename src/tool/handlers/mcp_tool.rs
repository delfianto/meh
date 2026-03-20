//! `mcp_tool` handler — routes to MCP servers via `McpHub`.

use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;

/// Handler for proxying tool calls to MCP servers.
pub struct McpToolHandler {
    /// The MCP tool name as registered by the server.
    tool_name: String,
    /// Description provided by the MCP server.
    tool_description: String,
    /// Input schema provided by the MCP server.
    schema: serde_json::Value,
}

impl McpToolHandler {
    /// Create a new MCP tool handler with the given server-provided metadata.
    pub const fn new(name: String, description: String, schema: serde_json::Value) -> Self {
        Self {
            tool_name: name,
            tool_description: description,
            schema,
        }
    }
}

#[async_trait]
impl ToolHandler for McpToolHandler {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Mcp
    }

    fn input_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        Ok(ToolResponse::error(format!(
            "MCP tool '{}' not yet implemented",
            self.tool_name
        )))
    }
}
