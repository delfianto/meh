//! MCP (Model Context Protocol) client — manages connections to external tool servers.
//!
//! MCP servers expose additional tools that the LLM can call, extending
//! the built-in tool set at runtime. The [`McpHub`] manages the lifecycle
//! of server connections, and the [`client`] module handles the JSON-RPC
//! protocol over the configured transport.
//!
//! ```text
//!   mcp_settings.json
//!     servers = { ... }
//!         │
//!         ▼
//!   McpHub::from_settings()
//!         │
//!         ├── Server A (stdio)  ──► spawn child process
//!         │     └── McpClient ◄──► JSON-RPC over stdin/stdout
//!         │
//!         └── Server B (SSE)    ──► HTTP connection (STEP 22)
//!               └── McpClient ◄──► JSON-RPC over SSE
//!         │
//!         ▼
//!   McpHub::tool_definitions() ──► merged into system prompt
//!   McpHub::call_tool()        ──► proxy through mcp_tool handler
//! ```
//!
//! Supported transports:
//! - **stdio** — spawns the server as a child process, communicates via stdin/stdout
//! - **SSE** — connects to an HTTP endpoint with Server-Sent Events (STEP 22)

pub mod client;
pub mod transport;
pub mod types;

use client::McpClient;
use std::collections::HashMap;
use types::{McpContent, McpSettings, McpToolDef};

/// Central hub managing connections to all configured MCP servers.
pub struct McpHub {
    /// Connected MCP clients keyed by server name.
    clients: HashMap<String, McpClient>,
    /// Maps full tool name (`mcp__{server}__{tool}`) to `(server_name, tool_name)`.
    tool_map: HashMap<String, (String, String)>,
    /// Cached tool definitions for system prompt injection.
    tool_defs: Vec<McpToolDef>,
    /// Server names for each cached tool def (parallel with `tool_defs`).
    tool_servers: Vec<String>,
}

impl McpHub {
    /// Connect to all configured MCP servers and discover their tools.
    ///
    /// Connection failures are logged as warnings but do not prevent
    /// other servers from connecting.
    pub async fn from_settings(settings: &McpSettings) -> anyhow::Result<Self> {
        let mut clients = HashMap::new();
        let mut tool_map = HashMap::new();
        let mut tool_defs = Vec::new();
        let mut tool_servers = Vec::new();

        for (name, config) in &settings.servers {
            match McpClient::connect(name.clone(), config).await {
                Ok(client) => match client.list_tools().await {
                    Ok(tools) => {
                        for tool in &tools {
                            let full_name = format!("mcp__{name}__{}", tool.name);
                            tool_map.insert(full_name, (name.clone(), tool.name.clone()));
                            tool_defs.push(tool.clone());
                            tool_servers.push(name.clone());
                        }
                        tracing::info!(
                            server = %name,
                            tool_count = tools.len(),
                            "MCP server connected"
                        );
                        clients.insert(name.clone(), client);
                    }
                    Err(e) => {
                        tracing::warn!(
                            server = %name,
                            error = %e,
                            "Failed to list MCP tools"
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        server = %name,
                        error = %e,
                        "Failed to connect to MCP server"
                    );
                }
            }
        }

        Ok(Self {
            clients,
            tool_map,
            tool_defs,
            tool_servers,
        })
    }

    /// Get tool definitions for all connected servers (for system prompt).
    ///
    /// Each tool name is prefixed as `mcp__{server}__{tool}` to avoid
    /// collisions with built-in tools.
    pub fn tool_definitions(&self) -> Vec<crate::provider::ToolDefinition> {
        self.tool_defs
            .iter()
            .zip(&self.tool_servers)
            .map(|(def, server)| crate::provider::ToolDefinition {
                name: format!("mcp__{server}__{}", def.name),
                description: def
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("MCP tool from server '{server}'")),
                input_schema: def.input_schema.clone(),
            })
            .collect()
    }

    /// Call an MCP tool by its full name (`mcp__{server}__{tool}`).
    pub async fn call_tool(
        &self,
        full_tool_name: &str,
        arguments: serde_json::Value,
    ) -> anyhow::Result<String> {
        let (server_name, tool_name) = self
            .tool_map
            .get(full_tool_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown MCP tool: {full_tool_name}"))?;

        let client = self
            .clients
            .get(server_name)
            .ok_or_else(|| anyhow::anyhow!("MCP server not connected: {server_name}"))?;

        let result = client.call_tool(tool_name, arguments).await?;

        let mut output = String::new();
        for content in &result.content {
            match content {
                McpContent::Text { text } => output.push_str(text),
                McpContent::Resource { resource } => {
                    output.push_str(&serde_json::to_string_pretty(resource)?);
                }
                McpContent::Image { .. } => {
                    output.push_str("[image]");
                }
            }
        }

        Ok(output)
    }

    /// Check if there are any connected servers.
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    /// Disconnect all servers.
    pub async fn disconnect_all(self) -> anyhow::Result<()> {
        for (name, client) in self.clients {
            if let Err(e) = client.disconnect().await {
                tracing::warn!(
                    server = %name,
                    error = %e,
                    "Error disconnecting MCP server"
                );
            }
        }
        Ok(())
    }
}

/// Load MCP settings from `~/.meh/mcp_settings.json`.
///
/// Returns an empty settings structure if the file does not exist.
pub fn load_mcp_settings() -> anyhow::Result<McpSettings> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let path = home.join(".meh").join("mcp_settings.json");

    if !path.exists() {
        return Ok(McpSettings {
            servers: HashMap::new(),
        });
    }

    let content = std::fs::read_to_string(&path)?;
    let settings: McpSettings = serde_json::from_str(&content)?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_settings() {
        let settings = McpSettings {
            servers: HashMap::new(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let hub = rt.block_on(McpHub::from_settings(&settings)).unwrap();
        assert!(hub.tool_definitions().is_empty());
        assert!(hub.is_empty());
    }

    #[tokio::test]
    async fn test_call_unknown_tool() {
        let settings = McpSettings {
            servers: HashMap::new(),
        };
        let hub = McpHub::from_settings(&settings).await.unwrap();
        let result = hub
            .call_tool("mcp__nonexistent__tool", serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown MCP tool"));
    }

    #[test]
    fn test_load_settings_missing_file() {
        let settings = load_mcp_settings();
        assert!(settings.is_ok());
    }
}
