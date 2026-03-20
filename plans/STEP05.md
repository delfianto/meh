# STEP 05 — Provider Trait + Anthropic Implementation (Streaming)

## Objective
Implement the `Provider` trait abstraction and the Anthropic (Claude) provider with full streaming support. After this step, the app can make streaming API calls to Claude and receive text, thinking, and tool call chunks.

## Prerequisites
- STEP 01-04 complete
- An `ANTHROPIC_API_KEY` environment variable for manual/integration testing

## Additional Dependencies
Add these to `Cargo.toml` if not already present:
```toml
async-stream = "0.3"
tokio-util = { version = "0.7", features = ["rt"] }
```

## Detailed Instructions

### 5.1 Define core streaming types (`src/provider/mod.rs`)

```rust
//! LLM provider abstraction and implementations.

pub mod anthropic;
pub mod openai;
pub mod gemini;
pub mod openrouter;
pub mod common;

use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;
use serde::{Deserialize, Serialize};

/// A chunk of streaming output from a provider.
#[derive(Debug, Clone)]
pub enum StreamChunk {
    /// Incremental text content.
    Text { delta: String },

    /// Reasoning/thinking content from extended thinking.
    Thinking {
        delta: String,
        /// Thinking signature for multi-turn verification.
        signature: Option<String>,
        /// Whether this block was redacted by the API.
        redacted: bool,
    },

    /// Incremental tool call (partial JSON argument fragment).
    ToolCallDelta {
        id: String,
        name: String,
        arguments_delta: String,
    },

    /// A fully-parsed tool call (emitted on content_block_stop).
    ToolCallComplete {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },

    /// Token usage information (emitted near end of stream).
    Usage(UsageInfo),

    /// Stream is done — no more chunks will be sent.
    Done,

    /// An error occurred during streaming.
    Error(String),
}

/// Token usage and cost information for a single API call.
#[derive(Debug, Clone, Default)]
pub struct UsageInfo {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub thinking_tokens: Option<u64>,
    pub total_cost: Option<f64>,
}

/// Boxed async stream of chunks. This is the return type of `Provider::create_message`.
pub type ProviderStream = Pin<Box<dyn Stream<Item = anyhow::Result<StreamChunk>> + Send>>;

/// Metadata about a model (capabilities, pricing, limits).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub max_tokens: u32,
    pub context_window: u32,
    pub supports_tools: bool,
    pub supports_thinking: bool,
    pub supports_images: bool,
    /// Price per million input tokens (USD).
    pub input_price_per_mtok: f64,
    /// Price per million output tokens (USD).
    pub output_price_per_mtok: f64,
}

/// Configuration passed to `Provider::create_message`.
#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub model_id: String,
    pub max_tokens: u32,
    pub temperature: Option<f64>,
    /// If set, enables extended thinking with this token budget.
    pub thinking_budget: Option<u32>,
}

/// Tool definition sent to the provider.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// A message in the conversation history.
#[derive(Debug, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
}

/// Message role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
}

/// A content block within a message.
#[derive(Debug, Clone)]
pub enum ContentBlock {
    /// Plain text content.
    Text(String),
    /// Thinking/reasoning content (for multi-turn thinking continuity).
    Thinking {
        text: String,
        signature: Option<String>,
    },
    /// A tool use request from the assistant.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// A tool result from the user.
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// An image (base64 encoded).
    Image {
        media_type: String,
        data: Vec<u8>,
    },
}

/// The core provider trait. All LLM providers implement this.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Stream a response from the model.
    ///
    /// Returns a stream of `StreamChunk`s. The caller should consume
    /// the stream until `StreamChunk::Done` is received.
    async fn create_message(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> anyhow::Result<ProviderStream>;

    /// Get model metadata (capabilities, pricing).
    fn model_info(&self) -> &ModelInfo;

    /// Abort an in-flight request.
    /// This cancels the current streaming response.
    fn abort(&self);
}

/// Factory function: creates a provider instance by name.
///
/// # Errors
/// Returns an error if the provider name is unknown or if
/// the provider fails to initialize (e.g., empty API key).
pub fn create_provider(
    provider_name: &str,
    api_key: &str,
    base_url: Option<&str>,
) -> anyhow::Result<Box<dyn Provider>> {
    match provider_name {
        "anthropic" => Ok(Box::new(anthropic::AnthropicProvider::new(
            api_key, base_url,
        )?)),
        // Other providers will be added in later steps:
        // "openai" => Ok(Box::new(openai::OpenAiProvider::new(api_key, base_url)?)),
        // "gemini" => Ok(Box::new(gemini::GeminiProvider::new(api_key, base_url)?)),
        // "openrouter" => Ok(Box::new(openrouter::OpenRouterProvider::new(api_key, base_url)?)),
        _ => anyhow::bail!("Unknown provider: {provider_name}"),
    }
}
```

### 5.2 Common HTTP utilities (`src/provider/common.rs`)

```rust
//! Shared HTTP client, retry logic, and error types.

use reqwest::Client;
use std::time::Duration;
use thiserror::Error;

/// Errors specific to provider operations.
#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Rate limited: retry after {retry_after:?}")]
    RateLimit { retry_after: Option<Duration> },

    #[error("Server error ({status}): {message}")]
    Server { status: u16, message: String },

    #[error("Request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}

impl ProviderError {
    /// Whether this error is retriable (rate limits and 5xx errors).
    pub fn is_retriable(&self) -> bool {
        match self {
            Self::RateLimit { .. } => true,
            Self::Server { status, .. } => *status >= 500,
            _ => false,
        }
    }
}

/// Create a configured HTTP client with sensible timeouts.
pub fn create_http_client() -> reqwest::Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(300))      // 5 min total timeout
        .connect_timeout(Duration::from_secs(10)) // 10s connect timeout
        .pool_max_idle_per_host(5)
        .build()
}

/// Retry an async operation with exponential backoff.
///
/// - On retriable errors (rate limit, 5xx), waits and retries up to `max_retries` times.
/// - On non-retriable errors, returns immediately.
/// - For rate limits with a `retry_after` header, uses that duration.
/// - Otherwise, uses exponential backoff: 1s, 2s, 4s, ...
pub async fn with_retry<F, Fut, T>(max_retries: u32, f: F) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let mut last_err = None;
    for attempt in 0..=max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                if let Some(pe) = e.downcast_ref::<ProviderError>() {
                    if !pe.is_retriable() || attempt == max_retries {
                        return Err(e);
                    }
                    let delay = match pe {
                        ProviderError::RateLimit {
                            retry_after: Some(d),
                        } => *d,
                        _ => Duration::from_millis(1000 * 2u64.pow(attempt)),
                    };
                    tracing::warn!(attempt, ?delay, "Retriable error, backing off");
                    tokio::time::sleep(delay).await;
                } else if attempt == max_retries {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Retry exhausted")))
}
```

### 5.3 Anthropic Provider (`src/provider/anthropic.rs`)

This is the most complex piece. The provider must:
1. Build the correct API request with system prompt caching, thinking config, and tools
2. Send it as an SSE streaming request
3. Parse each SSE event type and emit the correct `StreamChunk`
4. Support cancellation via `tokio_util::sync::CancellationToken`

```rust
//! Anthropic (Claude) provider implementation.

use super::common::*;
use super::*;
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    pub(crate) base_url: String,
    model_info: ModelInfo,
    cancel: CancellationToken,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider.
    ///
    /// # Errors
    /// Returns an error if the API key is empty.
    pub fn new(api_key: &str, base_url: Option<&str>) -> anyhow::Result<Self> {
        anyhow::ensure!(!api_key.is_empty(), "Anthropic API key is required");
        Ok(Self {
            client: create_http_client()?,
            api_key: api_key.to_string(),
            base_url: base_url.unwrap_or(DEFAULT_BASE_URL).to_string(),
            model_info: ModelInfo {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude Sonnet 4".to_string(),
                provider: "anthropic".to_string(),
                max_tokens: 8192,
                context_window: 200_000,
                supports_tools: true,
                supports_thinking: true,
                supports_images: true,
                input_price_per_mtok: 3.0,
                output_price_per_mtok: 15.0,
            },
            cancel: CancellationToken::new(),
        })
    }
}
```

#### API Request Types (private to module)

```rust
#[derive(Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    system: Vec<SystemBlock>,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiTool>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
}

#[derive(Serialize)]
struct SystemBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    control_type: String,
}

#[derive(Serialize)]
struct ThinkingConfig {
    #[serde(rename = "type")]
    thinking_type: String,
    budget_tokens: u32,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: Vec<ApiContentBlock>,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum ApiContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Serialize)]
struct ApiTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}
```

#### SSE Event Response Types (private, for deserialization)

```rust
#[derive(Deserialize)]
struct MessageStartEvent {
    message: MessageStartData,
}

#[derive(Deserialize)]
struct MessageStartData {
    usage: Option<MessageUsage>,
}

#[derive(Deserialize)]
struct MessageUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct ContentBlockStart {
    content_block: ContentBlockData,
}

#[derive(Deserialize)]
struct ContentBlockData {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlockDelta {
    delta: DeltaData,
}

#[derive(Deserialize)]
struct DeltaData {
    #[serde(rename = "type")]
    delta_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
    #[serde(default)]
    signature: Option<String>,
}

#[derive(Deserialize)]
struct MessageDelta {
    delta: MessageDeltaInner,
    usage: Option<MessageUsage>,
}

#[derive(Deserialize)]
struct MessageDeltaInner {
    stop_reason: Option<String>,
}
```

#### Conversion Functions

```rust
/// Convert internal Message list to Anthropic API format.
fn convert_messages(messages: &[Message]) -> Vec<ApiMessage> {
    messages
        .iter()
        .map(|msg| {
            let role = match msg.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
            };
            let content = msg
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::Text(text) => ApiContentBlock::Text {
                        text: text.clone(),
                    },
                    ContentBlock::Thinking { text, signature } => ApiContentBlock::Thinking {
                        thinking: text.clone(),
                        signature: signature.clone(),
                    },
                    ContentBlock::ToolUse { id, name, input } => ApiContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => ApiContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: content.clone(),
                        is_error: if *is_error { Some(true) } else { None },
                    },
                    ContentBlock::Image { .. } => {
                        // Image handling will be implemented later
                        ApiContentBlock::Text {
                            text: "[image]".to_string(),
                        }
                    }
                })
                .collect();
            ApiMessage {
                role: role.to_string(),
                content,
            }
        })
        .collect()
}

/// Convert tool definitions to Anthropic API format.
fn convert_tools(tools: &[ToolDefinition]) -> Vec<ApiTool> {
    tools
        .iter()
        .map(|t| ApiTool {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.input_schema.clone(),
        })
        .collect()
}
```

#### Provider Trait Implementation

```rust
#[async_trait]
impl Provider for AnthropicProvider {
    async fn create_message(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> anyhow::Result<ProviderStream> {
        let cancel = self.cancel.clone();

        // Build thinking config (temperature must be None when thinking is enabled)
        let thinking = config.thinking_budget.map(|budget| ThinkingConfig {
            thinking_type: "enabled".to_string(),
            budget_tokens: budget,
        });

        let request = ApiRequest {
            model: config.model_id.clone(),
            max_tokens: config.max_tokens,
            temperature: if thinking.is_some() {
                None
            } else {
                config.temperature
            },
            system: vec![SystemBlock {
                block_type: "text".to_string(),
                text: system_prompt.to_string(),
                cache_control: Some(CacheControl {
                    control_type: "ephemeral".to_string(),
                }),
            }],
            messages: convert_messages(messages),
            tools: convert_tools(tools),
            stream: true,
            thinking,
        };

        let url = format!("{}/v1/messages", self.base_url);

        let request_builder = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&request);

        let mut es = EventSource::new(request_builder)?;

        // Return a stream that processes SSE events and yields StreamChunks
        let stream = async_stream::stream! {
            let mut current_tool_id = String::new();
            let mut current_tool_name = String::new();
            let mut current_tool_args = String::new();
            let mut total_input = 0u64;
            let mut total_output = 0u64;
            let mut cache_read = 0u64;
            let mut cache_write = 0u64;

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        es.close();
                        break;
                    }
                    event = es.next() => {
                        match event {
                            Some(Ok(Event::Open)) => {
                                // SSE connection opened successfully
                            }
                            Some(Ok(Event::Message(msg))) => {
                                match msg.event.as_str() {
                                    "message_start" => {
                                        if let Ok(data) = serde_json::from_str::<MessageStartEvent>(&msg.data) {
                                            if let Some(usage) = data.message.usage {
                                                total_input += usage.input_tokens.unwrap_or(0);
                                                cache_read += usage.cache_read_input_tokens.unwrap_or(0);
                                                cache_write += usage.cache_creation_input_tokens.unwrap_or(0);
                                            }
                                        }
                                    }
                                    "content_block_start" => {
                                        if let Ok(data) = serde_json::from_str::<ContentBlockStart>(&msg.data) {
                                            if data.content_block.block_type == "tool_use" {
                                                current_tool_id = data.content_block.id.unwrap_or_default();
                                                current_tool_name = data.content_block.name.unwrap_or_default();
                                                current_tool_args.clear();
                                            }
                                        }
                                    }
                                    "content_block_delta" => {
                                        if let Ok(data) = serde_json::from_str::<ContentBlockDelta>(&msg.data) {
                                            match data.delta.delta_type.as_str() {
                                                "text_delta" => {
                                                    if let Some(text) = data.delta.text {
                                                        yield Ok(StreamChunk::Text { delta: text });
                                                    }
                                                }
                                                "thinking_delta" => {
                                                    if let Some(thinking) = data.delta.thinking {
                                                        yield Ok(StreamChunk::Thinking {
                                                            delta: thinking,
                                                            signature: None,
                                                            redacted: false,
                                                        });
                                                    }
                                                }
                                                "signature_delta" => {
                                                    if let Some(sig) = data.delta.signature {
                                                        yield Ok(StreamChunk::Thinking {
                                                            delta: String::new(),
                                                            signature: Some(sig),
                                                            redacted: false,
                                                        });
                                                    }
                                                }
                                                "input_json_delta" => {
                                                    if let Some(json) = data.delta.partial_json {
                                                        current_tool_args.push_str(&json);
                                                        yield Ok(StreamChunk::ToolCallDelta {
                                                            id: current_tool_id.clone(),
                                                            name: current_tool_name.clone(),
                                                            arguments_delta: json,
                                                        });
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    "content_block_stop" => {
                                        // If we were building a tool call, finalize it
                                        if !current_tool_name.is_empty() {
                                            if let Ok(args) = serde_json::from_str(&current_tool_args) {
                                                yield Ok(StreamChunk::ToolCallComplete {
                                                    id: current_tool_id.clone(),
                                                    name: current_tool_name.clone(),
                                                    arguments: args,
                                                });
                                            }
                                            current_tool_name.clear();
                                            current_tool_args.clear();
                                        }
                                    }
                                    "message_delta" => {
                                        if let Ok(data) = serde_json::from_str::<MessageDelta>(&msg.data) {
                                            if let Some(usage) = data.usage {
                                                total_output += usage.output_tokens.unwrap_or(0);
                                            }
                                        }
                                    }
                                    "message_stop" => {
                                        yield Ok(StreamChunk::Usage(UsageInfo {
                                            input_tokens: total_input,
                                            output_tokens: total_output,
                                            cache_read_tokens: Some(cache_read),
                                            cache_write_tokens: Some(cache_write),
                                            thinking_tokens: None,
                                            total_cost: None, // Calculated by caller
                                        }));
                                        yield Ok(StreamChunk::Done);
                                        es.close();
                                        break;
                                    }
                                    "error" => {
                                        yield Err(anyhow::anyhow!("Anthropic stream error: {}", msg.data));
                                        es.close();
                                        break;
                                    }
                                    _ => {
                                        // Ignore unknown event types (forward compatibility)
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                yield Err(anyhow::anyhow!("SSE error: {e}"));
                                break;
                            }
                            None => {
                                // Stream ended without message_stop
                                yield Ok(StreamChunk::Done);
                                break;
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.model_info
    }

    fn abort(&self) {
        self.cancel.cancel();
    }
}
```

### 5.4 Event Type Reference

For reference, the Anthropic SSE event types and their sequence:

```
message_start          → Contains input token usage
  content_block_start  → type: "text" | "thinking" | "tool_use"
    content_block_delta → type: "text_delta" | "thinking_delta" | "signature_delta" | "input_json_delta"
    ... (more deltas)
  content_block_stop
  ... (more blocks)
message_delta          → Contains output token usage, stop_reason
message_stop           → Stream is complete
```

Key behaviors:
- **Thinking blocks** come first (if enabled), then text and/or tool_use blocks
- **Tool calls** arrive as: `content_block_start` (type=tool_use, with id and name) -> `content_block_delta` (input_json_delta, partial JSON) -> `content_block_stop`
- **Temperature must be omitted** when thinking is enabled (API requirement)
- **Cache control** on the system prompt enables prompt caching
- **Signature deltas** are used for multi-turn thinking verification

## Tests

### StreamChunk and type tests
```rust
#[cfg(test)]
mod stream_chunk_tests {
    use super::*;

    #[test]
    fn test_stream_chunk_text() {
        let chunk = StreamChunk::Text {
            delta: "hello".to_string(),
        };
        assert!(matches!(chunk, StreamChunk::Text { delta } if delta == "hello"));
    }

    #[test]
    fn test_stream_chunk_thinking() {
        let chunk = StreamChunk::Thinking {
            delta: "reasoning".to_string(),
            signature: Some("sig".to_string()),
            redacted: false,
        };
        assert!(
            matches!(chunk, StreamChunk::Thinking { delta, signature, redacted }
                if delta == "reasoning" && signature == Some("sig".to_string()) && !redacted)
        );
    }

    #[test]
    fn test_stream_chunk_tool_call_complete() {
        let chunk = StreamChunk::ToolCallComplete {
            id: "tc-1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/main.rs"}),
        };
        assert!(matches!(chunk, StreamChunk::ToolCallComplete { name, .. } if name == "read_file"));
    }

    #[test]
    fn test_usage_info_default() {
        let usage = UsageInfo::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert!(usage.total_cost.is_none());
        assert!(usage.cache_read_tokens.is_none());
    }

    #[test]
    fn test_model_config_creation() {
        let config = ModelConfig {
            model_id: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 8192,
            temperature: Some(0.7),
            thinking_budget: Some(10000),
        };
        assert_eq!(config.max_tokens, 8192);
        assert_eq!(config.thinking_budget, Some(10000));
    }

    #[test]
    fn test_model_info_fields() {
        let info = ModelInfo {
            id: "test".to_string(),
            name: "Test".to_string(),
            provider: "test".to_string(),
            max_tokens: 4096,
            context_window: 100_000,
            supports_tools: true,
            supports_thinking: false,
            supports_images: true,
            input_price_per_mtok: 1.0,
            output_price_per_mtok: 5.0,
        };
        assert!(info.supports_tools);
        assert!(!info.supports_thinking);
    }
}
```

### Anthropic provider unit tests
```rust
#[cfg(test)]
mod anthropic_tests {
    use super::anthropic::*;
    use super::*;

    #[test]
    fn test_anthropic_provider_rejects_empty_key() {
        let result = AnthropicProvider::new("", None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("API key is required"));
    }

    #[test]
    fn test_anthropic_provider_creation() {
        let provider = AnthropicProvider::new("test-key", None).unwrap();
        assert_eq!(provider.model_info().provider, "anthropic");
        assert!(provider.model_info().supports_tools);
        assert!(provider.model_info().supports_thinking);
        assert_eq!(provider.model_info().context_window, 200_000);
    }

    #[test]
    fn test_anthropic_custom_base_url() {
        let provider =
            AnthropicProvider::new("test-key", Some("https://custom.api.com")).unwrap();
        assert_eq!(provider.base_url, "https://custom.api.com");
    }

    #[test]
    fn test_anthropic_default_base_url() {
        let provider = AnthropicProvider::new("test-key", None).unwrap();
        assert_eq!(provider.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn test_convert_tools() {
        let tools = vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file from disk".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
        }];
        let api_tools = convert_tools(&tools);
        assert_eq!(api_tools.len(), 1);
        assert_eq!(api_tools[0].name, "read_file");
        assert_eq!(api_tools[0].description, "Read a file from disk");
    }

    #[test]
    fn test_convert_empty_tools() {
        let api_tools = convert_tools(&[]);
        assert!(api_tools.is_empty());
    }

    #[test]
    fn test_message_conversion_user() {
        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text("hello".to_string())],
        }];
        let api_messages = convert_messages(&messages);
        assert_eq!(api_messages.len(), 1);
        assert_eq!(api_messages[0].role, "user");
    }

    #[test]
    fn test_message_conversion_assistant_with_tool() {
        let messages = vec![Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Text("Let me read that file.".to_string()),
                ContentBlock::ToolUse {
                    id: "tc-1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "/main.rs"}),
                },
            ],
        }];
        let api_messages = convert_messages(&messages);
        assert_eq!(api_messages.len(), 1);
        assert_eq!(api_messages[0].role, "assistant");
        assert_eq!(api_messages[0].content.len(), 2);
    }

    #[test]
    fn test_message_conversion_tool_result() {
        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tc-1".to_string(),
                content: "fn main() {}".to_string(),
                is_error: false,
            }],
        }];
        let api_messages = convert_messages(&messages);
        assert_eq!(api_messages[0].role, "user");
    }

    #[test]
    fn test_message_conversion_thinking() {
        let messages = vec![Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "Let me reason...".to_string(),
                    signature: Some("sig123".to_string()),
                },
                ContentBlock::Text("Here's my answer.".to_string()),
            ],
        }];
        let api_messages = convert_messages(&messages);
        assert_eq!(api_messages[0].content.len(), 2);
    }

    #[test]
    fn test_api_request_serialization() {
        let request = ApiRequest {
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 8192,
            temperature: None,
            system: vec![SystemBlock {
                block_type: "text".to_string(),
                text: "You are helpful.".to_string(),
                cache_control: Some(CacheControl {
                    control_type: "ephemeral".to_string(),
                }),
            }],
            messages: vec![],
            tools: vec![],
            stream: true,
            thinking: Some(ThinkingConfig {
                thinking_type: "enabled".to_string(),
                budget_tokens: 10000,
            }),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["model"], "claude-sonnet-4-20250514");
        assert_eq!(json["thinking"]["budget_tokens"], 10000);
        assert!(json["stream"].as_bool().unwrap());
    }

    #[test]
    fn test_api_request_without_thinking() {
        let request = ApiRequest {
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 4096,
            temperature: Some(0.7),
            system: vec![],
            messages: vec![],
            tools: vec![],
            stream: true,
            thinking: None,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert!(json["thinking"].is_null());
        assert_eq!(json["temperature"], 0.7);
    }

    #[test]
    fn test_sse_event_parsing_message_start() {
        let data = r#"{"message":{"usage":{"input_tokens":100,"output_tokens":0,"cache_read_input_tokens":50}}}"#;
        let parsed: MessageStartEvent = serde_json::from_str(data).unwrap();
        let usage = parsed.message.usage.unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.cache_read_input_tokens, Some(50));
    }

    #[test]
    fn test_sse_event_parsing_content_block_start_text() {
        let data = r#"{"content_block":{"type":"text"}}"#;
        let parsed: ContentBlockStart = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.content_block.block_type, "text");
    }

    #[test]
    fn test_sse_event_parsing_content_block_start_tool() {
        let data = r#"{"content_block":{"type":"tool_use","id":"toolu_123","name":"read_file"}}"#;
        let parsed: ContentBlockStart = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.content_block.block_type, "tool_use");
        assert_eq!(parsed.content_block.id, Some("toolu_123".to_string()));
        assert_eq!(parsed.content_block.name, Some("read_file".to_string()));
    }

    #[test]
    fn test_sse_event_parsing_text_delta() {
        let data = r#"{"delta":{"type":"text_delta","text":"Hello"}}"#;
        let parsed: ContentBlockDelta = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.delta.delta_type, "text_delta");
        assert_eq!(parsed.delta.text, Some("Hello".to_string()));
    }

    #[test]
    fn test_sse_event_parsing_thinking_delta() {
        let data = r#"{"delta":{"type":"thinking_delta","thinking":"Let me think..."}}"#;
        let parsed: ContentBlockDelta = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.delta.delta_type, "thinking_delta");
        assert_eq!(
            parsed.delta.thinking,
            Some("Let me think...".to_string())
        );
    }

    #[test]
    fn test_sse_event_parsing_signature_delta() {
        let data = r#"{"delta":{"type":"signature_delta","signature":"abc123"}}"#;
        let parsed: ContentBlockDelta = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.delta.delta_type, "signature_delta");
        assert_eq!(parsed.delta.signature, Some("abc123".to_string()));
    }

    #[test]
    fn test_sse_event_parsing_input_json_delta() {
        let data = r#"{"delta":{"type":"input_json_delta","partial_json":"{\"path\":\""}}"#;
        let parsed: ContentBlockDelta = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.delta.delta_type, "input_json_delta");
        assert!(parsed.delta.partial_json.is_some());
    }

    #[test]
    fn test_sse_event_parsing_message_delta() {
        let data = r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#;
        let parsed: MessageDelta = serde_json::from_str(data).unwrap();
        assert_eq!(
            parsed.delta.stop_reason,
            Some("end_turn".to_string())
        );
        assert_eq!(parsed.usage.unwrap().output_tokens, Some(42));
    }
}
```

### Common module tests
```rust
#[cfg(test)]
mod common_tests {
    use super::common::*;
    use std::time::Duration;

    #[test]
    fn test_provider_error_retriable_rate_limit() {
        assert!(ProviderError::RateLimit { retry_after: None }.is_retriable());
        assert!(ProviderError::RateLimit {
            retry_after: Some(Duration::from_secs(5))
        }
        .is_retriable());
    }

    #[test]
    fn test_provider_error_retriable_server() {
        assert!(ProviderError::Server {
            status: 500,
            message: "error".to_string()
        }
        .is_retriable());
        assert!(ProviderError::Server {
            status: 503,
            message: "unavailable".to_string()
        }
        .is_retriable());
    }

    #[test]
    fn test_provider_error_not_retriable() {
        assert!(!ProviderError::Auth("bad key".to_string()).is_retriable());
        assert!(!ProviderError::Server {
            status: 400,
            message: "bad request".to_string()
        }
        .is_retriable());
        assert!(!ProviderError::Server {
            status: 404,
            message: "not found".to_string()
        }
        .is_retriable());
        assert!(!ProviderError::InvalidResponse("bad".to_string()).is_retriable());
    }

    #[test]
    fn test_provider_error_display() {
        let err = ProviderError::Auth("invalid key".to_string());
        assert_eq!(err.to_string(), "Authentication failed: invalid key");

        let err = ProviderError::RateLimit {
            retry_after: Some(Duration::from_secs(30)),
        };
        assert!(err.to_string().contains("Rate limited"));
    }

    #[tokio::test]
    async fn test_with_retry_succeeds_first_try() {
        let result = with_retry(3, || async { Ok::<_, anyhow::Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_with_retry_fails_non_retriable() {
        let result = with_retry(3, || async {
            Err::<i32, _>(
                ProviderError::Auth("bad".to_string()).into(),
            )
        })
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_with_retry_fails_after_max() {
        let result = with_retry(1, || async {
            Err::<i32, _>(
                ProviderError::Server {
                    status: 500,
                    message: "fail".to_string(),
                }
                .into(),
            )
        })
        .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_create_http_client() {
        let client = create_http_client();
        assert!(client.is_ok());
    }
}
```

### Factory function tests
```rust
#[cfg(test)]
mod factory_tests {
    use super::*;

    #[test]
    fn test_create_provider_anthropic() {
        let provider = create_provider("anthropic", "test-key", None);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().model_info().provider, "anthropic");
    }

    #[test]
    fn test_create_provider_unknown() {
        let provider = create_provider("unknown", "key", None);
        assert!(provider.is_err());
        assert!(provider.unwrap_err().to_string().contains("Unknown provider"));
    }

    #[test]
    fn test_create_provider_empty_key() {
        let provider = create_provider("anthropic", "", None);
        assert!(provider.is_err());
    }
}
```

### Integration test (requires API key, marked `#[ignore]`)
```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    use futures::StreamExt;

    /// Test actual streaming against the Anthropic API.
    /// Run with: cargo test -- --ignored test_anthropic_streaming
    #[tokio::test]
    #[ignore]
    async fn test_anthropic_streaming() {
        let api_key =
            std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY not set");
        let provider = anthropic::AnthropicProvider::new(&api_key, None).unwrap();

        let config = ModelConfig {
            model_id: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 100,
            temperature: Some(0.0),
            thinking_budget: None,
        };

        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text(
                "Say 'hello' and nothing else.".to_string(),
            )],
        }];

        let mut stream = provider
            .create_message("You are a test assistant.", &messages, &[], &config)
            .await
            .unwrap();

        let mut got_text = false;
        let mut got_done = false;
        let mut got_usage = false;

        while let Some(chunk) = stream.next().await {
            match chunk.unwrap() {
                StreamChunk::Text { delta } => {
                    assert!(!delta.is_empty());
                    got_text = true;
                }
                StreamChunk::Usage(usage) => {
                    assert!(usage.input_tokens > 0);
                    got_usage = true;
                }
                StreamChunk::Done => {
                    got_done = true;
                }
                _ => {}
            }
        }
        assert!(got_text, "Should have received text chunks");
        assert!(got_done, "Should have received Done");
        assert!(got_usage, "Should have received Usage");
    }

    /// Test streaming with extended thinking enabled.
    /// Run with: cargo test -- --ignored test_anthropic_thinking
    #[tokio::test]
    #[ignore]
    async fn test_anthropic_thinking() {
        let api_key =
            std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY not set");
        let provider = anthropic::AnthropicProvider::new(&api_key, None).unwrap();

        let config = ModelConfig {
            model_id: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 16000,
            temperature: None, // Must be None with thinking
            thinking_budget: Some(10000),
        };

        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text(
                "What is 2 + 2? Think step by step.".to_string(),
            )],
        }];

        let mut stream = provider
            .create_message("You are a test assistant.", &messages, &[], &config)
            .await
            .unwrap();

        let mut got_thinking = false;
        let mut got_text = false;

        while let Some(chunk) = stream.next().await {
            match chunk.unwrap() {
                StreamChunk::Thinking { delta, .. } => {
                    if !delta.is_empty() {
                        got_thinking = true;
                    }
                }
                StreamChunk::Text { .. } => {
                    got_text = true;
                }
                StreamChunk::Done => break,
                _ => {}
            }
        }
        assert!(got_thinking, "Should have received thinking chunks");
        assert!(got_text, "Should have received text chunks");
    }
}
```

## Acceptance Criteria
- [x] `Provider` trait defined with `create_message`, `model_info`, `abort`
- [x] `StreamChunk` enum covers: Text, Thinking, ToolCallDelta, ToolCallComplete, Usage, Done, Error
- [x] `UsageInfo` tracks input/output tokens and cache stats
- [x] `ModelInfo` stores model capabilities and pricing
- [x] `AnthropicProvider` connects to API and streams SSE responses
- [x] SSE events correctly parsed: message_start, content_block_start/delta/stop, message_delta, message_stop
- [x] Thinking chunks (thinking_delta, signature_delta) correctly emitted
- [x] Tool call chunks incrementally emitted via ToolCallDelta, then ToolCallComplete on content_block_stop
- [x] Usage tokens tracked across message_start and message_delta events
- [x] Cancellation works via `CancellationToken` (abort closes the SSE stream)
- [x] Temperature is automatically omitted when thinking is enabled
- [x] System prompt uses ephemeral cache control for prompt caching
- [x] `ProviderError` correctly identifies retriable errors (429, 5xx)
- [x] Retry logic with exponential backoff for retriable errors
- [x] Factory function `create_provider` creates the correct provider
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes all unit tests
- [x] `cargo test -- --ignored` passes integration tests (with API key)

**Completed**: PR #2
