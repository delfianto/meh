//! MCP protocol client — wraps transport with protocol-level operations.
//!
//! The [`McpClient`] handles the MCP lifecycle: initialize handshake,
//! tool discovery, tool invocation, and disconnection. It abstracts
//! away the JSON-RPC details behind typed method calls.

use super::transport::McpTransport;
use super::types::{JsonRpcRequest, McpServerConfig, McpToolDef, McpToolResult};
use std::sync::atomic::{AtomicU64, Ordering};

/// MCP protocol client connected to a single server.
pub struct McpClient {
    /// Underlying transport for JSON-RPC communication.
    transport: Box<dyn McpTransport>,
    /// Monotonically increasing request id counter.
    request_id: AtomicU64,
    /// Name of the server this client is connected to.
    server_name: String,
}

impl McpClient {
    /// Connect to an MCP server using the configured transport.
    ///
    /// Spawns the transport and performs the MCP initialize handshake.
    pub async fn connect(server_name: String, config: &McpServerConfig) -> anyhow::Result<Self> {
        let transport: Box<dyn McpTransport> = match config.transport.as_str() {
            "stdio" => Box::new(super::transport::StdioTransport::new(
                &config.command,
                &config.args,
                &config.env,
            )?),
            other => anyhow::bail!("Unsupported MCP transport: {other}"),
        };

        let client = Self {
            transport,
            request_id: AtomicU64::new(1),
            server_name,
        };

        client.initialize().await?;

        Ok(client)
    }

    /// Generate the next unique request id.
    fn next_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Server name accessor.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Perform the MCP initialize handshake.
    ///
    /// Sends the `initialize` request with client capabilities, then
    /// sends the `notifications/initialized` notification.
    async fn initialize(&self) -> anyhow::Result<()> {
        let response = self
            .transport
            .send(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: self.next_id(),
                method: "initialize".to_string(),
                params: Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "meh",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
            })
            .await?;

        if let Some(err) = response.error {
            anyhow::bail!("MCP initialize failed: {}", err.message);
        }

        let _ = self
            .transport
            .send(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: self.next_id(),
                method: "notifications/initialized".to_string(),
                params: None,
            })
            .await;

        Ok(())
    }

    /// List available tools from the server.
    pub async fn list_tools(&self) -> anyhow::Result<Vec<McpToolDef>> {
        let response = self
            .transport
            .send(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: self.next_id(),
                method: "tools/list".to_string(),
                params: None,
            })
            .await?;

        if let Some(err) = response.error {
            anyhow::bail!("tools/list failed: {}", err.message);
        }

        let result = response.result.unwrap_or_default();
        let tools: Vec<McpToolDef> = serde_json::from_value(
            result
                .get("tools")
                .cloned()
                .unwrap_or(serde_json::json!([])),
        )?;
        Ok(tools)
    }

    /// Call a tool on the server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> anyhow::Result<McpToolResult> {
        let response = self
            .transport
            .send(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: self.next_id(),
                method: "tools/call".to_string(),
                params: Some(serde_json::json!({
                    "name": tool_name,
                    "arguments": arguments,
                })),
            })
            .await?;

        if let Some(err) = response.error {
            anyhow::bail!("tools/call failed: {}", err.message);
        }

        let result = response.result.unwrap_or_default();
        let tool_result: McpToolResult = serde_json::from_value(result)?;
        Ok(tool_result)
    }

    /// Disconnect from the server.
    pub async fn disconnect(self) -> anyhow::Result<()> {
        self.transport.close().await
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_tool_name_mapping() {
        let full_name = "mcp__filesystem__read";
        let parts: Vec<&str> = full_name.splitn(3, "__").collect();
        assert_eq!(parts, vec!["mcp", "filesystem", "read"]);
    }

    #[test]
    fn test_tool_name_with_underscores() {
        let full_name = "mcp__my_server__read_file";
        let parts: Vec<&str> = full_name.splitn(3, "__").collect();
        assert_eq!(parts, vec!["mcp", "my_server", "read_file"]);
    }
}
