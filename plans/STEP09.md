# STEP 09 — Gemini Provider

## Objective
Implement the Google Gemini provider with streaming, tool support, and thinking/reasoning. After this step, users can use Gemini 2.5 models.

## Prerequisites
- STEP 05 complete (Provider trait defined)

## Detailed Instructions

### 9.1 Gemini API specifics

The Gemini API (`generativelanguage.googleapis.com`) uses a different format from OpenAI/Anthropic:

**Endpoint**: `POST /v1beta/models/{model}:streamGenerateContent?key={key}&alt=sse`

**Key differences**:
- System instruction is a separate top-level field (not part of the message array)
- Messages are called "contents" with role "user" or "model"
- Tool results are `functionResponse` parts
- Tool calls are `functionCall` parts
- Thinking uses `thought: true` flag on text parts
- Thinking signature uses `thoughtSignature` field
- SSE events contain the full `candidates[0].content.parts[]` array per chunk
- Tool calls arrive as complete objects (not streamed incrementally like OpenAI)

### 9.2 Define Gemini types (`src/provider/gemini.rs`)

```rust
//! Google Gemini provider — streaming with tool and thinking support.

use serde::{Deserialize, Serialize};

// ─── Request types ──────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiToolDecl>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

#[derive(Serialize, Deserialize, Clone)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thought: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_response: Option<GeminiFunctionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_data: Option<InlineData>,
}

impl GeminiPart {
    fn text(s: impl Into<String>) -> Self {
        Self {
            text: Some(s.into()),
            thought: None,
            thought_signature: None,
            function_call: None,
            function_response: None,
            inline_data: None,
        }
    }

    fn thinking(s: impl Into<String>) -> Self {
        Self {
            text: Some(s.into()),
            thought: Some(true),
            thought_signature: None,
            function_call: None,
            function_response: None,
            inline_data: None,
        }
    }

    fn function_call(name: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            text: None,
            thought: None,
            thought_signature: None,
            function_call: Some(GeminiFunctionCall {
                name: name.into(),
                args,
            }),
            function_response: None,
            inline_data: None,
        }
    }

    fn function_response(name: impl Into<String>, response: serde_json::Value) -> Self {
        Self {
            text: None,
            thought: None,
            thought_signature: None,
            function_call: None,
            function_response: Some(GeminiFunctionResponse {
                name: name.into(),
                response,
            }),
            inline_data: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone)]
struct GeminiFunctionResponse {
    name: String,
    response: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct InlineData {
    mime_type: String,
    data: String, // base64-encoded
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiToolDecl {
    function_declarations: Vec<GeminiFunctionDecl>,
}

#[derive(Serialize)]
struct GeminiFunctionDecl {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_config: Option<ThinkingConfig>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_budget: Option<i32>,
}

// ─── Streaming response types ───────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiStreamChunk {
    #[serde(default)]
    candidates: Option<Vec<GeminiCandidate>>,
    #[serde(default)]
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    #[serde(default)]
    content: Option<GeminiContent>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageMetadata {
    #[serde(default)]
    prompt_token_count: Option<u64>,
    #[serde(default)]
    candidates_token_count: Option<u64>,
    #[serde(default)]
    thoughts_token_count: Option<u64>,
    #[serde(default)]
    cached_content_token_count: Option<u64>,
}
```

### 9.3 Implement GeminiProvider

```rust
use crate::provider::{
    create_http_client, CancellationToken, ModelInfo, Provider, ProviderStream, StreamChunk,
    Message, ContentBlock, MessageRole, ModelConfig, ToolDefinition, UsageInfo,
};
use async_trait::async_trait;
use futures::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model_info: ModelInfo,
    cancel: CancellationToken,
}

impl GeminiProvider {
    pub fn new(api_key: &str, base_url: Option<&str>) -> anyhow::Result<Self> {
        anyhow::ensure!(!api_key.is_empty(), "Gemini API key is required");
        Ok(Self {
            client: create_http_client()?,
            api_key: api_key.to_string(),
            base_url: base_url
                .unwrap_or("https://generativelanguage.googleapis.com")
                .to_string(),
            model_info: ModelInfo {
                id: "gemini-2.5-flash".to_string(),
                name: "Gemini 2.5 Flash".to_string(),
                provider: "gemini".to_string(),
                max_tokens: 65536,
                context_window: 1_000_000,
                supports_tools: true,
                supports_thinking: true,
                supports_images: true,
                input_price_per_mtok: 0.15,
                output_price_per_mtok: 0.60,
            },
            cancel: CancellationToken::new(),
        })
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    async fn create_message(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> anyhow::Result<ProviderStream> {
        let contents = convert_messages(messages);
        let system_instruction = GeminiContent {
            role: None,
            parts: vec![GeminiPart::text(system)],
        };

        let gemini_tools = if tools.is_empty() {
            None
        } else {
            Some(vec![convert_tools(tools)])
        };

        let generation_config = GenerationConfig {
            temperature: config.temperature,
            max_output_tokens: Some(config.max_tokens),
            thinking_config: map_thinking_budget(config.thinking_budget),
        };

        let request_body = GeminiRequest {
            contents,
            system_instruction: Some(system_instruction),
            tools: gemini_tools,
            generation_config: Some(generation_config),
        };

        let model_id = &config.model_id;
        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?key={}&alt=sse",
            self.base_url, model_id, self.api_key
        );

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Gemini API error {status}: {body}");
        }

        let cancel = self.cancel.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();

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
                        match serde_json::from_str::<GeminiStreamChunk>(data) {
                            Ok(chunk) => {
                                if let Some(candidates) = &chunk.candidates {
                                    for candidate in candidates {
                                        if let Some(content) = &candidate.content {
                                            for part in &content.parts {
                                                // Thinking part
                                                if part.thought == Some(true) {
                                                    if let Some(text) = &part.text {
                                                        let _ = tx
                                                            .send(Ok(StreamChunk::Thinking {
                                                                delta: text.clone(),
                                                                signature: part
                                                                    .thought_signature
                                                                    .clone(),
                                                                redacted: false,
                                                            }))
                                                            .await;
                                                    }
                                                }
                                                // Function call part
                                                else if let Some(fc) = &part.function_call {
                                                    // Gemini sends complete function calls
                                                    let call_id = format!(
                                                        "gemini_fc_{}",
                                                        uuid::Uuid::new_v4()
                                                    );
                                                    let _ = tx
                                                        .send(Ok(
                                                            StreamChunk::ToolCallComplete {
                                                                id: call_id,
                                                                name: fc.name.clone(),
                                                                arguments: fc.args.clone(),
                                                            },
                                                        ))
                                                        .await;
                                                }
                                                // Regular text part
                                                else if let Some(text) = &part.text {
                                                    if !text.is_empty() {
                                                        let _ = tx
                                                            .send(Ok(StreamChunk::Text {
                                                                delta: text.clone(),
                                                            }))
                                                            .await;
                                                    }
                                                }
                                            }
                                        }

                                        // Check for finish
                                        if let Some(reason) = &candidate.finish_reason {
                                            if reason == "STOP"
                                                || reason == "MAX_TOKENS"
                                                || reason == "SAFETY"
                                            {
                                                // Done will be sent after usage
                                            }
                                        }
                                    }
                                }

                                // Usage metadata
                                if let Some(usage) = &chunk.usage_metadata {
                                    let _ = tx
                                        .send(Ok(StreamChunk::Usage(UsageInfo {
                                            input_tokens: usage
                                                .prompt_token_count
                                                .unwrap_or(0),
                                            output_tokens: usage
                                                .candidates_token_count
                                                .unwrap_or(0),
                                            cache_read_tokens: usage
                                                .cached_content_token_count
                                                .unwrap_or(0),
                                            cache_write_tokens: 0,
                                        })))
                                        .await;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to parse Gemini chunk: {e}, data: {data}"
                                );
                            }
                        }
                    }
                }
            }

            // Always emit Done at end of stream
            let _ = tx.send(Ok(StreamChunk::Done)).await;
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

### 9.4 Message conversion

```rust
/// Convert internal Message format to Gemini content format.
fn convert_messages(messages: &[Message]) -> Vec<GeminiContent> {
    let mut result = Vec::new();

    for msg in messages {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "model",
        };

        let mut parts = Vec::new();

        for block in &msg.content {
            match block {
                ContentBlock::Text(t) => {
                    parts.push(GeminiPart::text(t));
                }
                ContentBlock::Thinking { thinking, signature } => {
                    // Preserve thinking for multi-turn context
                    let mut part = GeminiPart::thinking(thinking);
                    part.thought_signature = signature.clone();
                    parts.push(part);
                }
                ContentBlock::ToolUse { id: _, name, input } => {
                    parts.push(GeminiPart::function_call(name, input.clone()));
                }
                ContentBlock::ToolResult {
                    tool_use_id: _,
                    content,
                    is_error,
                } => {
                    // Gemini uses functionResponse parts in a "user" role message
                    // We need the tool name, but ToolResult doesn't carry it.
                    // Use a generic wrapper.
                    let response_value = if *is_error {
                        serde_json::json!({"error": content})
                    } else {
                        serde_json::json!({"result": content})
                    };
                    // Note: In practice, tool_use_id should map to the function name.
                    // The caller should ensure this mapping. For now, use a placeholder.
                    parts.push(GeminiPart::function_response("tool", response_value));
                }
                ContentBlock::Image { media_type, data } => {
                    parts.push(GeminiPart {
                        text: None,
                        thought: None,
                        thought_signature: None,
                        function_call: None,
                        function_response: None,
                        inline_data: Some(InlineData {
                            mime_type: media_type.clone(),
                            data: data.clone(),
                        }),
                    });
                }
            }
        }

        if !parts.is_empty() {
            result.push(GeminiContent {
                role: Some(role.to_string()),
                parts,
            });
        }
    }

    result
}

/// Convert internal ToolDefinition to Gemini tool declaration format.
fn convert_tools(tools: &[ToolDefinition]) -> GeminiToolDecl {
    GeminiToolDecl {
        function_declarations: tools
            .iter()
            .map(|t| GeminiFunctionDecl {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
            })
            .collect(),
    }
}
```

### 9.5 Thinking budget mapping

```rust
/// Map internal thinking_budget to Gemini ThinkingConfig.
/// Gemini accepts a direct token budget (or 0 to disable, -1 for dynamic).
fn map_thinking_budget(budget: Option<u32>) -> Option<ThinkingConfig> {
    budget.map(|b| ThinkingConfig {
        thinking_budget: match b {
            0 => Some(0),       // Disabled
            _ => Some(b as i32), // Direct token budget
        },
    })
}
```

### 9.6 Model info lookup

```rust
fn gemini_model_info(model_id: &str) -> ModelInfo {
    match model_id {
        "gemini-2.5-pro" => ModelInfo {
            id: "gemini-2.5-pro".into(),
            name: "Gemini 2.5 Pro".into(),
            provider: "gemini".into(),
            max_tokens: 65536,
            context_window: 1_000_000,
            supports_tools: true,
            supports_thinking: true,
            supports_images: true,
            input_price_per_mtok: 1.25,
            output_price_per_mtok: 10.0,
        },
        "gemini-2.5-flash" => ModelInfo {
            id: "gemini-2.5-flash".into(),
            name: "Gemini 2.5 Flash".into(),
            provider: "gemini".into(),
            max_tokens: 65536,
            context_window: 1_000_000,
            supports_tools: true,
            supports_thinking: true,
            supports_images: true,
            input_price_per_mtok: 0.15,
            output_price_per_mtok: 0.60,
        },
        other => ModelInfo {
            id: other.into(),
            name: other.into(),
            provider: "gemini".into(),
            max_tokens: 8192,
            context_window: 1_000_000,
            supports_tools: true,
            supports_thinking: false,
            supports_images: true,
            input_price_per_mtok: 0.15,
            output_price_per_mtok: 0.60,
        },
    }
}
```

### 9.7 Register in provider factory

Update `create_provider()` in `src/provider/mod.rs`:

```rust
"gemini" => Ok(Box::new(gemini::GeminiProvider::new(api_key, None)?)),
```

## Tests

```rust
#[cfg(test)]
mod gemini_tests {
    use super::*;

    #[test]
    fn test_gemini_rejects_empty_key() {
        assert!(GeminiProvider::new("", None).is_err());
    }

    #[test]
    fn test_gemini_provider_creation() {
        let p = GeminiProvider::new("test-key", None).unwrap();
        assert_eq!(p.model_info().provider, "gemini");
        assert!(p.model_info().supports_thinking);
    }

    #[test]
    fn test_gemini_custom_base_url() {
        let p = GeminiProvider::new("test-key", Some("https://custom.gemini.api")).unwrap();
        assert_eq!(p.base_url, "https://custom.gemini.api");
    }

    #[test]
    fn test_gemini_request_serialization() {
        let req = GeminiRequest {
            contents: vec![GeminiContent {
                role: Some("user".to_string()),
                parts: vec![GeminiPart::text("Hello")],
            }],
            system_instruction: Some(GeminiContent {
                role: None,
                parts: vec![GeminiPart::text("Be helpful")],
            }),
            tools: None,
            generation_config: Some(GenerationConfig {
                temperature: Some(0.7),
                max_output_tokens: Some(8192),
                thinking_config: None,
            }),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["contents"][0]["role"], "user");
        assert_eq!(json["contents"][0]["parts"][0]["text"], "Hello");
        assert_eq!(json["systemInstruction"]["parts"][0]["text"], "Be helpful");
        assert_eq!(json["generationConfig"]["temperature"], 0.7);
        assert_eq!(json["generationConfig"]["maxOutputTokens"], 8192);
    }

    #[test]
    fn test_gemini_request_serialization_with_thinking() {
        let req = GeminiRequest {
            contents: vec![],
            system_instruction: None,
            tools: None,
            generation_config: Some(GenerationConfig {
                temperature: None,
                max_output_tokens: Some(8192),
                thinking_config: Some(ThinkingConfig {
                    thinking_budget: Some(10000),
                }),
            }),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(
            json["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            10000
        );
    }

    #[test]
    fn test_gemini_stream_chunk_parsing_text() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello!"}]}}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        let candidates = chunk.candidates.unwrap();
        assert_eq!(candidates.len(), 1);
        let parts = &candidates[0].content.as_ref().unwrap().parts;
        assert_eq!(parts[0].text.as_deref(), Some("Hello!"));
        let usage = chunk.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, Some(10));
        assert_eq!(usage.candidates_token_count, Some(5));
    }

    #[test]
    fn test_gemini_thinking_chunk_parsing() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Let me think...","thought":true}]}}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        let part = &chunk.candidates.unwrap()[0]
            .content
            .as_ref()
            .unwrap()
            .parts[0];
        assert_eq!(part.thought, Some(true));
        assert_eq!(part.text.as_deref(), Some("Let me think..."));
    }

    #[test]
    fn test_gemini_thinking_with_signature() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"thinking...","thought":true,"thoughtSignature":"sig123"}]}}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        let part = &chunk.candidates.unwrap()[0]
            .content
            .as_ref()
            .unwrap()
            .parts[0];
        assert_eq!(part.thought, Some(true));
        assert_eq!(part.thought_signature.as_deref(), Some("sig123"));
    }

    #[test]
    fn test_gemini_function_call_parsing() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"read_file","args":{"path":"test.rs"}}}]}}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        let fc = chunk.candidates.unwrap()[0]
            .content
            .as_ref()
            .unwrap()
            .parts[0]
            .function_call
            .as_ref()
            .unwrap();
        assert_eq!(fc.name, "read_file");
        assert_eq!(fc.args["path"], "test.rs");
    }

    #[test]
    fn test_gemini_usage_with_thoughts() {
        let json = r#"{"usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50,"thoughtsTokenCount":200,"cachedContentTokenCount":10}}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        let usage = chunk.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, Some(100));
        assert_eq!(usage.candidates_token_count, Some(50));
        assert_eq!(usage.thoughts_token_count, Some(200));
        assert_eq!(usage.cached_content_token_count, Some(10));
    }

    #[test]
    fn test_gemini_finish_reason_parsing() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"done"}]},"finishReason":"STOP"}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(
            chunk.candidates.unwrap()[0].finish_reason.as_deref(),
            Some("STOP")
        );
    }

    #[test]
    fn test_message_conversion_basic() {
        let msgs = vec![
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::Text("hi".to_string())],
            },
            Message {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::Text("hello".to_string())],
            },
        ];
        let gemini = convert_messages(&msgs);
        assert_eq!(gemini.len(), 2);
        assert_eq!(gemini[0].role.as_deref(), Some("user"));
        assert_eq!(gemini[0].parts[0].text.as_deref(), Some("hi"));
        assert_eq!(gemini[1].role.as_deref(), Some("model"));
        assert_eq!(gemini[1].parts[0].text.as_deref(), Some("hello"));
    }

    #[test]
    fn test_message_conversion_with_thinking() {
        let msgs = vec![Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    thinking: "Let me think...".to_string(),
                    signature: Some("sig1".to_string()),
                },
                ContentBlock::Text("Answer here".to_string()),
            ],
        }];
        let gemini = convert_messages(&msgs);
        assert_eq!(gemini.len(), 1);
        assert_eq!(gemini[0].parts.len(), 2);
        assert_eq!(gemini[0].parts[0].thought, Some(true));
        assert_eq!(
            gemini[0].parts[0].thought_signature.as_deref(),
            Some("sig1")
        );
        assert_eq!(gemini[0].parts[1].text.as_deref(), Some("Answer here"));
    }

    #[test]
    fn test_message_conversion_tool_use() {
        let msgs = vec![Message {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tc1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "test.rs"}),
            }],
        }];
        let gemini = convert_messages(&msgs);
        assert_eq!(gemini[0].parts.len(), 1);
        let fc = gemini[0].parts[0].function_call.as_ref().unwrap();
        assert_eq!(fc.name, "read_file");
        assert_eq!(fc.args["path"], "test.rs");
    }

    #[test]
    fn test_message_conversion_tool_result() {
        let msgs = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tc1".to_string(),
                content: "file contents".to_string(),
                is_error: false,
            }],
        }];
        let gemini = convert_messages(&msgs);
        assert_eq!(gemini[0].role.as_deref(), Some("user"));
        let fr = gemini[0].parts[0].function_response.as_ref().unwrap();
        assert_eq!(fr.response["result"], "file contents");
    }

    #[test]
    fn test_message_conversion_tool_result_error() {
        let msgs = vec![Message {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tc1".to_string(),
                content: "file not found".to_string(),
                is_error: true,
            }],
        }];
        let gemini = convert_messages(&msgs);
        let fr = gemini[0].parts[0].function_response.as_ref().unwrap();
        assert_eq!(fr.response["error"], "file not found");
    }

    #[test]
    fn test_tool_conversion() {
        let tools = vec![
            ToolDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            },
            ToolDefinition {
                name: "write_file".to_string(),
                description: "Write a file".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        ];
        let decl = convert_tools(&tools);
        assert_eq!(decl.function_declarations.len(), 2);
        assert_eq!(decl.function_declarations[0].name, "read_file");
        assert_eq!(decl.function_declarations[1].name, "write_file");
    }

    #[test]
    fn test_thinking_budget_mapping_disabled() {
        let config = map_thinking_budget(Some(0));
        assert!(config.is_some());
        assert_eq!(config.unwrap().thinking_budget, Some(0));
    }

    #[test]
    fn test_thinking_budget_mapping_specific() {
        let config = map_thinking_budget(Some(10000));
        assert!(config.is_some());
        assert_eq!(config.unwrap().thinking_budget, Some(10000));
    }

    #[test]
    fn test_thinking_budget_mapping_none() {
        let config = map_thinking_budget(None);
        assert!(config.is_none());
    }

    #[test]
    fn test_model_info_gemini_25_pro() {
        let info = gemini_model_info("gemini-2.5-pro");
        assert_eq!(info.id, "gemini-2.5-pro");
        assert!(info.supports_thinking);
        assert_eq!(info.context_window, 1_000_000);
    }

    #[test]
    fn test_model_info_gemini_25_flash() {
        let info = gemini_model_info("gemini-2.5-flash");
        assert!(info.supports_thinking);
        assert_eq!(info.input_price_per_mtok, 0.15);
    }

    #[test]
    fn test_model_info_unknown() {
        let info = gemini_model_info("gemini-future");
        assert_eq!(info.id, "gemini-future");
        assert!(!info.supports_thinking);
    }

    #[test]
    fn test_gemini_part_constructors() {
        let text = GeminiPart::text("hello");
        assert_eq!(text.text.as_deref(), Some("hello"));
        assert!(text.thought.is_none());

        let thinking = GeminiPart::thinking("hmm");
        assert_eq!(thinking.thought, Some(true));

        let fc = GeminiPart::function_call("test", serde_json::json!({}));
        assert!(fc.function_call.is_some());

        let fr = GeminiPart::function_response("test", serde_json::json!({"ok": true}));
        assert!(fr.function_response.is_some());
    }

    #[tokio::test]
    #[ignore]
    async fn test_gemini_streaming_integration() {
        let key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY not set");
        let provider = GeminiProvider::new(&key, None).unwrap();
        let config = ModelConfig {
            model_id: "gemini-2.5-flash".to_string(),
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
        while let Some(chunk) = stream.next().await {
            if matches!(chunk.unwrap(), StreamChunk::Text { .. }) {
                got_text = true;
            }
        }
        assert!(got_text);
    }
}
```

## Acceptance Criteria
- [ ] Gemini SSE streaming works with `streamGenerateContent` endpoint
- [ ] Text parts correctly emitted as `StreamChunk::Text`
- [ ] Thinking parts (with `thought: true`) emitted as `StreamChunk::Thinking`
- [ ] Thinking signatures preserved and forwarded
- [ ] Function call parts emitted as `StreamChunk::ToolCallComplete`
- [ ] Message conversion handles user, model, tool use, tool result, thinking, and images
- [ ] Thinking budget configuration serialized as `thinkingConfig.thinkingBudget`
- [ ] Usage metadata (including thought tokens and cached tokens) tracked
- [ ] Cancellation via CancellationToken stops the stream
- [ ] Provider factory handles "gemini" provider name
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All unit tests pass
