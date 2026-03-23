//! MCP transport layer — abstracts communication with MCP servers.
//!
//! The [`McpTransport`] trait defines the interface for sending JSON-RPC
//! requests and receiving responses. [`StdioTransport`] implements this
//! by spawning a child process and communicating over stdin/stdout.
//!
//! ```text
//!   McpClient
//!       │
//!       ▼
//!   McpTransport::send(JsonRpcRequest)
//!       │
//!       ├── StdioTransport: write JSON line to stdin
//!       │     └── background task reads stdout, routes by request id
//!       │
//!       └── (SSE transport in STEP 22)
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
