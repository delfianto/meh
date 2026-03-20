# STEP 22 — MCP SSE + HTTP Transports

## Objective
Add SSE (Server-Sent Events) and StreamableHTTP transports for MCP servers. After this step, the app can connect to remote MCP servers over HTTP.

## Prerequisites
- STEP 21 complete (MCP stdio transport)

## Detailed Instructions

### 22.1 SSE Transport (`src/tool/mcp/transport.rs`)

Add `SseTransport` to the existing transport module.

**SSE Transport Architecture**:
- Server exposes two endpoints:
  - GET `/sse` — SSE endpoint for server-to-client messages
  - POST `/message` — HTTP endpoint for client-to-server messages
- Client connects to SSE endpoint, receives event stream
- Client sends requests via POST to message endpoint
- Responses arrive as SSE events matched by request ID

```rust
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};
use std::collections::HashMap;

pub struct SseTransport {
    base_url: String,
    client: reqwest::Client,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    _sse_task: tokio::task::JoinHandle<()>,
}

impl SseTransport {
    pub async fn new(base_url: &str) -> anyhow::Result<Self> {
        let client = reqwest::Client::new();
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Start SSE listener task
        let sse_url = format!("{base_url}/sse");
        let pending_clone = pending.clone();
        let sse_client = client.clone();

        let sse_task = tokio::spawn(async move {
            Self::sse_listener(sse_client, &sse_url, pending_clone).await;
        });

        Ok(Self {
            base_url: base_url.to_string(),
            client,
            pending,
            _sse_task: sse_task,
        })
    }

    async fn sse_listener(
        client: reqwest::Client,
        url: &str,
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    ) {
        // Connect to SSE endpoint with streaming response
        let response = match client.get(url)
            .header("Accept", "text/event-stream")
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::error!(error = %e, "Failed to connect to SSE endpoint");
                return;
            }
        };

        // Read SSE stream line by line
        // Parse "data:" fields as JSON-RPC responses
        // Route responses to pending map by ID
        //
        // SSE format:
        //   event: message
        //   data: {"jsonrpc":"2.0","id":1,"result":{...}}
        //
        // Handle reconnection on disconnect

        use futures::StreamExt;
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));

                    // Process complete SSE events (double newline separated)
                    while let Some(event_end) = buffer.find("\n\n") {
                        let event = buffer[..event_end].to_string();
                        buffer = buffer[event_end + 2..].to_string();

                        // Extract data field
                        for line in event.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                match serde_json::from_str::<JsonRpcResponse>(data) {
                                    Ok(response) => {
                                        if let Some(id) = response.id {
                                            let mut pending = pending.lock().await;
                                            if let Some(sender) = pending.remove(&id) {
                                                let _ = sender.send(response);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            error = %e,
                                            data = %data,
                                            "Failed to parse SSE data as JSON-RPC"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "SSE stream error");
                    break;
                }
            }
        }
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn send(&self, request: JsonRpcRequest) -> anyhow::Result<JsonRpcResponse> {
        let (tx, rx) = oneshot::channel();

        // Register pending request
        {
            let mut pending = self.pending.lock().await;
            pending.insert(request.id, tx);
        }

        // POST request to message endpoint
        let message_url = format!("{}/message", self.base_url);
        let response = self.client.post(&message_url)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            // Clean up pending
            let mut pending = self.pending.lock().await;
            pending.remove(&request.id);
            anyhow::bail!(
                "MCP SSE POST failed with status: {}",
                response.status()
            );
        }

        // Wait for response via SSE with timeout
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => anyhow::bail!("SSE response channel closed"),
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&request.id);
                anyhow::bail!("MCP SSE request timed out after 30s")
            }
        }
    }

    async fn close(&self) -> anyhow::Result<()> {
        self._sse_task.abort();
        Ok(())
    }
}
```

### 22.2 StreamableHTTP Transport

**StreamableHTTP** is a newer MCP transport where a single HTTP endpoint handles bidirectional communication:
- POST to single endpoint
- Request body: JSON-RPC request
- Response: JSON-RPC response (may be streamed for long-running operations)
- Session tracked via `Mcp-Session-Id` header

```rust
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct HttpTransport {
    url: String,
    client: reqwest::Client,
    session_id: Arc<RwLock<Option<String>>>,
}

impl HttpTransport {
    pub async fn new(url: &str) -> anyhow::Result<Self> {
        Ok(Self {
            url: url.to_string(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()?,
            session_id: Arc::new(RwLock::new(None)),
        })
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send(&self, request: JsonRpcRequest) -> anyhow::Result<JsonRpcResponse> {
        let mut req_builder = self.client.post(&self.url)
            .header("Content-Type", "application/json")
            .json(&request);

        // Include session ID if we have one
        {
            let session_id = self.session_id.read().await;
            if let Some(ref sid) = *session_id {
                req_builder = req_builder.header("Mcp-Session-Id", sid);
            }
        }

        let response = req_builder.send().await?;

        // Check for session ID in response headers
        if let Some(sid) = response.headers().get("Mcp-Session-Id") {
            if let Ok(sid_str) = sid.to_str() {
                let mut session_id = self.session_id.write().await;
                *session_id = Some(sid_str.to_string());
            }
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("MCP HTTP request failed: {} - {}", status, body);
        }

        let rpc_response: JsonRpcResponse = response.json().await?;
        Ok(rpc_response)
    }

    async fn close(&self) -> anyhow::Result<()> {
        // No persistent connection to close for HTTP transport
        // Optionally send a session termination request
        let session_id = self.session_id.read().await;
        if let Some(ref sid) = *session_id {
            // Best-effort DELETE to terminate session
            let _ = self.client.delete(&self.url)
                .header("Mcp-Session-Id", sid)
                .send()
                .await;
        }
        Ok(())
    }
}
```

### 22.3 Update McpClient transport selection

Update `McpClient::connect` in `src/tool/mcp/client.rs`:

```rust
pub async fn connect(server_name: String, config: &McpServerConfig) -> anyhow::Result<Self> {
    let transport: Box<dyn McpTransport> = match config.transport.as_str() {
        "stdio" => Box::new(
            StdioTransport::new(&config.command, &config.args, &config.env).await?
        ),
        "sse" => Box::new(
            SseTransport::new(&config.command).await?
        ),
        "http" | "streamable-http" => Box::new(
            HttpTransport::new(&config.command).await?
        ),
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
```

### 22.4 Authentication support

For remote transports (SSE, HTTP), support authentication via headers:

```rust
/// Extended server config for remote transports.
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
    /// Optional authentication headers for remote transports.
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
}
```

The `headers` field allows users to configure auth:
```json
{
    "servers": {
        "remote-api": {
            "command": "https://api.example.com/mcp",
            "transport": "http",
            "headers": {
                "Authorization": "Bearer ${MCP_API_TOKEN}"
            }
        }
    }
}
```

Environment variable expansion in header values:
```rust
fn expand_env_vars(value: &str) -> String {
    let mut result = value.to_string();
    // Match ${VAR_NAME} patterns
    let re = regex::Regex::new(r"\$\{([^}]+)\}").unwrap();
    for cap in re.captures_iter(value) {
        if let Ok(env_val) = std::env::var(&cap[1]) {
            result = result.replace(&cap[0], &env_val);
        }
    }
    result
}
```

### 22.5 Settings examples

```json
{
    "servers": {
        "local-fs": {
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home"],
            "transport": "stdio"
        },
        "remote-db": {
            "command": "https://mcp.example.com/db",
            "transport": "sse",
            "headers": {
                "Authorization": "Bearer ${DB_MCP_TOKEN}"
            }
        },
        "cloud-api": {
            "command": "https://api.example.com/mcp",
            "transport": "http",
            "headers": {
                "Authorization": "Bearer ${CLOUD_API_KEY}",
                "X-Org-Id": "my-org"
            }
        }
    }
}
```

### 22.6 Error handling and retries

Remote transports should handle transient failures:

```rust
/// Retry configuration for remote transports.
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 5000,
        }
    }
}

/// Retry a fallible async operation with exponential backoff.
async fn retry_with_backoff<F, Fut, T>(
    config: &RetryConfig,
    operation: F,
) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let mut backoff_ms = config.initial_backoff_ms;

    for attempt in 0..=config.max_retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                if attempt == config.max_retries {
                    return Err(e);
                }
                tracing::warn!(
                    attempt = attempt + 1,
                    max_retries = config.max_retries,
                    backoff_ms = backoff_ms,
                    error = %e,
                    "Retrying after transient error"
                );
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(config.max_backoff_ms);
            }
        }
    }
    unreachable!()
}
```

Apply retries to SSE reconnection and HTTP transport sends for connection errors (not application errors).

## Tests

```rust
#[cfg(test)]
mod transport_tests {
    use super::*;

    #[test]
    fn test_transport_selection_stdio() {
        let config = McpServerConfig {
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            env: Default::default(),
            transport: "stdio".to_string(),
            auto_approve: vec![],
            headers: Default::default(),
        };
        assert_eq!(config.transport, "stdio");
    }

    #[test]
    fn test_transport_selection_sse() {
        let config = McpServerConfig {
            command: "https://example.com".to_string(),
            transport: "sse".to_string(),
            ..Default::default()
        };
        assert_eq!(config.transport, "sse");
    }

    #[test]
    fn test_transport_selection_http() {
        let config = McpServerConfig {
            command: "https://api.example.com/mcp".to_string(),
            transport: "http".to_string(),
            ..Default::default()
        };
        assert_eq!(config.transport, "http");
    }

    #[test]
    fn test_transport_selection_streamable_http() {
        let config = McpServerConfig {
            command: "https://api.example.com/mcp".to_string(),
            transport: "streamable-http".to_string(),
            ..Default::default()
        };
        // Both "http" and "streamable-http" map to HttpTransport
        assert!(config.transport == "http" || config.transport == "streamable-http");
    }

    #[test]
    fn test_env_var_expansion() {
        std::env::set_var("TEST_MCP_VAR", "secret123");
        let expanded = expand_env_vars("Bearer ${TEST_MCP_VAR}");
        assert_eq!(expanded, "Bearer secret123");
        std::env::remove_var("TEST_MCP_VAR");
    }

    #[test]
    fn test_env_var_expansion_missing() {
        let expanded = expand_env_vars("Bearer ${NONEXISTENT_VAR_12345}");
        // Missing vars are left as-is
        assert_eq!(expanded, "Bearer ${NONEXISTENT_VAR_12345}");
    }

    #[test]
    fn test_env_var_expansion_multiple() {
        std::env::set_var("MCP_HOST", "example.com");
        std::env::set_var("MCP_PORT", "8080");
        let expanded = expand_env_vars("https://${MCP_HOST}:${MCP_PORT}/mcp");
        assert_eq!(expanded, "https://example.com:8080/mcp");
        std::env::remove_var("MCP_HOST");
        std::env::remove_var("MCP_PORT");
    }

    #[test]
    fn test_retry_config_defaults() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_backoff_ms, 100);
        assert_eq!(config.max_backoff_ms, 5000);
    }

    #[tokio::test]
    async fn test_retry_succeeds_first_attempt() {
        let config = RetryConfig::default();
        let result = retry_with_backoff(&config, || async { Ok::<_, anyhow::Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let config = RetryConfig { max_retries: 3, initial_backoff_ms: 1, max_backoff_ms: 10 };
        let attempt = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = attempt.clone();
        let result = retry_with_backoff(&config, || {
            let attempt = attempt_clone.clone();
            async move {
                let n = attempt.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n < 2 {
                    anyhow::bail!("transient error");
                }
                Ok(42)
            }
        }).await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempt.load(std::sync::atomic::Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let config = RetryConfig { max_retries: 2, initial_backoff_ms: 1, max_backoff_ms: 10 };
        let result = retry_with_backoff(&config, || async {
            Err::<i32, _>(anyhow::anyhow!("permanent error"))
        }).await;
        assert!(result.is_err());
    }

    // Integration tests require running servers
    #[tokio::test]
    #[ignore]
    async fn test_sse_transport_connect() {
        // Would need a running SSE MCP server
    }

    #[tokio::test]
    #[ignore]
    async fn test_http_transport_connect() {
        // Would need a running HTTP MCP server
    }

    #[tokio::test]
    #[ignore]
    async fn test_http_transport_session_tracking() {
        // Verify session ID is captured and reused
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;

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
```

## Acceptance Criteria
- [ ] SSE transport connects to SSE endpoint and POST message endpoint
- [ ] SSE responses correctly routed to pending requests by ID
- [ ] HTTP transport sends/receives JSON-RPC over single endpoint
- [ ] Session ID tracked in HTTP transport via Mcp-Session-Id header
- [ ] Transport selection based on config.transport field
- [ ] Authentication headers configurable per server
- [ ] Environment variable expansion in header values
- [ ] Retry with exponential backoff for transient failures
- [ ] Timeout handling for SSE and HTTP requests (30s default)
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass
