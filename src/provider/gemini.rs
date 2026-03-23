//! Google Gemini provider — streaming with tool and thinking support.
//!
//! Connects to the Gemini `streamGenerateContent` endpoint via SSE.
//! Tool calls arrive as complete `functionCall` parts (not streamed
//! incrementally). Thinking uses the `thought: true` flag on text parts.

use super::common::create_http_client;
use super::{
    ContentBlock, Message, MessageRole, ModelConfig, ModelInfo, Provider, ProviderStream,
    StreamChunk, ToolDefinition, UsageInfo,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// Google Gemini provider.
pub struct GeminiProvider {
    client: reqwest::Client,
    api_key: String,
    pub(crate) base_url: String,
    model_info: ModelInfo,
    cancel: CancellationToken,
}

impl GeminiProvider {
    /// Creates a new Gemini provider.
    ///
    /// # Errors
    /// Returns an error if the API key is empty.
    pub fn new(api_key: &str, base_url: Option<&str>) -> anyhow::Result<Self> {
        anyhow::ensure!(!api_key.is_empty(), "Gemini API key is required");
        Ok(Self {
            client: create_http_client()?,
            api_key: api_key.to_string(),
            base_url: base_url.unwrap_or(DEFAULT_BASE_URL).to_string(),
            model_info: gemini_model_info("gemini-2.5-flash"),
            cancel: CancellationToken::new(),
        })
    }
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl Provider for GeminiProvider {
    /// Streams a response from the Gemini `streamGenerateContent` endpoint.
    async fn create_message(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> anyhow::Result<ProviderStream> {
        let contents = convert_messages(messages);
        let system_instruction = GeminiContent {
            role: None,
            parts: vec![GeminiPart::text(system_prompt)],
        };

        let gemini_tools = if tools.is_empty() {
            None
        } else {
            Some(vec![convert_tools(tools)])
        };

        let generation_config = GenerationConfig {
            temperature: config.temperature,
            max_output_tokens: Some(config.max_tokens),
            thinking_config: config.thinking_budget.map(|b| ThinkingConfig {
                thinking_budget: Some(i32::try_from(b).unwrap_or(i32::MAX)),
            }),
        };

        let request_body = GeminiRequest {
            contents,
            system_instruction: Some(system_instruction),
            tools: gemini_tools,
            generation_config: Some(generation_config),
        };

        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?key={}&alt=sse",
            self.base_url, config.model_id, self.api_key
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

        let stream = async_stream::stream! {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();

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
                        if let Ok(chunk) = serde_json::from_str::<GeminiStreamChunk>(data) {
                            for event in process_gemini_chunk(&chunk) {
                                yield Ok(event);
                            }
                        }
                    }
                }
            }

            yield Ok(StreamChunk::Done);
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

/// Processes a single Gemini SSE chunk and returns `StreamChunk` events.
fn process_gemini_chunk(chunk: &GeminiStreamChunk) -> Vec<StreamChunk> {
    let mut events = Vec::new();

    if let Some(candidates) = &chunk.candidates {
        for candidate in candidates {
            if let Some(content) = &candidate.content {
                for part in &content.parts {
                    if part.thought == Some(true) {
                        if let Some(text) = &part.text {
                            events.push(StreamChunk::Thinking {
                                delta: text.clone(),
                                signature: part.thought_signature.clone(),
                                redacted: false,
                            });
                        }
                    } else if let Some(fc) = &part.function_call {
                        let call_id = format!("gemini_fc_{}", uuid::Uuid::new_v4());
                        events.push(StreamChunk::ToolCallComplete {
                            id: call_id,
                            name: fc.name.clone(),
                            arguments: fc.args.clone(),
                        });
                    } else if let Some(text) = &part.text {
                        if !text.is_empty() {
                            events.push(StreamChunk::Text {
                                delta: text.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    if let Some(usage) = &chunk.usage_metadata {
        events.push(StreamChunk::Usage(UsageInfo {
            input_tokens: usage.prompt_token_count.unwrap_or(0),
            output_tokens: usage.candidates_token_count.unwrap_or(0),
            cache_read_tokens: usage.cached_content_token_count,
            cache_write_tokens: None,
            thinking_tokens: usage.thoughts_token_count,
            total_cost: None,
        }));
    }

    events
}

/// Converts internal [`Message`] list to Gemini content format.
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
                ContentBlock::Thinking { text, signature } => {
                    let mut part = GeminiPart::thinking(text);
                    part.thought_signature.clone_from(signature);
                    parts.push(part);
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    parts.push(GeminiPart::function_call(name, input.clone()));
                }
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    let response_value = if *is_error {
                        serde_json::json!({"error": content})
                    } else {
                        serde_json::json!({"result": content})
                    };
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
                            data: base64_encode(data),
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

/// Converts [`ToolDefinition`] list to Gemini tool declaration format.
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

/// Returns [`ModelInfo`] for known Gemini models, with a sensible fallback.
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
            cache_read_price_per_mtok: None,
            cache_write_price_per_mtok: None,
            thinking_price_per_mtok: None,
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
            cache_read_price_per_mtok: None,
            cache_write_price_per_mtok: None,
            thinking_price_per_mtok: None,
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
            cache_read_price_per_mtok: None,
            cache_write_price_per_mtok: None,
            thinking_price_per_mtok: None,
        },
    }
}

/// Base64-encodes binary data for inline image parts.
fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

// ── Request types ──────────────────────────────────────────────────

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
    /// Creates a plain text part.
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

    /// Creates a thinking/reasoning text part.
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

    /// Creates a function call part.
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

    /// Creates a function response part.
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
    data: String,
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

// ── Streaming response types ───────────────────────────────────────

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
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ContentBlock, Message, MessageRole, ToolDefinition};

    #[test]
    fn rejects_empty_key() {
        assert!(GeminiProvider::new("", None).is_err());
    }

    #[test]
    fn provider_creation() {
        let p = GeminiProvider::new("test-key", None).unwrap();
        assert_eq!(p.model_info().provider, "gemini");
        assert!(p.model_info().supports_thinking);
    }

    #[test]
    fn custom_base_url() {
        let p = GeminiProvider::new("test-key", Some("https://custom.gemini.api")).unwrap();
        assert_eq!(p.base_url, "https://custom.gemini.api");
    }

    #[test]
    fn request_serialization() {
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
    fn request_serialization_with_thinking() {
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
    fn stream_chunk_parsing_text() {
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
    fn thinking_chunk_parsing() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Let me think...","thought":true}]}}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        let candidates = chunk.candidates.unwrap();
        let part = &candidates[0].content.as_ref().unwrap().parts[0];
        assert_eq!(part.thought, Some(true));
        assert_eq!(part.text.as_deref(), Some("Let me think..."));
    }

    #[test]
    fn thinking_with_signature() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"thinking...","thought":true,"thoughtSignature":"sig123"}]}}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        let candidates = chunk.candidates.unwrap();
        let part = &candidates[0].content.as_ref().unwrap().parts[0];
        assert_eq!(part.thought, Some(true));
        assert_eq!(part.thought_signature.as_deref(), Some("sig123"));
    }

    #[test]
    fn function_call_parsing() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"read_file","args":{"path":"test.rs"}}}]}}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        let candidates = chunk.candidates.unwrap();
        let fc = candidates[0].content.as_ref().unwrap().parts[0]
            .function_call
            .as_ref()
            .unwrap();
        assert_eq!(fc.name, "read_file");
        assert_eq!(fc.args["path"], "test.rs");
    }

    #[test]
    fn usage_with_thoughts() {
        let json = r#"{"usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50,"thoughtsTokenCount":200,"cachedContentTokenCount":10}}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        let usage = chunk.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, Some(100));
        assert_eq!(usage.candidates_token_count, Some(50));
        assert_eq!(usage.thoughts_token_count, Some(200));
        assert_eq!(usage.cached_content_token_count, Some(10));
    }

    #[test]
    fn finish_reason_parsing() {
        let json = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"done"}]},"finishReason":"STOP"}]}"#;
        let chunk: GeminiStreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(
            chunk.candidates.unwrap()[0].finish_reason.as_deref(),
            Some("STOP")
        );
    }

    #[test]
    fn message_conversion_basic() {
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
    fn message_conversion_with_thinking() {
        let msgs = vec![Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "Let me think...".to_string(),
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
    fn message_conversion_tool_use() {
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
    fn message_conversion_tool_result() {
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
    fn message_conversion_tool_result_error() {
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
    fn tool_conversion() {
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
    fn model_info_gemini_25_pro() {
        let info = gemini_model_info("gemini-2.5-pro");
        assert_eq!(info.id, "gemini-2.5-pro");
        assert!(info.supports_thinking);
        assert_eq!(info.context_window, 1_000_000);
    }

    #[test]
    fn model_info_gemini_25_flash() {
        let info = gemini_model_info("gemini-2.5-flash");
        assert!(info.supports_thinking);
        assert_eq!(info.input_price_per_mtok, 0.15);
    }

    #[test]
    fn model_info_unknown() {
        let info = gemini_model_info("gemini-future");
        assert_eq!(info.id, "gemini-future");
        assert!(!info.supports_thinking);
    }

    #[test]
    fn part_constructors() {
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
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::provider::{ContentBlock, Message, MessageRole, ModelConfig};
    use futures::StreamExt;

    #[tokio::test]
    #[ignore]
    async fn gemini_streaming() {
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
