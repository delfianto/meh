//! `OpenAI` provider — Chat Completions API with SSE streaming.
//!
//! Supports GPT-4o, GPT-4.1, O-series, and other `OpenAI` models.
//! Tool calls use index-based tracking (`OpenAI` sends deltas keyed
//! by array index rather than by ID). Reasoning content from O-series
//! models is mapped to `StreamChunk::Thinking`.

use super::common::create_http_client;
use super::{
    ContentBlock, Message, MessageRole, ModelConfig, ModelInfo, Provider, ProviderStream,
    StreamChunk, ToolDefinition, UsageInfo,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// `OpenAI` provider.
pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: String,
    pub base_url: String,
    model_info: ModelInfo,
    cancel: CancellationToken,
}

impl OpenAiProvider {
    /// Creates a new `OpenAI` provider.
    ///
    /// # Errors
    /// Returns an error if the API key is empty.
    pub fn new(api_key: &str, base_url: Option<&str>) -> anyhow::Result<Self> {
        anyhow::ensure!(!api_key.is_empty(), "OpenAI API key is required");
        Ok(Self {
            client: create_http_client()?,
            api_key: api_key.to_string(),
            base_url: base_url.unwrap_or(DEFAULT_BASE_URL).to_string(),
            model_info: openai_model_info("gpt-4.1"),
            cancel: CancellationToken::new(),
        })
    }
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl Provider for OpenAiProvider {
    /// Streams a response from the `OpenAI` Chat Completions API via SSE.
    async fn create_message(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> anyhow::Result<ProviderStream> {
        let mut oai_messages = vec![OaiMessage {
            role: "system".to_string(),
            content: Some(serde_json::Value::String(system_prompt.to_string())),
            tool_calls: None,
            tool_call_id: None,
        }];
        oai_messages.extend(convert_messages(messages));

        let request_body = ChatCompletionRequest {
            model: config.model_id.clone(),
            messages: oai_messages,
            temperature: config.temperature,
            max_tokens: Some(config.max_tokens),
            stream: true,
            tools: convert_tools(tools),
            reasoning_effort: config.thinking_budget.map(thinking_budget_to_effort),
        };

        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI API error {status}: {body}");
        }

        let cancel = self.cancel.clone();

        let stream = async_stream::stream! {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut tool_calls: HashMap<usize, (String, String, String)> = HashMap::new();

            while let Some(chunk_result) = byte_stream.next().await {
                if cancel.is_cancelled() {
                    break;
                }

                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(e) => {
                        yield Err(anyhow::anyhow!("Stream error: {e}"));
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&bytes));

                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        if data.trim() == "[DONE]" {
                            yield Ok(StreamChunk::Done);
                            return;
                        }

                        if let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) {
                            for event in process_chunk(&chunk, &mut tool_calls) {
                                yield Ok(event);
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    /// Returns model metadata.
    fn model_info(&self) -> &ModelInfo {
        &self.model_info
    }

    /// Cancels the current in-flight streaming request.
    fn abort(&self) {
        self.cancel.cancel();
    }
}

/// Processes a single parsed SSE chunk and returns `StreamChunk` events.
pub fn process_chunk(
    chunk: &ChatCompletionChunk,
    tool_calls: &mut HashMap<usize, (String, String, String)>,
) -> Vec<StreamChunk> {
    let mut events = Vec::new();

    for choice in &chunk.choices {
        if let Some(content) = &choice.delta.content {
            if !content.is_empty() {
                events.push(StreamChunk::Text {
                    delta: content.clone(),
                });
            }
        }

        if let Some(reasoning) = &choice.delta.reasoning {
            if !reasoning.is_empty() {
                events.push(StreamChunk::Thinking {
                    delta: reasoning.clone(),
                    signature: None,
                    redacted: false,
                });
            }
        }

        if let Some(tcs) = &choice.delta.tool_calls {
            for tc in tcs {
                let entry = tool_calls
                    .entry(tc.index)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));

                if let Some(id) = &tc.id {
                    entry.0.clone_from(id);
                }
                if let Some(func) = &tc.function {
                    if let Some(name) = &func.name {
                        entry.1.clone_from(name);
                    }
                    if let Some(args) = &func.arguments {
                        entry.2.push_str(args);
                        events.push(StreamChunk::ToolCallDelta {
                            id: entry.0.clone(),
                            name: entry.1.clone(),
                            arguments_delta: args.clone(),
                        });
                    }
                }
            }
        }

        if let Some(reason) = &choice.finish_reason {
            if reason == "tool_calls" {
                for (_idx, (id, name, args)) in tool_calls.drain() {
                    let arguments =
                        serde_json::from_str(&args).unwrap_or(serde_json::Value::String(args));
                    events.push(StreamChunk::ToolCallComplete {
                        id,
                        name,
                        arguments,
                    });
                }
            }
        }
    }

    if let Some(usage) = &chunk.usage {
        events.push(StreamChunk::Usage(UsageInfo {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cache_read_tokens: None,
            cache_write_tokens: None,
            thinking_tokens: usage
                .completion_tokens_details
                .as_ref()
                .and_then(|d| d.reasoning_tokens),
            total_cost: None,
        }));
    }

    events
}

/// Converts internal [`Message`] list to `OpenAI` message format.
pub fn convert_messages(messages: &[Message]) -> Vec<OaiMessage> {
    let mut result = Vec::new();

    for msg in messages {
        match msg.role {
            MessageRole::User => {
                let mut tool_results = Vec::new();
                let mut text_parts = Vec::new();

                for block in &msg.content {
                    match block {
                        ContentBlock::Text(t) => text_parts.push(t.clone()),
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            tool_results.push((tool_use_id.clone(), content.clone()));
                        }
                        ContentBlock::Thinking { .. }
                        | ContentBlock::ToolUse { .. }
                        | ContentBlock::Image { .. } => {}
                    }
                }

                for (tool_use_id, content) in tool_results {
                    result.push(OaiMessage {
                        role: "tool".to_string(),
                        content: Some(serde_json::Value::String(content)),
                        tool_calls: None,
                        tool_call_id: Some(tool_use_id),
                    });
                }

                if !text_parts.is_empty() {
                    result.push(OaiMessage {
                        role: "user".to_string(),
                        content: Some(serde_json::Value::String(text_parts.join("\n"))),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
            }

            MessageRole::Assistant => {
                let mut text_content = String::new();
                let mut oai_tool_calls = Vec::new();

                for block in &msg.content {
                    match block {
                        ContentBlock::Text(t) => text_content.push_str(t),
                        ContentBlock::ToolUse { id, name, input } => {
                            oai_tool_calls.push(OaiToolCall {
                                id: id.clone(),
                                call_type: "function".to_string(),
                                function: OaiToolCallFunction {
                                    name: name.clone(),
                                    arguments: serde_json::to_string(input).unwrap_or_default(),
                                },
                            });
                        }
                        ContentBlock::Thinking { .. }
                        | ContentBlock::ToolResult { .. }
                        | ContentBlock::Image { .. } => {}
                    }
                }

                result.push(OaiMessage {
                    role: "assistant".to_string(),
                    content: if text_content.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::String(text_content))
                    },
                    tool_calls: if oai_tool_calls.is_empty() {
                        None
                    } else {
                        Some(oai_tool_calls)
                    },
                    tool_call_id: None,
                });
            }
        }
    }

    result
}

/// Converts [`ToolDefinition`] list to `OpenAI` tool format.
pub fn convert_tools(tools: &[ToolDefinition]) -> Vec<OaiTool> {
    tools
        .iter()
        .map(|t| OaiTool {
            tool_type: "function".to_string(),
            function: OaiFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
            },
        })
        .collect()
}

/// Maps `thinking_budget` token count to `OpenAI` `reasoning_effort` level.
fn thinking_budget_to_effort(budget: u32) -> String {
    match budget {
        0 => "none".to_string(),
        1..=5000 => "low".to_string(),
        5001..=15000 => "medium".to_string(),
        _ => "high".to_string(),
    }
}

/// Returns [`ModelInfo`] for known `OpenAI` models, with a sensible fallback.
fn openai_model_info(model_id: &str) -> ModelInfo {
    match model_id {
        "gpt-4.1" => ModelInfo {
            id: "gpt-4.1".into(),
            name: "GPT-4.1".into(),
            provider: "openai".into(),
            max_tokens: 32768,
            context_window: 1_000_000,
            supports_tools: true,
            supports_thinking: false,
            supports_images: true,
            input_price_per_mtok: 2.0,
            output_price_per_mtok: 8.0,
            cache_read_price_per_mtok: None,
            cache_write_price_per_mtok: None,
            thinking_price_per_mtok: None,
        },
        "gpt-4.1-mini" => ModelInfo {
            id: "gpt-4.1-mini".into(),
            name: "GPT-4.1 Mini".into(),
            provider: "openai".into(),
            max_tokens: 32768,
            context_window: 1_000_000,
            supports_tools: true,
            supports_thinking: false,
            supports_images: true,
            input_price_per_mtok: 0.4,
            output_price_per_mtok: 1.6,
            cache_read_price_per_mtok: None,
            cache_write_price_per_mtok: None,
            thinking_price_per_mtok: None,
        },
        "gpt-4o" => ModelInfo {
            id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            provider: "openai".into(),
            max_tokens: 16384,
            context_window: 128_000,
            supports_tools: true,
            supports_thinking: false,
            supports_images: true,
            input_price_per_mtok: 2.5,
            output_price_per_mtok: 10.0,
            cache_read_price_per_mtok: None,
            cache_write_price_per_mtok: None,
            thinking_price_per_mtok: None,
        },
        "o3" => ModelInfo {
            id: "o3".into(),
            name: "O3".into(),
            provider: "openai".into(),
            max_tokens: 100_000,
            context_window: 200_000,
            supports_tools: true,
            supports_thinking: true,
            supports_images: true,
            input_price_per_mtok: 2.0,
            output_price_per_mtok: 8.0,
            cache_read_price_per_mtok: None,
            cache_write_price_per_mtok: None,
            thinking_price_per_mtok: None,
        },
        "o3-mini" => ModelInfo {
            id: "o3-mini".into(),
            name: "O3 Mini".into(),
            provider: "openai".into(),
            max_tokens: 100_000,
            context_window: 200_000,
            supports_tools: true,
            supports_thinking: true,
            supports_images: false,
            input_price_per_mtok: 1.1,
            output_price_per_mtok: 4.4,
            cache_read_price_per_mtok: None,
            cache_write_price_per_mtok: None,
            thinking_price_per_mtok: None,
        },
        other => ModelInfo {
            id: other.into(),
            name: other.into(),
            provider: "openai".into(),
            max_tokens: 4096,
            context_window: 128_000,
            supports_tools: true,
            supports_thinking: false,
            supports_images: false,
            input_price_per_mtok: 2.0,
            output_price_per_mtok: 8.0,
            cache_read_price_per_mtok: None,
            cache_write_price_per_mtok: None,
            thinking_price_per_mtok: None,
        },
    }
}

// ── Request types ──────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OaiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

#[derive(Serialize)]
pub struct OaiMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OaiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize)]
pub struct OaiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OaiToolCallFunction,
}

#[derive(Serialize)]
pub struct OaiToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
pub struct OaiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OaiFunction,
}

#[derive(Serialize)]
pub struct OaiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

// ── Streaming response types ───────────────────────────────────────

#[derive(Deserialize)]
pub struct ChatCompletionChunk {
    #[allow(dead_code)]
    pub id: Option<String>,
    pub choices: Vec<ChunkChoice>,
    #[serde(default)]
    pub usage: Option<ChunkUsage>,
}

#[derive(Deserialize)]
pub struct ChunkChoice {
    pub delta: ChunkDelta,
    pub finish_reason: Option<String>,
}

#[derive(Deserialize)]
pub struct ChunkDelta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ChunkToolCall>>,
    #[serde(default)]
    pub reasoning: Option<String>,
}

#[derive(Deserialize)]
pub struct ChunkToolCall {
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<ChunkFunction>,
}

#[derive(Deserialize)]
pub struct ChunkFunction {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Deserialize)]
pub struct ChunkUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default)]
    pub completion_tokens_details: Option<CompletionTokenDetails>,
}

#[derive(Deserialize)]
pub struct CompletionTokenDetails {
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ContentBlock, Message, MessageRole, ToolDefinition};

    #[test]
    fn rejects_empty_key() {
        assert!(OpenAiProvider::new("", None).is_err());
    }

    #[test]
    fn provider_creation() {
        let p = OpenAiProvider::new("test-key", None).unwrap();
        assert_eq!(p.model_info().provider, "openai");
    }

    #[test]
    fn custom_base_url() {
        let p = OpenAiProvider::new("test-key", Some("https://custom.api.com")).unwrap();
        assert_eq!(p.base_url, "https://custom.api.com");
    }

    #[test]
    fn message_conversion_simple() {
        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text("hello".to_string())],
        }];
        let oai = convert_messages(&messages);
        assert_eq!(oai.len(), 1);
        assert_eq!(oai[0].role, "user");
    }

    #[test]
    fn message_conversion_tool_result() {
        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tc1".to_string(),
                content: "result".to_string(),
                is_error: false,
            }],
        }];
        let oai = convert_messages(&messages);
        assert_eq!(oai.len(), 1);
        assert_eq!(oai[0].role, "tool");
        assert_eq!(oai[0].tool_call_id, Some("tc1".to_string()));
    }

    #[test]
    fn message_conversion_assistant_with_tool_use() {
        let messages = vec![Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Text("Let me check.".to_string()),
                ContentBlock::ToolUse {
                    id: "tc1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "test.rs"}),
                },
            ],
        }];
        let oai = convert_messages(&messages);
        assert_eq!(oai.len(), 1);
        assert_eq!(oai[0].role, "assistant");
        assert!(oai[0].tool_calls.is_some());
        let tool_calls = oai[0].tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "read_file");
    }

    #[test]
    fn message_conversion_strips_thinking() {
        let messages = vec![Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "Let me think...".to_string(),
                    signature: None,
                },
                ContentBlock::Text("Here is the answer.".to_string()),
            ],
        }];
        let oai = convert_messages(&messages);
        assert_eq!(oai.len(), 1);
        let content = oai[0].content.as_ref().unwrap().as_str().unwrap();
        assert_eq!(content, "Here is the answer.");
    }

    #[test]
    fn reasoning_effort_mapping() {
        assert_eq!(thinking_budget_to_effort(0), "none");
        assert_eq!(thinking_budget_to_effort(1000), "low");
        assert_eq!(thinking_budget_to_effort(5000), "low");
        assert_eq!(thinking_budget_to_effort(10000), "medium");
        assert_eq!(thinking_budget_to_effort(15000), "medium");
        assert_eq!(thinking_budget_to_effort(50000), "high");
        assert_eq!(
            Some(0).map(thinking_budget_to_effort),
            Some("none".to_string())
        );
        assert_eq!(None::<u32>.map(thinking_budget_to_effort), None);
    }

    #[test]
    fn chunk_parsing_text() {
        let json =
            r#"{"id":"chatcmpl-1","choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(chunk.choices[0].delta.content, Some("Hello".to_string()));
    }

    #[test]
    fn chunk_parsing_tool_call_start() {
        let json = r#"{"id":"chatcmpl-1","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":""}}]},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        let tc = &chunk.choices[0].delta.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.index, 0);
        assert_eq!(tc.id, Some("call_1".to_string()));
        assert_eq!(
            tc.function.as_ref().unwrap().name,
            Some("read_file".to_string())
        );
    }

    #[test]
    fn chunk_parsing_tool_call_arguments() {
        let json = r#"{"id":"chatcmpl-1","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        let tc = &chunk.choices[0].delta.tool_calls.as_ref().unwrap()[0];
        assert_eq!(
            tc.function.as_ref().unwrap().arguments,
            Some("{\"path\":".to_string())
        );
    }

    #[test]
    fn chunk_parsing_usage() {
        let json = r#"{"id":"chatcmpl-1","choices":[],"usage":{"prompt_tokens":100,"completion_tokens":50,"completion_tokens_details":{"reasoning_tokens":10}}}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        let usage = chunk.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(
            usage.completion_tokens_details.unwrap().reasoning_tokens,
            Some(10)
        );
    }

    #[test]
    fn chunk_parsing_finish_reason_stop() {
        let json = r#"{"id":"chatcmpl-1","choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices[0].finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn chunk_parsing_finish_reason_tool_calls() {
        let json = r#"{"id":"chatcmpl-1","choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(
            chunk.choices[0].finish_reason,
            Some("tool_calls".to_string())
        );
    }

    #[test]
    fn tool_conversion() {
        let tools = vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }];
        let oai_tools = convert_tools(&tools);
        assert_eq!(oai_tools.len(), 1);
        assert_eq!(oai_tools[0].function.name, "read_file");
        assert_eq!(oai_tools[0].tool_type, "function");
    }

    #[test]
    fn tool_conversion_empty() {
        let oai_tools = convert_tools(&[]);
        assert!(oai_tools.is_empty());
    }

    #[test]
    fn model_info_known_model() {
        let info = openai_model_info("gpt-4.1");
        assert_eq!(info.id, "gpt-4.1");
        assert_eq!(info.context_window, 1_000_000);
        assert!(info.supports_tools);
    }

    #[test]
    fn model_info_o3() {
        let info = openai_model_info("o3");
        assert!(info.supports_thinking);
    }

    #[test]
    fn model_info_unknown_model() {
        let info = openai_model_info("gpt-future");
        assert_eq!(info.id, "gpt-future");
        assert_eq!(info.provider, "openai");
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::provider::{ContentBlock, Message, MessageRole, ModelConfig};
    use futures::StreamExt;

    #[tokio::test]
    #[ignore]
    async fn openai_streaming() {
        let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY not set");
        let provider = OpenAiProvider::new(&api_key, None).unwrap();
        let config = ModelConfig {
            model_id: "gpt-4.1-mini".to_string(),
            max_tokens: 100,
            temperature: Some(0.0),
            thinking_budget: None,
        };
        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text("Say hello in one word.".to_string())],
        }];
        let mut stream = provider
            .create_message("You are a helpful assistant.", &messages, &[], &config)
            .await
            .unwrap();

        let mut got_text = false;
        let mut got_done = false;
        while let Some(chunk) = stream.next().await {
            match chunk.unwrap() {
                StreamChunk::Text { .. } => got_text = true,
                StreamChunk::Done => got_done = true,
                _ => {}
            }
        }
        assert!(got_text);
        assert!(got_done);
    }
}
