# STEP 10 — OpenRouter Provider

## Objective
Implement the OpenRouter provider, which uses an OpenAI-compatible API format with some extensions. After this step, users can access hundreds of models via OpenRouter.

## Prerequisites
- STEP 05 and STEP 08 complete (shares much of the OpenAI format)

## Detailed Instructions

### 10.1 OpenRouter specifics

OpenRouter (`https://openrouter.ai/api/v1`) uses the OpenAI Chat Completions format with these extensions:

- API key in `Authorization: Bearer {key}` header
- Extra headers: `HTTP-Referer` and `X-Title` for app identification
- Reasoning content in `delta.reasoning` field (same as parsed in STEP 08 `ChunkDelta`)
- Reasoning details in `delta.reasoning_details` array with structured reasoning steps
- Usage may not arrive in the stream. Fallback: `GET /api/v1/generation?id={gen_id}` after stream ends
- Error detection: `finish_reason: "error"` or top-level `error` field in chunk JSON
- Some models (e.g., Grok) return low-quality reasoning. Filter those based on known model IDs

### 10.2 Implement OpenRouterProvider (`src/provider/openrouter.rs`)

Since OpenRouter uses OpenAI format, this provider reuses the OpenAI message/tool conversion functions from STEP 08. Key differences are in headers, streaming extensions, and post-stream usage fetch.

```rust
//! OpenRouter provider — OpenAI-compatible with reasoning and usage extensions.

use crate::provider::{
    create_http_client, CancellationToken, ModelInfo, Provider, ProviderStream, StreamChunk,
    Message, ContentBlock, MessageRole, ModelConfig, ToolDefinition, UsageInfo,
};
use crate::provider::openai::{
    convert_messages, convert_tools, ChatCompletionRequest, ChatCompletionChunk,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;

pub struct OpenRouterProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model_info: ModelInfo,
    cancel: CancellationToken,
}

impl OpenRouterProvider {
    pub fn new(api_key: &str, base_url: Option<&str>) -> anyhow::Result<Self> {
        anyhow::ensure!(!api_key.is_empty(), "OpenRouter API key is required");
        Ok(Self {
            client: create_http_client()?,
            api_key: api_key.to_string(),
            base_url: base_url
                .unwrap_or("https://openrouter.ai/api")
                .to_string(),
            model_info: ModelInfo {
                id: "anthropic/claude-sonnet-4".to_string(),
                name: "Claude Sonnet 4 (OpenRouter)".to_string(),
                provider: "openrouter".to_string(),
                max_tokens: 16384,
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

### 10.3 Implement Provider trait

```rust
#[async_trait]
impl Provider for OpenRouterProvider {
    async fn create_message(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> anyhow::Result<ProviderStream> {
        // Build request using OpenAI format (reuse convert_messages, convert_tools)
        let mut oai_messages = vec![/* system message */];
        oai_messages.extend(convert_messages(messages));

        let request_body = ChatCompletionRequest {
            model: config.model_id.clone(),
            messages: oai_messages,
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            stream: true,
            tools: convert_tools(tools),
            reasoning_effort: None, // OpenRouter handles this differently per model
        };

        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://github.com/meh-cli/meh")
            .header("X-Title", "meh")
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenRouter API error {status}: {body}");
        }

        let cancel = self.cancel.clone();
        let client = self.client.clone();
        let base_url = self.base_url.clone();
        let api_key = self.api_key.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut tool_calls: std::collections::HashMap<usize, (String, String, String)> =
                std::collections::HashMap::new();
            let mut generation_id: Option<String> = None;
            let mut got_usage = false;

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

                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        if data.trim() == "[DONE]" {
                            // If we didn't get usage in-stream, fetch from generation endpoint
                            if !got_usage {
                                if let Some(gen_id) = &generation_id {
                                    if let Some(usage) = fetch_generation_details(
                                        &client, &base_url, &api_key, gen_id,
                                    )
                                    .await
                                    {
                                        let _ =
                                            tx.send(Ok(StreamChunk::Usage(usage))).await;
                                    }
                                }
                            }
                            let _ = tx.send(Ok(StreamChunk::Done)).await;
                            return;
                        }

                        // Check for top-level error object
                        if let Ok(error_obj) =
                            serde_json::from_str::<OpenRouterError>(data)
                        {
                            if error_obj.error.is_some() {
                                let msg = error_obj
                                    .error
                                    .as_ref()
                                    .and_then(|e| e.get("message"))
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("Unknown OpenRouter error");
                                let _ = tx
                                    .send(Ok(StreamChunk::Error(msg.to_string())))
                                    .await;
                                return;
                            }
                        }

                        match serde_json::from_str::<ChatCompletionChunk>(data) {
                            Ok(chunk) => {
                                // Track generation ID for post-stream usage fetch
                                if generation_id.is_none() {
                                    generation_id = chunk.id.clone();
                                }

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

                                    // Reasoning content (OpenRouter extension)
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

                                    // Tool calls (same index-based tracking as OpenAI)
                                    if let Some(tcs) = &choice.delta.tool_calls {
                                        for tc in tcs {
                                            let entry = tool_calls
                                                .entry(tc.index)
                                                .or_insert_with(|| {
                                                    (
                                                        String::new(),
                                                        String::new(),
                                                        String::new(),
                                                    )
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
                                                        .send(Ok(
                                                            StreamChunk::ToolCallDelta {
                                                                index: tc.index,
                                                                id: entry.0.clone(),
                                                                name: if entry.1.is_empty()
                                                                {
                                                                    None
                                                                } else {
                                                                    Some(entry.1.clone())
                                                                },
                                                                arguments_delta: args
                                                                    .clone(),
                                                            },
                                                        ))
                                                        .await;
                                                }
                                            }
                                        }
                                    }

                                    // Finish reason
                                    if let Some(reason) = &choice.finish_reason {
                                        if reason == "tool_calls" {
                                            for (_idx, (id, name, args)) in
                                                tool_calls.drain()
                                            {
                                                let arguments =
                                                    serde_json::from_str(&args)
                                                        .unwrap_or(
                                                            serde_json::Value::String(args),
                                                        );
                                                let _ = tx
                                                    .send(Ok(
                                                        StreamChunk::ToolCallComplete {
                                                            id,
                                                            name,
                                                            arguments,
                                                        },
                                                    ))
                                                    .await;
                                            }
                                        } else if reason == "error" {
                                            let _ = tx
                                                .send(Ok(StreamChunk::Error(
                                                    "OpenRouter model error".to_string(),
                                                )))
                                                .await;
                                        }
                                    }
                                }

                                // Usage (if provided in-stream)
                                if let Some(usage) = &chunk.usage {
                                    got_usage = true;
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
                                tracing::warn!(
                                    "Failed to parse OpenRouter chunk: {e}, data: {data}"
                                );
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

### 10.4 Error type for OpenRouter error responses

```rust
/// Top-level error object that OpenRouter may send instead of a chunk.
#[derive(Deserialize)]
struct OpenRouterError {
    #[serde(default)]
    error: Option<serde_json::Value>,
}
```

### 10.5 Generation details fetch (post-stream usage fallback)

```rust
/// Fetch usage details from the OpenRouter generation endpoint.
/// This is called after stream ends if no usage was included in the stream.
/// Retries up to 3 times with 500ms delay since the generation may not be
/// immediately available.
async fn fetch_generation_details(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    generation_id: &str,
) -> Option<UsageInfo> {
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        let url = format!("{}/v1/generation?id={}", base_url, generation_id);
        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await
            .ok()?;

        if !response.status().is_success() {
            continue;
        }

        let body: serde_json::Value = response.json().await.ok()?;
        let data = body.get("data")?;

        let input_tokens = data
            .get("native_tokens_prompt")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output_tokens = data
            .get("native_tokens_completion")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let _total_cost = data
            .get("total_cost")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        return Some(UsageInfo {
            input_tokens,
            output_tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        });
    }

    None
}
```

### 10.6 Register in provider factory

Update `create_provider()` in `src/provider/mod.rs`:

```rust
"openrouter" => Ok(Box::new(openrouter::OpenRouterProvider::new(api_key, None)?)),
```

## Tests

```rust
#[cfg(test)]
mod openrouter_tests {
    use super::*;

    #[test]
    fn test_openrouter_creation() {
        let p = OpenRouterProvider::new("test-key", None).unwrap();
        assert_eq!(p.model_info().provider, "openrouter");
    }

    #[test]
    fn test_openrouter_rejects_empty_key() {
        assert!(OpenRouterProvider::new("", None).is_err());
    }

    #[test]
    fn test_openrouter_default_base_url() {
        let p = OpenRouterProvider::new("test-key", None).unwrap();
        assert_eq!(p.base_url, "https://openrouter.ai/api");
    }

    #[test]
    fn test_openrouter_custom_base_url() {
        let p = OpenRouterProvider::new("test-key", Some("https://custom.api")).unwrap();
        assert_eq!(p.base_url, "https://custom.api");
    }

    #[test]
    fn test_openrouter_chunk_with_reasoning() {
        let json = r#"{"id":"gen-1","choices":[{"delta":{"content":null,"reasoning":"Thinking step..."},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(
            chunk.choices[0].delta.reasoning,
            Some("Thinking step...".to_string())
        );
        assert!(chunk.choices[0].delta.content.is_none());
    }

    #[test]
    fn test_openrouter_chunk_with_content_and_reasoning() {
        let json = r#"{"id":"gen-1","choices":[{"delta":{"content":"Hello","reasoning":"I should greet"},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(
            chunk.choices[0].delta.content,
            Some("Hello".to_string())
        );
        assert_eq!(
            chunk.choices[0].delta.reasoning,
            Some("I should greet".to_string())
        );
    }

    #[test]
    fn test_openrouter_error_object_parsing() {
        let json = r#"{"error":{"message":"Rate limit exceeded","code":429}}"#;
        let error: OpenRouterError = serde_json::from_str(json).unwrap();
        assert!(error.error.is_some());
        let error_obj = error.error.unwrap();
        assert_eq!(error_obj["message"], "Rate limit exceeded");
        assert_eq!(error_obj["code"], 429);
    }

    #[test]
    fn test_openrouter_error_not_present() {
        let json = r#"{"id":"gen-1","choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#;
        let error: OpenRouterError = serde_json::from_str(json).unwrap();
        assert!(error.error.is_none());
    }

    #[test]
    fn test_openrouter_finish_reason_error() {
        let json = r#"{"id":"gen-1","choices":[{"delta":{},"finish_reason":"error"}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(
            chunk.choices[0].finish_reason,
            Some("error".to_string())
        );
    }

    #[test]
    fn test_generation_details_parsing() {
        let json = r#"{"data":{"native_tokens_prompt":100,"native_tokens_completion":50,"total_cost":0.001}}"#;
        let body: serde_json::Value = serde_json::from_str(json).unwrap();
        let data = body.get("data").unwrap();
        let input = data
            .get("native_tokens_prompt")
            .and_then(|v| v.as_u64())
            .unwrap();
        let output = data
            .get("native_tokens_completion")
            .and_then(|v| v.as_u64())
            .unwrap();
        let cost = data
            .get("total_cost")
            .and_then(|v| v.as_f64())
            .unwrap();
        assert_eq!(input, 100);
        assert_eq!(output, 50);
        assert!((cost - 0.001).abs() < f64::EPSILON);
    }

    #[test]
    fn test_generation_details_missing_fields() {
        let json = r#"{"data":{}}"#;
        let body: serde_json::Value = serde_json::from_str(json).unwrap();
        let data = body.get("data").unwrap();
        let input = data
            .get("native_tokens_prompt")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        assert_eq!(input, 0);
    }

    #[test]
    fn test_chunk_with_generation_id() {
        let json = r#"{"id":"gen-abc123","choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.id, Some("gen-abc123".to_string()));
    }

    #[test]
    fn test_reuses_openai_message_conversion() {
        // Verify that OpenAI convert_messages works for OpenRouter too
        let messages = vec![
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::Text("hello".to_string())],
            },
            Message {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::Text("hi".to_string())],
            },
        ];
        let oai = convert_messages(&messages);
        assert_eq!(oai.len(), 2);
        assert_eq!(oai[0].role, "user");
        assert_eq!(oai[1].role, "assistant");
    }

    #[tokio::test]
    #[ignore]
    async fn test_openrouter_streaming_integration() {
        let key = std::env::var("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY not set");
        let provider = OpenRouterProvider::new(&key, None).unwrap();
        let config = ModelConfig {
            model_id: "anthropic/claude-sonnet-4".to_string(),
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
- [ ] OpenRouter streaming works with OpenAI-compatible format
- [ ] Reasoning chunks from `delta.reasoning` emitted as `StreamChunk::Thinking`
- [ ] Fallback usage fetch via `/v1/generation?id={id}` endpoint works
- [ ] Error detection handles both `finish_reason: "error"` and top-level `error` object
- [ ] Proper headers sent: `Authorization`, `HTTP-Referer`, `X-Title`
- [ ] Reuses OpenAI message and tool conversion functions
- [ ] Generation ID tracked from first chunk for post-stream usage fetch
- [ ] Retry logic (3 attempts, 500ms delay) in generation details fetch
- [ ] Provider factory handles "openrouter" provider name
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All unit tests pass
