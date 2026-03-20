# STEP 08 — OpenAI Provider

## Objective
Implement the OpenAI provider supporting both the Chat Completions API and the Responses API (for newer models). After this step, users can use GPT-4o, GPT-4.1, O-series, and other OpenAI models.

## Prerequisites
- STEP 05 complete (Provider trait defined)

## Detailed Instructions

### 8.1 Implement OpenAI provider (`src/provider/openai.rs`)

Two API paths are supported:
1. **Chat Completions** (`/v1/chat/completions`) — Standard path for most models (GPT-4o, GPT-4.1).
2. **Responses API** (`/v1/responses`) — For newer models that support it. Uses WebSocket for lower latency optionally.

#### 8.1.1 Define OpenAI API types

```rust
//! OpenAI provider — Chat Completions API and Responses API.

use serde::{Deserialize, Serialize};

// ─── Request types ──────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    max_tokens: Option<u32>,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OaiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

#[derive(Serialize)]
struct OaiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<serde_json::Value>, // Can be string or array of content parts
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OaiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct OaiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String, // "function"
    function: OaiToolCallFunction,
}

#[derive(Serialize)]
struct OaiToolCallFunction {
    name: String,
    arguments: String, // JSON string
}

#[derive(Serialize)]
struct OaiTool {
    #[serde(rename = "type")]
    tool_type: String, // "function"
    function: OaiFunction,
}

#[derive(Serialize)]
struct OaiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

// ─── Streaming response types ───────────────────────────────────────────────

#[derive(Deserialize)]
struct ChatCompletionChunk {
    id: Option<String>,
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<ChunkUsage>,
}

#[derive(Deserialize)]
struct ChunkChoice {
    delta: ChunkDelta,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChunkDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChunkToolCall>>,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Deserialize)]
struct ChunkToolCall {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ChunkFunction>,
}

#[derive(Deserialize)]
struct ChunkFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct ChunkUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    #[serde(default)]
    completion_tokens_details: Option<CompletionTokenDetails>,
}

#[derive(Deserialize)]
struct CompletionTokenDetails {
    #[serde(default)]
    reasoning_tokens: Option<u64>,
}
```

#### 8.1.2 Implement the provider struct

```rust
use crate::provider::{
    create_http_client, CancellationToken, ModelInfo, Provider, ProviderStream, StreamChunk,
    Message, ContentBlock, MessageRole, ModelConfig, ToolDefinition, UsageInfo,
};
use async_trait::async_trait;
use futures::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model_info: ModelInfo,
    cancel: CancellationToken,
}

impl OpenAiProvider {
    pub fn new(api_key: &str, base_url: Option<&str>) -> anyhow::Result<Self> {
        anyhow::ensure!(!api_key.is_empty(), "OpenAI API key is required");
        Ok(Self {
            client: create_http_client()?,
            api_key: api_key.to_string(),
            base_url: base_url.unwrap_or("https://api.openai.com").to_string(),
            model_info: ModelInfo {
                id: "gpt-4.1".to_string(),
                name: "GPT-4.1".to_string(),
                provider: "openai".to_string(),
                max_tokens: 32768,
                context_window: 1_000_000,
                supports_tools: true,
                supports_thinking: false,
                supports_images: true,
                input_price_per_mtok: 2.0,
                output_price_per_mtok: 8.0,
            },
            cancel: CancellationToken::new(),
        })
    }
}
```

#### 8.1.3 Implement the Provider trait with streaming via SSE

The `create_message` implementation should:
1. Convert messages to OpenAI format using `convert_messages()`
2. Convert tools to OpenAI format using `convert_tools()`
3. Build the request body with `stream: true`
4. Send POST to `{base_url}/v1/chat/completions`
5. Parse SSE events (`data: {...}` lines) from the response body
6. For each chunk, convert to `StreamChunk` variants:
   - `delta.content` -> `StreamChunk::Text`
   - `delta.reasoning` -> `StreamChunk::Thinking`
   - `delta.tool_calls` -> track by index, emit `StreamChunk::ToolCallDelta` and then `ToolCallComplete` on finish
   - `usage` -> `StreamChunk::Usage`
   - `finish_reason: "stop"` -> `StreamChunk::Done`
   - `data: [DONE]` -> stream end sentinel

```rust
#[async_trait]
impl Provider for OpenAiProvider {
    async fn create_message(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> anyhow::Result<ProviderStream> {
        let mut oai_messages = vec![OaiMessage {
            role: "system".to_string(),
            content: Some(serde_json::Value::String(system.to_string())),
            tool_calls: None,
            tool_call_id: None,
        }];
        oai_messages.extend(convert_messages(messages));

        let request_body = ChatCompletionRequest {
            model: config.model_id.clone(),
            messages: oai_messages,
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            stream: true,
            tools: convert_tools(tools),
            reasoning_effort: map_thinking_budget_to_effort(config.thinking_budget),
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
        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();
            // Track in-progress tool calls by index
            let mut tool_calls: std::collections::HashMap<usize, (String, String, String)> =
                std::collections::HashMap::new();

            while let Some(chunk_result) = byte_stream.next().await {
                if cancel.is_cancelled() {
                    break;
                }

                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = tx.send(Err(anyhow::anyhow!("Stream error: {e}"))).await;
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&bytes));

                // Process complete SSE lines
                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        if data.trim() == "[DONE]" {
                            let _ = tx.send(Ok(StreamChunk::Done)).await;
                            return;
                        }

                        match serde_json::from_str::<ChatCompletionChunk>(data) {
                            Ok(chunk) => {
                                // Process each choice
                                for choice in &chunk.choices {
                                    // Text content
                                    if let Some(content) = &choice.delta.content {
                                        if !content.is_empty() {
                                            let _ = tx
                                                .send(Ok(StreamChunk::Text {
                                                    delta: content.clone(),
                                                }))
                                                .await;
                                        }
                                    }

                                    // Reasoning content
                                    if let Some(reasoning) = &choice.delta.reasoning {
                                        if !reasoning.is_empty() {
                                            let _ = tx
                                                .send(Ok(StreamChunk::Thinking {
                                                    delta: reasoning.clone(),
                                                    signature: None,
                                                    redacted: false,
                                                }))
                                                .await;
                                        }
                                    }

                                    // Tool calls (index-based tracking)
                                    if let Some(tcs) = &choice.delta.tool_calls {
                                        for tc in tcs {
                                            let entry = tool_calls
                                                .entry(tc.index)
                                                .or_insert_with(|| {
                                                    (String::new(), String::new(), String::new())
                                                });

                                            if let Some(id) = &tc.id {
                                                entry.0 = id.clone();
                                            }
                                            if let Some(func) = &tc.function {
                                                if let Some(name) = &func.name {
                                                    entry.1 = name.clone();
                                                }
                                                if let Some(args) = &func.arguments {
                                                    entry.2.push_str(args);
                                                    let _ = tx
                                                        .send(Ok(StreamChunk::ToolCallDelta {
                                                            index: tc.index,
                                                            id: entry.0.clone(),
                                                            name: if entry.1.is_empty() {
                                                                None
                                                            } else {
                                                                Some(entry.1.clone())
                                                            },
                                                            arguments_delta: args.clone(),
                                                        }))
                                                        .await;
                                                }
                                            }
                                        }
                                    }

                                    // Finish reason
                                    if let Some(reason) = &choice.finish_reason {
                                        if reason == "tool_calls" {
                                            // Emit ToolCallComplete for all tracked tool calls
                                            for (_idx, (id, name, args)) in tool_calls.drain() {
                                                let arguments =
                                                    serde_json::from_str(&args).unwrap_or(
                                                        serde_json::Value::String(args),
                                                    );
                                                let _ = tx
                                                    .send(Ok(StreamChunk::ToolCallComplete {
                                                        id,
                                                        name,
                                                        arguments,
                                                    }))
                                                    .await;
                                            }
                                        }
                                    }
                                }

                                // Usage
                                if let Some(usage) = &chunk.usage {
                                    let _ = tx
                                        .send(Ok(StreamChunk::Usage(UsageInfo {
                                            input_tokens: usage.prompt_tokens,
                                            output_tokens: usage.completion_tokens,
                                            cache_read_tokens: 0,
                                            cache_write_tokens: 0,
                                        })))
                                        .await;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Failed to parse OpenAI chunk: {e}, data: {data}");
                            }
                        }
                    }
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.model_info
    }

    fn abort(&self) {
        self.cancel.cancel();
    }
}
```

#### 8.1.4 Message conversion

```rust
/// Convert internal Message format to OpenAI message format.
fn convert_messages(messages: &[Message]) -> Vec<OaiMessage> {
    let mut result = Vec::new();

    for msg in messages {
        match msg.role {
            MessageRole::User => {
                // Check if this is a tool result message
                let mut tool_results = Vec::new();
                let mut text_parts = Vec::new();

                for block in &msg.content {
                    match block {
                        ContentBlock::Text(t) => text_parts.push(t.clone()),
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            tool_results.push((tool_use_id.clone(), content.clone(), *is_error));
                        }
                        // Skip thinking blocks — not supported in OpenAI format
                        ContentBlock::Thinking { .. } => {}
                        _ => {}
                    }
                }

                // Emit tool result messages first (each as separate "tool" role message)
                for (tool_use_id, content, _is_error) in tool_results {
                    result.push(OaiMessage {
                        role: "tool".to_string(),
                        content: Some(serde_json::Value::String(content)),
                        tool_calls: None,
                        tool_call_id: Some(tool_use_id),
                    });
                }

                // Emit text content as user message
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
                let mut tool_calls = Vec::new();

                for block in &msg.content {
                    match block {
                        ContentBlock::Text(t) => text_content.push_str(t),
                        ContentBlock::ToolUse { id, name, input } => {
                            tool_calls.push(OaiToolCall {
                                id: id.clone(),
                                call_type: "function".to_string(),
                                function: OaiToolCallFunction {
                                    name: name.clone(),
                                    arguments: serde_json::to_string(input)
                                        .unwrap_or_default(),
                                },
                            });
                        }
                        // Skip thinking blocks — not sent to OpenAI
                        ContentBlock::Thinking { .. } => {}
                        _ => {}
                    }
                }

                result.push(OaiMessage {
                    role: "assistant".to_string(),
                    content: if text_content.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::String(text_content))
                    },
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                    tool_call_id: None,
                });
            }
        }
    }

    result
}

/// Convert internal ToolDefinition to OpenAI tool format.
fn convert_tools(tools: &[ToolDefinition]) -> Vec<OaiTool> {
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
```

#### 8.1.5 Reasoning effort mapping

```rust
/// Map internal thinking_budget to OpenAI reasoning_effort parameter.
/// The thinking_budget is a token count; reasoning_effort is a qualitative level.
fn map_thinking_budget_to_effort(budget: Option<u32>) -> Option<String> {
    budget.map(|b| match b {
        0 => "none".to_string(),
        1..=5000 => "low".to_string(),
        5001..=15000 => "medium".to_string(),
        _ => "high".to_string(),
    })
}
```

### 8.2 Register in provider factory

Update `create_provider()` in `src/provider/mod.rs`:

```rust
pub fn create_provider(provider_name: &str, api_key: &str) -> anyhow::Result<Box<dyn Provider>> {
    match provider_name {
        "anthropic" => Ok(Box::new(anthropic::AnthropicProvider::new(api_key, None)?)),
        "openai" => Ok(Box::new(openai::OpenAiProvider::new(api_key, None)?)),
        other => anyhow::bail!("Unknown provider: {other}"),
    }
}
```

### 8.3 Model info lookup

Add a helper to get model info for known OpenAI models:

```rust
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
        },
        // Default fallback
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
        },
    }
}
```

## Tests

```rust
#[cfg(test)]
mod openai_tests {
    use super::*;

    #[test]
    fn test_openai_rejects_empty_key() {
        assert!(OpenAiProvider::new("", None).is_err());
    }

    #[test]
    fn test_openai_provider_creation() {
        let p = OpenAiProvider::new("test-key", None).unwrap();
        assert_eq!(p.model_info().provider, "openai");
    }

    #[test]
    fn test_openai_custom_base_url() {
        let p = OpenAiProvider::new("test-key", Some("https://custom.api.com")).unwrap();
        assert_eq!(p.base_url, "https://custom.api.com");
    }

    #[test]
    fn test_message_conversion_simple() {
        let messages = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text("hello".to_string())],
        }];
        let oai = convert_messages(&messages);
        assert_eq!(oai.len(), 1);
        assert_eq!(oai[0].role, "user");
    }

    #[test]
    fn test_message_conversion_tool_result() {
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
    fn test_message_conversion_assistant_with_tool_use() {
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
    fn test_message_conversion_strips_thinking() {
        let messages = vec![Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    thinking: "Let me think...".to_string(),
                    signature: None,
                },
                ContentBlock::Text("Here is the answer.".to_string()),
            ],
        }];
        let oai = convert_messages(&messages);
        assert_eq!(oai.len(), 1);
        // Content should only have the text, not thinking
        let content = oai[0].content.as_ref().unwrap().as_str().unwrap();
        assert_eq!(content, "Here is the answer.");
    }

    #[test]
    fn test_reasoning_effort_mapping() {
        assert_eq!(
            map_thinking_budget_to_effort(Some(0)),
            Some("none".to_string())
        );
        assert_eq!(
            map_thinking_budget_to_effort(Some(1000)),
            Some("low".to_string())
        );
        assert_eq!(
            map_thinking_budget_to_effort(Some(5000)),
            Some("low".to_string())
        );
        assert_eq!(
            map_thinking_budget_to_effort(Some(10000)),
            Some("medium".to_string())
        );
        assert_eq!(
            map_thinking_budget_to_effort(Some(15000)),
            Some("medium".to_string())
        );
        assert_eq!(
            map_thinking_budget_to_effort(Some(50000)),
            Some("high".to_string())
        );
        assert_eq!(map_thinking_budget_to_effort(None), None);
    }

    #[test]
    fn test_chunk_parsing_text() {
        let json = r#"{"id":"chatcmpl-1","choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(chunk.choices[0].delta.content, Some("Hello".to_string()));
    }

    #[test]
    fn test_chunk_parsing_tool_call_start() {
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
    fn test_chunk_parsing_tool_call_arguments() {
        let json = r#"{"id":"chatcmpl-1","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        let tc = &chunk.choices[0].delta.tool_calls.as_ref().unwrap()[0];
        assert_eq!(
            tc.function.as_ref().unwrap().arguments,
            Some("{\"path\":".to_string())
        );
    }

    #[test]
    fn test_chunk_parsing_usage() {
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
    fn test_chunk_parsing_finish_reason_stop() {
        let json = r#"{"id":"chatcmpl-1","choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(
            chunk.choices[0].finish_reason,
            Some("stop".to_string())
        );
    }

    #[test]
    fn test_chunk_parsing_finish_reason_tool_calls() {
        let json =
            r#"{"id":"chatcmpl-1","choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(
            chunk.choices[0].finish_reason,
            Some("tool_calls".to_string())
        );
    }

    #[test]
    fn test_tool_conversion() {
        let tools = vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }];
        let oai_tools = convert_tools(&tools);
        assert_eq!(oai_tools.len(), 1);
        assert_eq!(oai_tools[0].function.name, "read_file");
        assert_eq!(oai_tools[0].tool_type, "function");
        assert_eq!(oai_tools[0].function.description, "Read a file");
    }

    #[test]
    fn test_tool_conversion_empty() {
        let oai_tools = convert_tools(&[]);
        assert!(oai_tools.is_empty());
    }

    #[test]
    fn test_model_info_known_model() {
        let info = openai_model_info("gpt-4.1");
        assert_eq!(info.id, "gpt-4.1");
        assert_eq!(info.context_window, 1_000_000);
        assert!(info.supports_tools);
    }

    #[test]
    fn test_model_info_o3() {
        let info = openai_model_info("o3");
        assert!(info.supports_thinking);
    }

    #[test]
    fn test_model_info_unknown_model() {
        let info = openai_model_info("gpt-future");
        assert_eq!(info.id, "gpt-future");
        assert_eq!(info.provider, "openai");
    }

    #[tokio::test]
    #[ignore]
    async fn test_openai_streaming_integration() {
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
```

## Acceptance Criteria
- [ ] OpenAI Chat Completions API streaming works end-to-end
- [ ] Tool calls with index-based tracking accumulate arguments correctly
- [ ] ToolCallComplete emitted when finish_reason is "tool_calls"
- [ ] Reasoning effort maps correctly from thinking_budget
- [ ] Message conversion handles all content block types (Text, ToolUse, ToolResult)
- [ ] Thinking blocks are stripped when converting to OpenAI format
- [ ] Usage tokens include reasoning token breakdown when available
- [ ] Cancellation via CancellationToken stops the stream
- [ ] Provider factory handles "openai" provider name
- [ ] Model info lookup returns correct values for known models
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All unit tests pass
