//! `mcp_tool` handler — routes tool calls to MCP servers via [`McpHub`].
//!
//! Each MCP tool discovered from a server gets its own `McpToolHandler`
//! instance registered in the `ToolRegistry`. The handler proxies the
//! call through the shared `McpHub`.

use crate::tool::mcp::McpHub;
use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Handler for proxying tool calls to MCP servers.
pub struct McpToolHandler {
    /// The full MCP tool name (e.g., `mcp__server__tool`).
    tool_name: String,
    /// Description provided by the MCP server.
    tool_description: String,
    /// Input schema provided by the MCP server.
    schema: serde_json::Value,
    /// Shared reference to the MCP hub for calling tools.
    hub: Arc<RwLock<McpHub>>,
}

impl McpToolHandler {
    /// Create a new MCP tool handler with the given server-provided metadata.
    pub const fn new(
        name: String,
        description: String,
        schema: serde_json::Value,
        hub: Arc<RwLock<McpHub>>,
    ) -> Self {
        Self {
            tool_name: name,
            tool_description: description,
            schema,
            hub,
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
        params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        let hub = self.hub.read().await;
        match hub.call_tool(&self.tool_name, params).await {
            Ok(output) => Ok(ToolResponse::success(output)),
            Err(e) => Ok(ToolResponse::error(format!("MCP tool error: {e}"))),
        }
    }
}
