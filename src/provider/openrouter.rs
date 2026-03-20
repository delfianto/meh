//! `OpenRouter` provider — `OpenAI`-compatible with reasoning and usage extensions.
//!
//! Uses the `OpenAI` Chat Completions format with extra headers for app
//! identification and a post-stream generation endpoint fallback for
//! usage data when it's not included in the SSE stream.

use super::common::create_http_client;
use super::openai::{self, ChatCompletionChunk, ChatCompletionRequest, OaiMessage};
use super::{
    Message, ModelConfig, ModelInfo, Provider, ProviderStream, StreamChunk, ToolDefinition,
    UsageInfo,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api";

/// `OpenRouter` provider.
pub struct OpenRouterProvider {
    client: reqwest::Client,
    api_key: String,
    pub(crate) base_url: String,
    model_info: ModelInfo,
    cancel: CancellationToken,
}

impl OpenRouterProvider {
    /// Creates a new `OpenRouter` provider.
    ///
    /// # Errors
    /// Returns an error if the API key is empty.
    pub fn new(api_key: &str, base_url: Option<&str>) -> anyhow::Result<Self> {
        anyhow::ensure!(!api_key.is_empty(), "OpenRouter API key is required");
        Ok(Self {
            client: create_http_client()?,
            api_key: api_key.to_string(),
            base_url: base_url.unwrap_or(DEFAULT_BASE_URL).to_string(),
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

#[async_trait]
#[allow(clippy::too_many_lines)]
impl Provider for OpenRouterProvider {
    /// Streams a response via the `OpenRouter` `OpenAI`-compatible endpoint.
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
        oai_messages.extend(openai::convert_messages(messages));

        let request_body = ChatCompletionRequest {
            model: config.model_id.clone(),
            messages: oai_messages,
            temperature: config.temperature,
            max_tokens: Some(config.max_tokens),
            stream: true,
            tools: openai::convert_tools(tools),
            reasoning_effort: None,
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

        let stream = async_stream::stream! {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut tool_calls: HashMap<usize, (String, String, String)> = HashMap::new();
            let mut generation_id: Option<String> = None;
            let mut got_usage = false;

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
                            if !got_usage {
                                if let Some(ref gen_id) = generation_id {
                                    if let Some(usage) = fetch_generation_usage(
                                        &client, &base_url, &api_key, gen_id,
                                    ).await {
                                        yield Ok(StreamChunk::Usage(usage));
                                    }
                                }
                            }
                            yield Ok(StreamChunk::Done);
                            return;
                        }

                        if let Ok(error_obj) = serde_json::from_str::<OpenRouterError>(data) {
                            if let Some(ref err) = error_obj.error {
                                let msg = err
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("Unknown OpenRouter error");
                                yield Ok(StreamChunk::Error(msg.to_string()));
                                return;
                            }
                        }

                        if let Ok(chunk) = serde_json::from_str::<ChatCompletionChunk>(data) {
                            if generation_id.is_none() {
                                generation_id.clone_from(&chunk.id);
                            }

                            for event in openai::process_chunk(&chunk, &mut tool_calls) {
                                if matches!(&event, StreamChunk::Usage(_)) {
                                    got_usage = true;
                                }
                                yield Ok(event);
                            }

                            for choice in &chunk.choices {
                                if let Some(reason) = &choice.finish_reason {
                                    if reason == "error" {
                                        yield Ok(StreamChunk::Error(
                                            "OpenRouter model error".to_string(),
                                        ));
                                        return;
                                    }
                                }
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

/// Top-level error object that `OpenRouter` may send instead of a chunk.
#[derive(Deserialize)]
struct OpenRouterError {
    #[serde(default)]
    error: Option<serde_json::Value>,
}

/// Fetches usage details from the `OpenRouter` generation endpoint.
///
/// Retries up to 3 times with 500ms delay since the generation data
/// may not be immediately available after stream ends.
async fn fetch_generation_usage(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    generation_id: &str,
) -> Option<UsageInfo> {
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        let url = format!("{base_url}/v1/generation?id={generation_id}");
        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}"))
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
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let output_tokens = data
            .get("native_tokens_completion")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let total_cost = data.get("total_cost").and_then(serde_json::Value::as_f64);

        return Some(UsageInfo {
            input_tokens,
            output_tokens,
            cache_read_tokens: None,
            cache_write_tokens: None,
            thinking_tokens: None,
            total_cost,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::openai::ChatCompletionChunk;
    use crate::provider::{ContentBlock, Message, MessageRole};

    #[test]
    fn provider_creation() {
        let p = OpenRouterProvider::new("test-key", None).unwrap();
        assert_eq!(p.model_info().provider, "openrouter");
    }

    #[test]
    fn rejects_empty_key() {
        assert!(OpenRouterProvider::new("", None).is_err());
    }

    #[test]
    fn default_base_url() {
        let p = OpenRouterProvider::new("test-key", None).unwrap();
        assert_eq!(p.base_url, "https://openrouter.ai/api");
    }

    #[test]
    fn custom_base_url() {
        let p = OpenRouterProvider::new("test-key", Some("https://custom.api")).unwrap();
        assert_eq!(p.base_url, "https://custom.api");
    }

    #[test]
    fn chunk_with_reasoning() {
        let json = r#"{"id":"gen-1","choices":[{"delta":{"content":null,"reasoning":"Thinking step..."},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(
            chunk.choices[0].delta.reasoning,
            Some("Thinking step...".to_string())
        );
        assert!(chunk.choices[0].delta.content.is_none());
    }

    #[test]
    fn chunk_with_content_and_reasoning() {
        let json = r#"{"id":"gen-1","choices":[{"delta":{"content":"Hello","reasoning":"I should greet"},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices[0].delta.content, Some("Hello".to_string()));
        assert_eq!(
            chunk.choices[0].delta.reasoning,
            Some("I should greet".to_string())
        );
    }

    #[test]
    fn error_object_parsing() {
        let json = r#"{"error":{"message":"Rate limit exceeded","code":429}}"#;
        let error: OpenRouterError = serde_json::from_str(json).unwrap();
        assert!(error.error.is_some());
        let error_obj = error.error.unwrap();
        assert_eq!(error_obj["message"], "Rate limit exceeded");
        assert_eq!(error_obj["code"], 429);
    }

    #[test]
    fn error_not_present() {
        let json = r#"{"id":"gen-1","choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#;
        let error: OpenRouterError = serde_json::from_str(json).unwrap();
        assert!(error.error.is_none());
    }

    #[test]
    fn finish_reason_error() {
        let json = r#"{"id":"gen-1","choices":[{"delta":{},"finish_reason":"error"}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices[0].finish_reason, Some("error".to_string()));
    }

    #[test]
    fn generation_details_parsing() {
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
        let cost = data.get("total_cost").and_then(|v| v.as_f64()).unwrap();
        assert_eq!(input, 100);
        assert_eq!(output, 50);
        assert!((cost - 0.001).abs() < f64::EPSILON);
    }

    #[test]
    fn generation_details_missing_fields() {
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
    fn chunk_with_generation_id() {
        let json =
            r#"{"id":"gen-abc123","choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#;
        let chunk: ChatCompletionChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.id, Some("gen-abc123".to_string()));
    }

    #[test]
    fn reuses_openai_message_conversion() {
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
        let oai = openai::convert_messages(&messages);
        assert_eq!(oai.len(), 2);
        assert_eq!(oai[0].role, "user");
        assert_eq!(oai[1].role, "assistant");
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::provider::{ContentBlock, Message, MessageRole, ModelConfig};
    use futures::StreamExt;

    #[tokio::test]
    #[ignore]
    async fn openrouter_streaming() {
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
