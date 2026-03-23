//! LLM provider abstraction and implementations.
//!
//! All providers implement a common `Provider` trait that returns a
//! `Stream<Item = Result<StreamChunk>>`. This normalizes the differences
//! between provider APIs behind a uniform interface.
//!
//! ```text
//!   Agent
//!     │
//!     ▼
//!   dyn Provider::create_message()
//!     │
//!     ├── AnthropicProvider  ──► Messages API (SSE)
//!     ├── OpenAiProvider     ──► Chat Completions / Responses API
//!     ├── GeminiProvider     ──► streamGenerateContent (SSE)
//!     └── OpenRouterProvider ──► OpenAI-compatible API + extras
//!     │
//!     ▼
//!   Stream<Item = Result<StreamChunk>>
//!     │
//!     ├── StreamChunk::Text { delta }
//!     ├── StreamChunk::Thinking { delta, signature }
//!     ├── StreamChunk::ToolCallDelta { id, name, arguments_delta }
//!     ├── StreamChunk::Usage(UsageInfo)
//!     └── StreamChunk::Done
//! ```
//!
//! The `common` module provides shared HTTP client setup, retry logic
//! with exponential backoff, and error types used across all providers.

pub mod anthropic;
pub mod common;
pub mod gemini;
pub mod openai;
pub mod openrouter;
pub mod resolve;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

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

    /// A fully-parsed tool call (emitted on `content_block_stop`).
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

/// Boxed async stream of chunks returned by [`Provider::create_message`].
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
    /// Price per million cache-read tokens (USD). Provider-specific.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_read_price_per_mtok: Option<f64>,
    /// Price per million cache-write tokens (USD). Provider-specific.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_write_price_per_mtok: Option<f64>,
    /// Price per million thinking/reasoning tokens (USD). Falls back to output price.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thinking_price_per_mtok: Option<f64>,
}

/// Configuration passed to [`Provider::create_message`].
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
    Image { media_type: String, data: Vec<u8> },
}

/// The core provider trait. All LLM providers implement this.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Stream a response from the model.
    ///
    /// Returns a stream of [`StreamChunk`]s. The caller should consume
    /// the stream until [`StreamChunk::Done`] is received.
    async fn create_message(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> anyhow::Result<ProviderStream>;

    /// Get model metadata (capabilities, pricing).
    fn model_info(&self) -> &ModelInfo;

    /// Abort an in-flight request by cancelling the current streaming response.
    fn abort(&self);
}

/// Creates a provider instance by name.
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
        "openai" => Ok(Box::new(openai::OpenAiProvider::new(api_key, base_url)?)),
        "gemini" => Ok(Box::new(gemini::GeminiProvider::new(api_key, base_url)?)),
        "openrouter" => Ok(Box::new(openrouter::OpenRouterProvider::new(
            api_key, base_url,
        )?)),
        _ => anyhow::bail!("Unknown provider: {provider_name}"),
    }
}

#[cfg(test)]
mod stream_chunk_tests {
    use super::*;

    #[test]
    fn stream_chunk_text() {
        let chunk = StreamChunk::Text {
            delta: "hello".to_string(),
        };
        assert!(matches!(chunk, StreamChunk::Text { delta } if delta == "hello"));
    }

    #[test]
    fn stream_chunk_thinking() {
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
    fn stream_chunk_tool_call_complete() {
        let chunk = StreamChunk::ToolCallComplete {
            id: "tc-1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/main.rs"}),
        };
        assert!(matches!(chunk, StreamChunk::ToolCallComplete { name, .. } if name == "read_file"));
    }

    #[test]
    fn usage_info_default() {
        let usage = UsageInfo::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert!(usage.total_cost.is_none());
        assert!(usage.cache_read_tokens.is_none());
    }

    #[test]
    fn model_config_creation() {
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
    fn model_info_fields() {
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
            cache_read_price_per_mtok: None,
            cache_write_price_per_mtok: None,
            thinking_price_per_mtok: None,
        };
        assert!(info.supports_tools);
        assert!(!info.supports_thinking);
    }
}

#[cfg(test)]
mod factory_tests {
    use super::*;

    #[test]
    fn create_provider_anthropic() {
        let provider = create_provider("anthropic", "test-key", None);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().model_info().provider, "anthropic");
    }

    #[test]
    fn create_provider_unknown() {
        let result = create_provider("unknown", "key", None);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("Unknown provider"));
    }

    #[test]
    fn create_provider_empty_key() {
        let provider = create_provider("anthropic", "", None);
        assert!(provider.is_err());
    }
}
