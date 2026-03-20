# STEP 21 — MCP Client (Stdio Transport)

## Objective
Implement the MCP (Model Context Protocol) client with stdio transport. After this step, the app can connect to MCP servers that communicate over stdin/stdout and call their tools.

## Prerequisites
- STEP 11 complete (tool system)
- STEP 13 complete (permissions)

## Detailed Instructions

### 21.1 MCP Protocol Types (`src/tool/mcp/types.rs`)

```rust
//! MCP protocol types (JSON-RPC 2.0 based).

use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 request.
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String, // "2.0"
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

/// MCP tool definition from a server.
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// MCP tool result content.
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolResult {
    pub content: Vec<McpContent>,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum McpContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String, // base64
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(rename = "resource")]
    Resource {
        resource: serde_json::Value,
    },
}

/// MCP server configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default = "default_transport")]
    pub transport: String,
    #[serde(default)]
    pub auto_approve: Vec<String>,
}

fn default_transport() -> String { "stdio".to_string() }

/// MCP settings file structure.
#[derive(Debug, Clone, Deserialize)]
pub struct McpSettings {
    #[serde(default)]
    pub servers: std::collections::HashMap<String, McpServerConfig>,
}
```

### 21.2 MCP Transport Trait and Stdio Implementation (`src/tool/mcp/transport.rs`)

```rust
//! MCP transport layer — stdio, SSE, HTTP.

use super::types::*;
use async_trait::async_trait;

#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and receive a response.
    async fn send(&self, request: JsonRpcRequest) -> anyhow::Result<JsonRpcResponse>;
    /// Close the transport.
    async fn close(&self) -> anyhow::Result<()>;
}

/// Stdio transport — communicates with MCP server via stdin/stdout.
pub struct StdioTransport {
    stdin: tokio::sync::Mutex<tokio::process::ChildStdin>,
    pending: std::sync::Arc<tokio::sync::Mutex<
        std::collections::HashMap<u64, tokio::sync::oneshot::Sender<JsonRpcResponse>>
    >>,
    _child: std::sync::Arc<tokio::sync::Mutex<tokio::process::Child>>,
}

impl StdioTransport {
    pub async fn new(
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
    ) -> anyhow::Result<Self> {
        // 1. Spawn child process with piped stdin/stdout/stderr
        // 2. Start a background task to read stdout lines
        // 3. Parse each line as JsonRpcResponse
        // 4. Route to pending request via oneshot channel
        // 5. Log stderr lines as warnings
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, request: JsonRpcRequest) -> anyhow::Result<JsonRpcResponse> {
        // 1. Create oneshot channel for response
        // 2. Register in pending map
        // 3. Serialize request as JSON line (append \n)
        // 4. Write to stdin
        // 5. Wait for response on oneshot receiver (with timeout)
    }

    async fn close(&self) -> anyhow::Result<()> {
        // Kill child process
    }
}
```

**Implementation details for StdioTransport::new**:

```rust
pub async fn new(
    command: &str,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
) -> anyhow::Result<Self> {
    use tokio::process::Command;
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut cmd = Command::new(command);
    cmd.args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    for (key, value) in env {
        // Expand environment variables in values
        let expanded = shellexpand::env(value)
            .unwrap_or_else(|_| std::borrow::Cow::Borrowed(value));
        cmd.env(key, expanded.as_ref());
    }

    let mut child = cmd.spawn()?;

    let stdin = child.stdin.take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture stdin"))?;
    let stdout = child.stdout.take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout"))?;
    let stderr = child.stderr.take()
        .ok_or_else(|| anyhow::anyhow!("Failed to capture stderr"))?;

    let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Stdout reader task — parse JSON-RPC responses
    let pending_clone = pending.clone();
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() { continue; }
            match serde_json::from_str::<JsonRpcResponse>(&line) {
                Ok(response) => {
                    if let Some(id) = response.id {
                        let mut pending = pending_clone.lock().await;
                        if let Some(sender) = pending.remove(&id) {
                            let _ = sender.send(response);
                        }
                    }
                    // Notifications (no id) are logged but not routed
                }
                Err(e) => {
                    tracing::warn!(error = %e, line = %line, "Failed to parse MCP response");
                }
            }
        }
    });

    // Stderr reader task — log warnings
    tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::warn!(target: "mcp_server_stderr", "{}", line);
        }
    });

    Ok(Self {
        stdin: tokio::sync::Mutex::new(stdin),
        pending,
        _child: Arc::new(tokio::sync::Mutex::new(child)),
    })
}
```

**Implementation details for StdioTransport::send**:

```rust
async fn send(&self, request: JsonRpcRequest) -> anyhow::Result<JsonRpcResponse> {
    use tokio::io::AsyncWriteExt;

    let (tx, rx) = tokio::sync::oneshot::channel();

    // Register pending request
    {
        let mut pending = self.pending.lock().await;
        pending.insert(request.id, tx);
    }

    // Serialize and write
    let mut json = serde_json::to_string(&request)?;
    json.push('\n');

    {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(json.as_bytes()).await?;
        stdin.flush().await?;
    }

    // Wait for response with timeout
    match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => anyhow::bail!("Response channel closed"),
        Err(_) => {
            // Clean up pending request
            let mut pending = self.pending.lock().await;
            pending.remove(&request.id);
            anyhow::bail!("MCP request timed out after 30s")
        }
    }
}
```

### 21.3 MCP Client (`src/tool/mcp/client.rs`)

```rust
//! MCP protocol client — wraps transport with protocol-level operations.

use super::types::*;
use super::transport::McpTransport;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct McpClient {
    transport: Box<dyn McpTransport>,
    request_id: AtomicU64,
    server_name: String,
}

impl McpClient {
    pub async fn connect(
        server_name: String,
        config: &McpServerConfig,
    ) -> anyhow::Result<Self> {
        let transport: Box<dyn McpTransport> = match config.transport.as_str() {
            "stdio" => Box::new(
                super::transport::StdioTransport::new(&config.command, &config.args, &config.env).await?
            ),
            _ => anyhow::bail!("Unsupported transport: {}", config.transport),
        };

        let client = Self {
            transport,
            request_id: AtomicU64::new(1),
            server_name,
        };

        // Initialize the connection
        client.initialize().await?;

        Ok(client)
    }

    fn next_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::Relaxed)
    }

    /// MCP initialize handshake.
    async fn initialize(&self) -> anyhow::Result<()> {
        let response = self.transport.send(JsonRpcRequest {
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
        }).await?;

        if let Some(err) = response.error {
            anyhow::bail!("MCP initialize failed: {}", err.message);
        }

        // Send initialized notification
        let _ = self.transport.send(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: self.next_id(),
            method: "notifications/initialized".to_string(),
            params: None,
        }).await;

        Ok(())
    }

    /// List available tools from the server.
    pub async fn list_tools(&self) -> anyhow::Result<Vec<McpToolDef>> {
        let response = self.transport.send(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: self.next_id(),
            method: "tools/list".to_string(),
            params: None,
        }).await?;

        if let Some(err) = response.error {
            anyhow::bail!("tools/list failed: {}", err.message);
        }

        let result = response.result.unwrap_or_default();
        let tools: Vec<McpToolDef> = serde_json::from_value(
            result.get("tools").cloned().unwrap_or(serde_json::json!([]))
        )?;
        Ok(tools)
    }

    /// Call a tool on the server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> anyhow::Result<McpToolResult> {
        let response = self.transport.send(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: self.next_id(),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": tool_name,
                "arguments": arguments,
            })),
        }).await?;

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
```

### 21.4 McpHub (`src/tool/mcp/mod.rs`)

```rust
//! MCP Hub — manages connections to all configured MCP servers.

pub mod client;
pub mod transport;
pub mod types;

use client::McpClient;
use types::{McpSettings, McpToolDef};
use std::collections::HashMap;

pub struct McpHub {
    clients: HashMap<String, McpClient>,
    tool_map: HashMap<String, (String, String)>, // full_tool_name → (server_name, tool_name)
}

impl McpHub {
    /// Load settings and connect to all configured servers.
    pub async fn from_settings(settings: &McpSettings) -> anyhow::Result<Self> {
        let mut clients = HashMap::new();
        let mut tool_map = HashMap::new();

        for (name, config) in &settings.servers {
            match McpClient::connect(name.clone(), config).await {
                Ok(client) => {
                    // List tools
                    match client.list_tools().await {
                        Ok(tools) => {
                            for tool in &tools {
                                let full_name = format!("mcp__{}__{}", name, tool.name);
                                tool_map.insert(full_name, (name.clone(), tool.name.clone()));
                            }
                            clients.insert(name.clone(), client);
                            tracing::info!(server = %name, tool_count = tools.len(), "MCP server connected");
                        }
                        Err(e) => {
                            tracing::warn!(server = %name, error = %e, "Failed to list MCP tools");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(server = %name, error = %e, "Failed to connect to MCP server");
                }
            }
        }

        Ok(Self { clients, tool_map })
    }

    /// Get tool definitions for all connected servers (for system prompt).
    pub fn tool_definitions(&self) -> Vec<crate::provider::ToolDefinition> {
        // Return all tools from all servers, prefixed with server name
        // Each tool name is formatted as "mcp__{server}__{tool}"
        // Include the tool's input schema from McpToolDef
        Vec::new() // Placeholder — iterate tool_map + cached McpToolDef
    }

    /// Call an MCP tool.
    pub async fn call_tool(
        &self,
        full_tool_name: &str,
        arguments: serde_json::Value,
    ) -> anyhow::Result<String> {
        let (server_name, tool_name) = self.tool_map.get(full_tool_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown MCP tool: {full_tool_name}"))?;

        let client = self.clients.get(server_name)
            .ok_or_else(|| anyhow::anyhow!("MCP server not connected: {server_name}"))?;

        let result = client.call_tool(tool_name, arguments).await?;

        // Convert MCP result to string
        let mut output = String::new();
        for content in &result.content {
            match content {
                types::McpContent::Text { text } => output.push_str(text),
                types::McpContent::Resource { resource } => {
                    output.push_str(&serde_json::to_string_pretty(resource)?);
                }
                types::McpContent::Image { .. } => {
                    output.push_str("[image]");
                }
            }
        }

        Ok(output)
    }

    /// Disconnect all servers.
    pub async fn disconnect_all(self) -> anyhow::Result<()> {
        for (name, client) in self.clients {
            if let Err(e) = client.disconnect().await {
                tracing::warn!(server = %name, error = %e, "Error disconnecting MCP server");
            }
        }
        Ok(())
    }
}
```

### 21.5 MCP Tool Handler (`src/tool/handlers/mcp_tool.rs`)

```rust
//! mcp_tool handler — routes to MCP servers via McpHub.

use crate::tool::mcp::McpHub;
use crate::tool::{ToolHandler, ToolCategory, ToolResult};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct McpToolHandler {
    hub: Arc<RwLock<McpHub>>,
}

impl McpToolHandler {
    pub fn new(hub: Arc<RwLock<McpHub>>) -> Self {
        Self { hub }
    }
}

#[async_trait]
impl ToolHandler for McpToolHandler {
    fn name(&self) -> &str {
        "mcp_tool"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Mcp
    }

    fn requires_approval(&self) -> bool {
        true // Unless auto_approve pattern matches
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let tool_name = input["tool_name"].as_str().unwrap_or("");
        let arguments = input.get("arguments").cloned().unwrap_or(serde_json::json!({}));

        let hub = self.hub.read().await;
        match hub.call_tool(tool_name, arguments).await {
            Ok(output) => ToolResult::success(output),
            Err(e) => ToolResult::error(format!("MCP tool error: {e}")),
        }
    }
}
```

### 21.6 Settings file location

MCP settings are read from `~/.meh/mcp_settings.json`:
```json
{
    "servers": {
        "filesystem": {
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/user"],
            "transport": "stdio"
        }
    }
}
```

Expand environment variables in `env` values (e.g., `"${HOME}"` -> actual home dir).

**Loading logic**:
```rust
pub fn load_mcp_settings() -> anyhow::Result<McpSettings> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let path = home.join(".meh").join("mcp_settings.json");

    if !path.exists() {
        return Ok(McpSettings { servers: HashMap::new() });
    }

    let content = std::fs::read_to_string(&path)?;
    let settings: McpSettings = serde_json::from_str(&content)?;
    Ok(settings)
}
```

## Tests

```rust
#[cfg(test)]
mod types_tests {
    use super::types::*;

    #[test]
    fn test_json_rpc_request_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "tools/list".to_string(),
            params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"tools/list\""));
        assert!(!json.contains("\"params\"")); // skip_serializing_if None
    }

    #[test]
    fn test_json_rpc_request_with_params() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 42,
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "read", "arguments": {"path": "/tmp"}})),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"params\""));
        assert!(json.contains("\"name\":\"read\""));
    }

    #[test]
    fn test_json_rpc_response_parsing() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"read","description":"Read a file","inputSchema":{"type":"object"}}]}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
    }

    #[test]
    fn test_json_rpc_response_with_error() {
        let json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid Request");
    }

    #[test]
    fn test_mcp_tool_def_parsing() {
        let json = r#"{"name":"read_file","description":"Read a file","inputSchema":{"type":"object","properties":{"path":{"type":"string"}}}}"#;
        let tool: McpToolDef = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.description, Some("Read a file".to_string()));
    }

    #[test]
    fn test_mcp_tool_result_parsing() {
        let json = r#"{"content":[{"type":"text","text":"file contents here"}],"is_error":false}"#;
        let result: McpToolResult = serde_json::from_str(json).unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            McpContent::Text { text } => assert_eq!(text, "file contents here"),
            _ => panic!("Expected text content"),
        }
    }

    #[test]
    fn test_mcp_tool_result_error() {
        let json = r#"{"content":[{"type":"text","text":"not found"}],"is_error":true}"#;
        let result: McpToolResult = serde_json::from_str(json).unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn test_mcp_content_image() {
        let json = r#"{"type":"image","data":"base64data","mimeType":"image/png"}"#;
        let content: McpContent = serde_json::from_str(json).unwrap();
        match content {
            McpContent::Image { data, mime_type } => {
                assert_eq!(data, "base64data");
                assert_eq!(mime_type, "image/png");
            }
            _ => panic!("Expected image content"),
        }
    }

    #[test]
    fn test_mcp_settings_parsing() {
        let json = r#"{"servers":{"fs":{"command":"node","args":["server.js"],"transport":"stdio"}}}"#;
        let settings: McpSettings = serde_json::from_str(json).unwrap();
        assert!(settings.servers.contains_key("fs"));
        assert_eq!(settings.servers["fs"].command, "node");
        assert_eq!(settings.servers["fs"].args, vec!["server.js"]);
    }

    #[test]
    fn test_mcp_settings_empty() {
        let json = r#"{}"#;
        let settings: McpSettings = serde_json::from_str(json).unwrap();
        assert!(settings.servers.is_empty());
    }

    #[test]
    fn test_mcp_server_config_defaults() {
        let json = r#"{"command":"node"}"#;
        let config: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.transport, "stdio"); // default
        assert!(config.args.is_empty());
        assert!(config.env.is_empty());
        assert!(config.auto_approve.is_empty());
    }
}

#[cfg(test)]
mod client_tests {
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

#[cfg(test)]
mod hub_tests {
    use super::*;
    use super::types::McpSettings;

    #[test]
    fn test_empty_settings() {
        let settings = McpSettings { servers: std::collections::HashMap::new() };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let hub = rt.block_on(McpHub::from_settings(&settings)).unwrap();
        assert!(hub.tool_definitions().is_empty());
    }

    #[tokio::test]
    async fn test_call_unknown_tool() {
        let settings = McpSettings { servers: std::collections::HashMap::new() };
        let hub = McpHub::from_settings(&settings).await.unwrap();
        let result = hub.call_tool("mcp__nonexistent__tool", serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown MCP tool"));
    }
}
```

## Acceptance Criteria
- [ ] MCP JSON-RPC types serialize/deserialize correctly
- [ ] Stdio transport spawns process, communicates via stdin/stdout
- [ ] McpClient handles initialize handshake, tools/list, tools/call
- [ ] McpHub manages multiple server connections
- [ ] MCP tools appear in system prompt with server prefix
- [ ] MCP tool handler routes calls through McpHub
- [ ] Server connection errors logged but don't crash the app
- [ ] Settings loaded from ~/.meh/mcp_settings.json
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass
