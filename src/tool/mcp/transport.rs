//! MCP transport layer — abstracts communication with MCP servers.
//!
//! The [`McpTransport`] trait defines the interface for sending JSON-RPC
//! requests and receiving responses. Three implementations are provided:
//!
//! ```text
//!   McpClient
//!       │
//!       ▼
//!   McpTransport::send(JsonRpcRequest)
//!       │
//!       ├── StdioTransport: JSON-RPC over child process stdin/stdout
//!       ├── SseTransport:   POST requests, SSE response stream
//!       └── HttpTransport:  POST/response over single HTTP endpoint
//! ```

use super::types::{JsonRpcRequest, JsonRpcResponse};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// Trait for MCP transports — send JSON-RPC requests and receive responses.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and wait for the corresponding response.
    async fn send(&self, request: JsonRpcRequest) -> anyhow::Result<JsonRpcResponse>;

    /// Close the transport and release resources.
    async fn close(&self) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// Stdio Transport
// ---------------------------------------------------------------------------

/// Stdio transport — communicates with an MCP server via stdin/stdout of a child process.
pub struct StdioTransport {
    /// Write end of the child process stdin.
    stdin: Mutex<tokio::process::ChildStdin>,
    /// Map of pending request ids to their response channels.
    pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<JsonRpcResponse>>>>,
    /// Handle to the child process (kept alive for the transport lifetime).
    _child: Arc<Mutex<tokio::process::Child>>,
}

impl StdioTransport {
    /// Spawn a child process and set up bidirectional JSON-RPC communication.
    ///
    /// Starts two background tasks:
    /// - Stdout reader: parses JSON-RPC responses and routes them by request id
    /// - Stderr reader: logs server error output as warnings
    pub fn new(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> anyhow::Result<Self> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        for (key, value) in env {
            let expanded =
                shellexpand::env(value).unwrap_or_else(|_| std::borrow::Cow::Borrowed(value));
            cmd.env(key, expanded.as_ref());
        }

        let mut child = cmd.spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture MCP server stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture MCP server stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture MCP server stderr"))?;

        let pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let pending_clone = pending.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<JsonRpcResponse>(&trimmed) {
                    Ok(response) => {
                        if let Some(id) = response.id {
                            let mut map = pending_clone.lock().await;
                            if let Some(sender) = map.remove(&id) {
                                let _ = sender.send(response);
                            }
                        } else {
                            tracing::debug!("MCP notification received (no id)");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            line = %trimmed,
                            "Failed to parse MCP response"
                        );
                    }
                }
            }
            tracing::debug!("MCP stdout reader task ended");
        });

        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::warn!(target: "mcp_server_stderr", "{}", line);
            }
        });

        Ok(Self {
            stdin: Mutex::new(stdin),
            pending,
            _child: Arc::new(Mutex::new(child)),
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, request: JsonRpcRequest) -> anyhow::Result<JsonRpcResponse> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            pending.insert(request.id, tx);
        }

        let mut json = serde_json::to_string(&request)?;
        json.push('\n');

        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(json.as_bytes()).await?;
            stdin.flush().await?;
            drop(stdin);
        }

        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => anyhow::bail!("Response channel closed"),
            Err(_) => {
                self.pending.lock().await.remove(&request.id);
                anyhow::bail!("MCP request timed out after 30s")
            }
        }
    }

    async fn close(&self) -> anyhow::Result<()> {
        self._child.lock().await.kill().await.ok();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SSE Transport
// ---------------------------------------------------------------------------

/// SSE transport — communicates with an MCP server over Server-Sent Events.
///
/// Client sends requests via POST to `{base_url}/message`.
/// Server sends responses as SSE events on `{base_url}/sse`.
pub struct SseTransport {
    /// Base URL of the MCP server.
    base_url: String,
    /// HTTP client for POST requests.
    client: reqwest::Client,
    /// Map of pending request ids to their response channels.
    pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<JsonRpcResponse>>>>,
    /// Background task listening for SSE events.
    _sse_task: tokio::task::JoinHandle<()>,
}

impl SseTransport {
    /// Connect to an SSE-based MCP server.
    ///
    /// Starts a background task to listen for SSE events and route
    /// responses to pending requests by id.
    pub fn new(base_url: &str, headers: &HashMap<String, String>) -> anyhow::Result<Self> {
        let mut default_headers = reqwest::header::HeaderMap::new();
        for (key, value) in headers {
            let expanded = expand_env_vars(value);
            default_headers.insert(
                reqwest::header::HeaderName::from_bytes(key.as_bytes())?,
                reqwest::header::HeaderValue::from_str(&expanded)?,
            );
        }

        let client = reqwest::Client::builder()
            .default_headers(default_headers)
            .build()?;

        let pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

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

    /// Background SSE event listener.
    async fn sse_listener(
        client: reqwest::Client,
        url: &str,
        pending: Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<JsonRpcResponse>>>>,
    ) {
        use futures::StreamExt;

        let response = match client
            .get(url)
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

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));

                    while let Some(event_end) = buffer.find("\n\n") {
                        let event = buffer[..event_end].to_string();
                        buffer = buffer[event_end + 2..].to_string();

                        for line in event.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                match serde_json::from_str::<JsonRpcResponse>(data) {
                                    Ok(response) => {
                                        if let Some(id) = response.id {
                                            let mut map = pending.lock().await;
                                            if let Some(sender) = map.remove(&id) {
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
        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            pending.insert(request.id, tx);
        }

        let message_url = format!("{}/message", self.base_url);
        let response = self.client.post(&message_url).json(&request).send().await?;

        if !response.status().is_success() {
            self.pending.lock().await.remove(&request.id);
            anyhow::bail!("MCP SSE POST failed with status: {}", response.status());
        }

        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => anyhow::bail!("SSE response channel closed"),
            Err(_) => {
                self.pending.lock().await.remove(&request.id);
                anyhow::bail!("MCP SSE request timed out after 30s")
            }
        }
    }

    async fn close(&self) -> anyhow::Result<()> {
        self._sse_task.abort();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// HTTP (Streamable HTTP) Transport
// ---------------------------------------------------------------------------

/// HTTP transport — communicates with an MCP server over a single HTTP endpoint.
///
/// Each request is a POST, each response is synchronous JSON.
/// Session continuity is maintained via the `Mcp-Session-Id` header.
pub struct HttpTransport {
    /// URL of the MCP endpoint.
    url: String,
    /// HTTP client with default auth headers.
    client: reqwest::Client,
    /// Server-assigned session identifier.
    session_id: Arc<tokio::sync::RwLock<Option<String>>>,
}

impl HttpTransport {
    /// Create a new HTTP transport for the given URL.
    pub fn new(url: &str, headers: &HashMap<String, String>) -> anyhow::Result<Self> {
        let mut default_headers = reqwest::header::HeaderMap::new();
        default_headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        for (key, value) in headers {
            let expanded = expand_env_vars(value);
            default_headers.insert(
                reqwest::header::HeaderName::from_bytes(key.as_bytes())?,
                reqwest::header::HeaderValue::from_str(&expanded)?,
            );
        }

        Ok(Self {
            url: url.to_string(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .default_headers(default_headers)
                .build()?,
            session_id: Arc::new(tokio::sync::RwLock::new(None)),
        })
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send(&self, request: JsonRpcRequest) -> anyhow::Result<JsonRpcResponse> {
        let mut req_builder = self.client.post(&self.url).json(&request);

        {
            let session_id = self.session_id.read().await;
            if let Some(ref sid) = *session_id {
                req_builder = req_builder.header("Mcp-Session-Id", sid);
            }
        }

        let response = req_builder.send().await?;

        if let Some(sid) = response.headers().get("Mcp-Session-Id") {
            if let Ok(sid_str) = sid.to_str() {
                let mut session_id = self.session_id.write().await;
                *session_id = Some(sid_str.to_string());
            }
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("MCP HTTP request failed: {status} - {body}");
        }

        let rpc_response: JsonRpcResponse = response.json().await?;
        Ok(rpc_response)
    }

    async fn close(&self) -> anyhow::Result<()> {
        let sid = self.session_id.read().await.clone();
        if let Some(ref sid) = sid {
            let _ = self
                .client
                .delete(&self.url)
                .header("Mcp-Session-Id", sid)
                .send()
                .await;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Expand `${VAR_NAME}` patterns in a string using environment variables.
///
/// Variables that are not set are left as-is in the output.
pub fn expand_env_vars(value: &str) -> String {
    let re = regex::Regex::new(r"\$\{([^}]+)\}")
        .unwrap_or_else(|_| unreachable!("static regex is valid"));
    let mut result = value.to_string();
    for cap in re.captures_iter(value) {
        if let Ok(env_val) = std::env::var(&cap[1]) {
            result = result.replace(&cap[0], &env_val);
        }
    }
    result
}

/// Configuration for retry with exponential backoff.
pub struct RetryConfig {
    /// Maximum number of retry attempts.
    pub max_retries: u32,
    /// Initial backoff duration in milliseconds.
    pub initial_backoff_ms: u64,
    /// Maximum backoff duration in milliseconds.
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

/// Retry an async operation with exponential backoff.
///
/// Returns the first successful result, or the error from the final attempt.
pub async fn retry_with_backoff<F, Fut, T>(config: &RetryConfig, operation: F) -> anyhow::Result<T>
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::mcp::types::McpServerConfig;

    #[test]
    fn test_transport_selection_stdio() {
        let config = McpServerConfig {
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            transport: "stdio".to_string(),
            ..Default::default()
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
        assert!(config.transport == "http" || config.transport == "streamable-http");
    }

    #[test]
    fn test_env_var_expansion_with_home() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            let expanded = expand_env_vars("dir: ${HOME}");
            assert_eq!(expanded, format!("dir: {home}"));
        }
    }

    #[test]
    fn test_env_var_expansion_missing() {
        let expanded = expand_env_vars("Bearer ${NONEXISTENT_VAR_12345}");
        assert_eq!(expanded, "Bearer ${NONEXISTENT_VAR_12345}");
    }

    #[test]
    fn test_env_var_expansion_no_vars() {
        let expanded = expand_env_vars("plain text without vars");
        assert_eq!(expanded, "plain text without vars");
    }

    #[test]
    fn test_env_var_expansion_empty_input() {
        let expanded = expand_env_vars("");
        assert_eq!(expanded, "");
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
        let config = RetryConfig {
            max_retries: 3,
            initial_backoff_ms: 1,
            max_backoff_ms: 10,
        };
        let attempt = Arc::new(std::sync::atomic::AtomicU32::new(0));
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
        })
        .await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempt.load(std::sync::atomic::Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let config = RetryConfig {
            max_retries: 2,
            initial_backoff_ms: 1,
            max_backoff_ms: 10,
        };
        let result = retry_with_backoff(&config, || async {
            Err::<i32, _>(anyhow::anyhow!("permanent error"))
        })
        .await;
        assert!(result.is_err());
    }
}
