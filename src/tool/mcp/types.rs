//! MCP protocol types (JSON-RPC 2.0 based).
//!
//! Defines the wire format for communication with MCP servers:
//! - JSON-RPC 2.0 request/response envelopes
//! - MCP-specific tool definitions and results
//! - Server configuration and settings file structures

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// JSON-RPC 2.0 request.
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    /// Protocol version — always "2.0".
    pub jsonrpc: String,
    /// Request identifier for correlating responses.
    pub id: u64,
    /// Method name (e.g., "initialize", "tools/list", "tools/call").
    pub method: String,
    /// Optional parameters for the method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    /// Protocol version — always "2.0".
    pub jsonrpc: String,
    /// Request identifier (absent for notifications).
    pub id: Option<u64>,
    /// Result payload on success.
    pub result: Option<serde_json::Value>,
    /// Error payload on failure.
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    /// Numeric error code.
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured error data.
    pub data: Option<serde_json::Value>,
}

/// MCP tool definition from a server.
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolDef {
    /// Tool name as registered on the server.
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// JSON Schema for the tool's input parameters.
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// MCP tool execution result.
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolResult {
    /// Content blocks returned by the tool.
    pub content: Vec<McpContent>,
    /// Whether the tool execution resulted in an error.
    #[serde(default)]
    pub is_error: bool,
}

/// A single content block in an MCP tool result.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum McpContent {
    /// Plain text content.
    #[serde(rename = "text")]
    Text { text: String },
    /// Base64-encoded image content.
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// Embedded resource content.
    #[serde(rename = "resource")]
    Resource { resource: serde_json::Value },
}

/// Configuration for a single MCP server.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    /// Command to spawn the server process (or URL for remote transports).
    pub command: String,
    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables to set for the process.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Transport type ("stdio", "sse", "http", "streamable-http").
    #[serde(default = "default_transport")]
    pub transport: String,
    /// Tool name patterns that are auto-approved for this server.
    #[serde(default)]
    pub auto_approve: Vec<String>,
    /// Authentication headers for remote transports (SSE, HTTP).
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            transport: default_transport(),
            auto_approve: Vec::new(),
            headers: HashMap::new(),
        }
    }
}

/// Default transport type.
fn default_transport() -> String {
    "stdio".to_string()
}

/// MCP settings file structure (`~/.meh/mcp_settings.json`).
#[derive(Debug, Clone, Deserialize)]
pub struct McpSettings {
    /// Map of server name to server configuration.
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!json.contains("\"params\""));
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
        let json =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}"#;
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
        let json =
            r#"{"servers":{"fs":{"command":"node","args":["server.js"],"transport":"stdio"}}}"#;
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
        assert_eq!(config.transport, "stdio");
        assert!(config.args.is_empty());
        assert!(config.env.is_empty());
        assert!(config.auto_approve.is_empty());
        assert!(config.headers.is_empty());
    }

    #[test]
    fn test_headers_config_parsing() {
        let json = r#"{
            "command": "https://api.example.com/mcp",
            "transport": "http",
            "headers": {
                "Authorization": "Bearer token123",
                "X-Custom": "value"
            }
        }"#;
        let config: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.headers.len(), 2);
        assert_eq!(config.headers["Authorization"], "Bearer token123");
    }

    #[test]
    fn test_no_headers_defaults_empty() {
        let json = r#"{"command": "node", "transport": "stdio"}"#;
        let config: McpServerConfig = serde_json::from_str(json).unwrap();
        assert!(config.headers.is_empty());
    }
}
