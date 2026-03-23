//! Anthropic (Claude) provider implementation.
//!
//! Connects to the Anthropic Messages API via SSE streaming. Supports
//! text, extended thinking (with signature tracking), and tool use.

use super::common::create_http_client;
use super::{
    ContentBlock, Message, MessageRole, ModelConfig, ModelInfo, Provider, ProviderStream,
    StreamChunk, ToolDefinition, UsageInfo,
};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

/// Anthropic (Claude) provider.
pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    pub(crate) base_url: String,
    model_info: ModelInfo,
    cancel: CancellationToken,
}

impl AnthropicProvider {
    /// Creates a new Anthropic provider.
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
                cache_read_price_per_mtok: Some(0.30),
                cache_write_price_per_mtok: Some(3.75),
                thinking_price_per_mtok: None,
            },
            cancel: CancellationToken::new(),
        })
    }
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl Provider for AnthropicProvider {
    /// Streams a response from the Anthropic Messages API via SSE.
    async fn create_message(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> anyhow::Result<ProviderStream> {
        let cancel = self.cancel.clone();

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
            system: system_prompt.to_string(),
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
                    () = cancel.cancelled() => {
                        es.close();
                        break;
                    }
                    event = es.next() => {
                        match event {
                            Some(Ok(Event::Open)) => {}
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
                                            total_cost: None,
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
                                    _ => {}
                                }
                            }
                            Some(Err(e)) => {
                                yield Err(anyhow::anyhow!("SSE error: {e}"));
                                break;
                            }
                            None => {
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

    /// Returns model metadata for the default Claude model.
    fn model_info(&self) -> &ModelInfo {
        &self.model_info
    }

    /// Cancels the current in-flight streaming request.
    fn abort(&self) {
        self.cancel.cancel();
    }
}

/// Converts internal [`Message`] list to Anthropic API format.
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
                    ContentBlock::Text(text) => ApiContentBlock::Text { text: text.clone() },
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
                    ContentBlock::Image { .. } => ApiContentBlock::Text {
                        text: "[image]".to_string(),
                    },
                })
                .collect();
            ApiMessage {
                role: role.to_string(),
                content,
            }
        })
        .collect()
}

/// Converts [`ToolDefinition`] list to Anthropic API format.
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

// ── API request types (serialization) ──────────────────────────────

#[derive(Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    system: String,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiTool>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
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

// ── SSE event response types (deserialization) ─────────────────────

#[derive(Deserialize)]
struct MessageStartEvent {
    message: MessageStartData,
}

#[derive(Deserialize)]
struct MessageStartData {
    usage: Option<MessageUsage>,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)]
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
    #[allow(dead_code)]
    delta: MessageDeltaInner,
    usage: Option<MessageUsage>,
}

#[derive(Deserialize)]
struct MessageDeltaInner {
    #[allow(dead_code)]
    stop_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ContentBlock, Message, MessageRole, ToolDefinition};

    #[test]
    fn rejects_empty_key() {
        let result = AnthropicProvider::new("", None);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("API key is required"));
    }

    #[test]
    fn provider_creation() {
        let provider = AnthropicProvider::new("test-key", None).unwrap();
        assert_eq!(provider.model_info().provider, "anthropic");
        assert!(provider.model_info().supports_tools);
        assert!(provider.model_info().supports_thinking);
        assert_eq!(provider.model_info().context_window, 200_000);
    }

    #[test]
    fn custom_base_url() {
        let provider = AnthropicProvider::new("test-key", Some("https://custom.api.com")).unwrap();
        assert_eq!(provider.base_url, "https://custom.api.com");
    }

    #[test]
    fn default_base_url() {
        let provider = AnthropicProvider::new("test-key", None).unwrap();
        assert_eq!(provider.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn convert_tools_works() {
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
    fn convert_empty_tools() {
        let api_tools = convert_tools(&[]);
        assert!(api_tools.is_empty());
    }

    #[test]
    fn message_conversion_user() {
        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text("hello".to_string())],
        }];
        let api_messages = convert_messages(&messages);
        assert_eq!(api_messages.len(), 1);
        assert_eq!(api_messages[0].role, "user");
    }

    #[test]
    fn message_conversion_assistant_with_tool() {
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
    fn message_conversion_tool_result() {
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
    fn message_conversion_thinking() {
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
    fn api_request_serialization() {
        let request = ApiRequest {
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 8192,
            temperature: None,
            system: "You are helpful.".to_string(),
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
        assert_eq!(json["system"], "You are helpful.");
        assert_eq!(json["thinking"]["budget_tokens"], 10000);
        assert!(json["stream"].as_bool().unwrap());
    }

    #[test]
    fn api_request_without_thinking() {
        let request = ApiRequest {
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 4096,
            temperature: Some(0.7),
            system: String::new(),
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
    fn sse_event_parsing_message_start() {
        let data = r#"{"message":{"usage":{"input_tokens":100,"output_tokens":0,"cache_read_input_tokens":50}}}"#;
        let parsed: MessageStartEvent = serde_json::from_str(data).unwrap();
        let usage = parsed.message.usage.unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.cache_read_input_tokens, Some(50));
    }

    #[test]
    fn sse_event_parsing_content_block_start_text() {
        let data = r#"{"content_block":{"type":"text"}}"#;
        let parsed: ContentBlockStart = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.content_block.block_type, "text");
    }

    #[test]
    fn sse_event_parsing_content_block_start_tool() {
        let data = r#"{"content_block":{"type":"tool_use","id":"toolu_123","name":"read_file"}}"#;
        let parsed: ContentBlockStart = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.content_block.block_type, "tool_use");
        assert_eq!(parsed.content_block.id, Some("toolu_123".to_string()));
        assert_eq!(parsed.content_block.name, Some("read_file".to_string()));
    }

    #[test]
    fn sse_event_parsing_text_delta() {
        let data = r#"{"delta":{"type":"text_delta","text":"Hello"}}"#;
        let parsed: ContentBlockDelta = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.delta.delta_type, "text_delta");
        assert_eq!(parsed.delta.text, Some("Hello".to_string()));
    }

    #[test]
    fn sse_event_parsing_thinking_delta() {
        let data = r#"{"delta":{"type":"thinking_delta","thinking":"Let me think..."}}"#;
        let parsed: ContentBlockDelta = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.delta.delta_type, "thinking_delta");
        assert_eq!(parsed.delta.thinking, Some("Let me think...".to_string()));
    }

    #[test]
    fn sse_event_parsing_signature_delta() {
        let data = r#"{"delta":{"type":"signature_delta","signature":"abc123"}}"#;
        let parsed: ContentBlockDelta = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.delta.delta_type, "signature_delta");
        assert_eq!(parsed.delta.signature, Some("abc123".to_string()));
    }

    #[test]
    fn sse_event_parsing_input_json_delta() {
        let data = r#"{"delta":{"type":"input_json_delta","partial_json":"{\"path\":\""}}"#;
        let parsed: ContentBlockDelta = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.delta.delta_type, "input_json_delta");
        assert!(parsed.delta.partial_json.is_some());
    }

    #[test]
    fn sse_event_parsing_message_delta() {
        let data = r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#;
        let parsed: MessageDelta = serde_json::from_str(data).unwrap();
        assert_eq!(parsed.delta.stop_reason, Some("end_turn".to_string()));
        assert_eq!(parsed.usage.unwrap().output_tokens, Some(42));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::provider::{ContentBlock, Message, MessageRole, ModelConfig};
    use futures::StreamExt;

    #[tokio::test]
    #[ignore]
    async fn anthropic_streaming() {
        let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY not set");
        let provider = AnthropicProvider::new(&api_key, None).unwrap();

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

    #[tokio::test]
    #[ignore]
    async fn anthropic_thinking() {
        let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY not set");
        let provider = AnthropicProvider::new(&api_key, None).unwrap();

        let config = ModelConfig {
            model_id: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 16000,
            temperature: None,
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
