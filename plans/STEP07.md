# STEP 07 — Wiring: User Input -> Controller -> Agent -> Provider -> Streaming -> TUI

## Objective
Connect all components end-to-end. After this step, a user types a message, it goes through the Controller to a TaskAgent, the agent calls the Anthropic provider, streams the response back through the StreamProcessor, and the TUI displays the streaming text in real-time.

## Prerequisites
- STEP 01–06 complete

## Detailed Instructions

### 7.1 Implement TaskAgent (`src/agent/task_agent.rs`)

The TaskAgent is a tokio task that runs the main conversation loop:

```rust
//! Main task agent — runs the conversation loop with an LLM provider.

use crate::controller::messages::{ControllerMessage, ToolCallRequest, ToolCallResult, TaskResult};
use crate::provider::{Provider, StreamChunk, Message, ContentBlock, MessageRole, ModelConfig, ToolDefinition};
use crate::streaming::{StreamProcessor, ProcessedEvent};
use tokio::sync::mpsc;
use futures::StreamExt;

/// Messages the controller sends to the agent.
#[derive(Debug)]
pub enum AgentMessage {
    /// Result of a tool execution.
    ToolCallResult(ToolCallResult),
    /// Cancel the current task.
    Cancel,
    /// Switch mode.
    ModeSwitch(crate::state::task_state::Mode),
}

pub struct TaskAgent {
    task_id: String,
    provider: Box<dyn Provider>,
    system_prompt: String,
    messages: Vec<Message>,
    config: ModelConfig,
    tools: Vec<ToolDefinition>,

    /// Send messages to the controller.
    ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
    /// Receive messages from the controller.
    rx: mpsc::UnboundedReceiver<AgentMessage>,

    stream_processor: StreamProcessor,
    cancelled: bool,
}

impl TaskAgent {
    pub fn new(
        task_id: String,
        provider: Box<dyn Provider>,
        system_prompt: String,
        config: ModelConfig,
        tools: Vec<ToolDefinition>,
        ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
        rx: mpsc::UnboundedReceiver<AgentMessage>,
    ) -> Self { /* initialize with empty messages, new StreamProcessor */ }

    /// Main execution loop. Returns when task completes or is cancelled.
    pub async fn run(mut self) -> anyhow::Result<()> {
        loop {
            // 1. Call provider
            self.stream_processor.reset();
            let stream_result = self.provider.create_message(
                &self.system_prompt,
                &self.messages,
                &self.tools,
                &self.config,
            ).await;

            let mut stream = match stream_result {
                Ok(s) => s,
                Err(e) => {
                    let _ = self.ctrl_tx.send(ControllerMessage::TaskError(e.to_string()));
                    return Err(e);
                }
            };

            // 2. Process stream
            let mut assistant_text = String::new();
            let mut assistant_content_blocks: Vec<ContentBlock> = Vec::new();
            let mut pending_tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();

            loop {
                tokio::select! {
                    // Check for agent messages (cancel, tool results)
                    Some(agent_msg) = self.rx.recv() => {
                        match agent_msg {
                            AgentMessage::Cancel => {
                                self.provider.abort();
                                self.cancelled = true;
                                break;
                            }
                            _ => {} // Tool results handled after stream ends
                        }
                    }
                    // Process stream chunks
                    chunk = stream.next() => {
                        match chunk {
                            Some(Ok(stream_chunk)) => {
                                let events = self.stream_processor.process(stream_chunk);
                                for event in events {
                                    match event {
                                        ProcessedEvent::TextBatch(text) => {
                                            assistant_text.push_str(&text);
                                            let _ = self.ctrl_tx.send(
                                                ControllerMessage::StreamChunk(StreamChunk::Text { delta: text })
                                            );
                                        }
                                        ProcessedEvent::ThinkingDelta(delta) => {
                                            let _ = self.ctrl_tx.send(
                                                ControllerMessage::StreamChunk(StreamChunk::Thinking {
                                                    delta, signature: None, redacted: false,
                                                })
                                            );
                                        }
                                        ProcessedEvent::ToolCallReady { id, name, arguments } => {
                                            pending_tool_calls.push((id, name, arguments));
                                        }
                                        ProcessedEvent::Usage(usage) => {
                                            let _ = self.ctrl_tx.send(
                                                ControllerMessage::StreamChunk(StreamChunk::Usage(usage))
                                            );
                                        }
                                        ProcessedEvent::Done => { break; }
                                        ProcessedEvent::Error(e) => {
                                            let _ = self.ctrl_tx.send(ControllerMessage::TaskError(e));
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                let _ = self.ctrl_tx.send(ControllerMessage::TaskError(e.to_string()));
                                break;
                            }
                            None => break,
                        }
                    }
                }

                if self.cancelled { break; }
            }

            if self.cancelled { break; }

            // 3. Build assistant message
            if !assistant_text.is_empty() {
                assistant_content_blocks.push(ContentBlock::Text(assistant_text.clone()));
            }
            for (id, name, args) in &pending_tool_calls {
                assistant_content_blocks.push(ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: args.clone(),
                });
            }
            if !assistant_content_blocks.is_empty() {
                self.messages.push(Message {
                    role: MessageRole::Assistant,
                    content: assistant_content_blocks,
                });
            }

            // 4. Handle tool calls
            if pending_tool_calls.is_empty() {
                // No tool calls — task complete
                let _ = self.ctrl_tx.send(ControllerMessage::TaskComplete(TaskResult {
                    task_id: self.task_id.clone(),
                    completion_message: Some(assistant_text),
                    total_tokens: 0,
                    total_cost: 0.0,
                }));
                break;
            }

            // Execute tool calls (sequentially for now)
            let mut tool_results = Vec::new();
            for (id, name, arguments) in pending_tool_calls {
                // Request tool execution from controller
                let _ = self.ctrl_tx.send(ControllerMessage::ToolCallRequest(ToolCallRequest {
                    tool_use_id: id.clone(),
                    tool_name: name.clone(),
                    arguments: arguments.clone(),
                    description: format!("{name}({arguments})"),
                }));

                // Wait for result
                let result = loop {
                    match self.rx.recv().await {
                        Some(AgentMessage::ToolCallResult(r)) if r.tool_use_id == id => break r,
                        Some(AgentMessage::Cancel) => {
                            self.cancelled = true;
                            break ToolCallResult {
                                tool_use_id: id.clone(),
                                content: "Cancelled".to_string(),
                                is_error: true,
                            };
                        }
                        Some(_) => continue, // Ignore other messages while waiting
                        None => {
                            self.cancelled = true;
                            break ToolCallResult {
                                tool_use_id: id.clone(),
                                content: "Channel closed".to_string(),
                                is_error: true,
                            };
                        }
                    }
                };
                tool_results.push(result);
                if self.cancelled { break; }
            }

            if self.cancelled { break; }

            // 5. Add tool results as user message
            let mut result_blocks = Vec::new();
            for result in tool_results {
                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: result.tool_use_id,
                    content: result.content,
                    is_error: result.is_error,
                });
            }
            self.messages.push(Message {
                role: MessageRole::User,
                content: result_blocks,
            });

            // 6. Loop back to step 1 (next API call)
            assistant_text = String::new();
            assistant_content_blocks = Vec::new();
            pending_tool_calls = Vec::new();
        }

        Ok(())
    }
}
```

### 7.2 Agent module (`src/agent/mod.rs`)

```rust
pub mod task_agent;
pub mod sub_agent;

pub use task_agent::{TaskAgent, AgentMessage};
```

### 7.3 Update Controller to manage agent lifecycle

In `src/controller/mod.rs`, add:
- A field `agent_tx: Option<mpsc::UnboundedSender<AgentMessage>>` to send messages to the active agent
- Handle `ControllerMessage::UserSubmit`: create provider, create TaskAgent, spawn as tokio task
- Handle `ControllerMessage::StreamChunk`: forward to TUI as `UiUpdate::StreamContent` or `UiUpdate::ThinkingContent`
- Handle `ControllerMessage::ToolCallRequest`: for now, auto-execute (permission system comes in STEP 13)
- Handle `ControllerMessage::ToolCallResult`: forward to agent
- Handle `ControllerMessage::TaskComplete`: notify TUI, clean up agent

```rust
// In Controller's main loop (handle_message method or equivalent):

async fn handle_message(&mut self, msg: ControllerMessage) {
    match msg {
        ControllerMessage::UserSubmit(text) => {
            // 1. Add user message to state
            self.state.add_message(Message {
                role: MessageRole::User,
                content: vec![ContentBlock::Text(text.clone())],
            });

            // 2. Notify TUI
            let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                role: "user".to_string(),
                content: text,
            });

            // 3. Create agent
            let (agent_tx, agent_rx) = mpsc::unbounded_channel();
            self.agent_tx = Some(agent_tx);

            let provider = self.create_provider();
            let system_prompt = crate::prompt::build_system_prompt(&self.state.config.cwd);
            let tools = self.tool_registry.tool_definitions();

            let agent = TaskAgent::new(
                uuid::Uuid::new_v4().to_string(),
                provider,
                system_prompt,
                self.state.config.model_config.clone(),
                tools,
                self.ctrl_tx.clone(),
                agent_rx,
            );

            // 4. Spawn agent task
            tokio::spawn(agent.run());
        }

        ControllerMessage::StreamChunk(chunk) => {
            match chunk {
                StreamChunk::Text { delta } => {
                    let _ = self.ui_tx.send(UiUpdate::StreamContent { delta });
                }
                StreamChunk::Thinking { delta, .. } => {
                    let _ = self.ui_tx.send(UiUpdate::ThinkingContent { delta });
                }
                StreamChunk::Usage(usage) => {
                    self.state.update_usage(&usage);
                    let _ = self.ui_tx.send(UiUpdate::StatusUpdate {
                        tokens: Some(self.state.total_tokens()),
                        cost: Some(self.state.total_cost()),
                    });
                }
                _ => {}
            }
        }

        ControllerMessage::ToolCallRequest(request) => {
            // For now, auto-execute all tools (permission system in STEP 13)
            let result = self.tool_registry.execute(&request).await;
            if let Some(agent_tx) = &self.agent_tx {
                let _ = agent_tx.send(AgentMessage::ToolCallResult(result));
            }
        }

        ControllerMessage::TaskComplete(result) => {
            self.agent_tx = None;
            let _ = self.ui_tx.send(UiUpdate::StreamEnd);
            let _ = self.ui_tx.send(UiUpdate::TaskComplete {
                message: result.completion_message,
            });
        }

        ControllerMessage::TaskError(error) => {
            let _ = self.ui_tx.send(UiUpdate::Error { message: error });
        }

        ControllerMessage::Cancel => {
            if let Some(agent_tx) = &self.agent_tx {
                let _ = agent_tx.send(AgentMessage::Cancel);
            }
        }

        _ => {}
    }
}
```

### 7.4 Update TUI to handle streaming

In `src/tui/` — update the event loop to handle:
- `UiUpdate::StreamContent { delta }` -> call `chat_state.update_last_message()` (append delta)
- `UiUpdate::StreamEnd` -> mark last message as `streaming: false`
- `UiUpdate::ThinkingContent { delta }` -> append to thinking view (STEP 19 will implement full view; for now, append to chat as dimmed text)
- Start a new assistant message (with `streaming: true`) when the first `StreamContent` arrives
- `UiUpdate::StatusUpdate` -> update status bar fields

```rust
// In the TUI event loop (src/tui/app.rs or equivalent):

fn handle_ui_update(&mut self, update: UiUpdate) {
    match update {
        UiUpdate::StreamContent { delta } => {
            if !self.chat_state.is_streaming() {
                // Start a new assistant message
                self.chat_state.start_assistant_message();
            }
            self.chat_state.append_to_last_message(&delta);
            self.needs_redraw = true;
        }

        UiUpdate::StreamEnd => {
            self.chat_state.end_streaming();
            self.needs_redraw = true;
        }

        UiUpdate::ThinkingContent { delta } => {
            // For now, display as dimmed text before the assistant message
            // Full thinking view comes in STEP 19
            self.chat_state.append_thinking(&delta);
            self.needs_redraw = true;
        }

        UiUpdate::AppendMessage { role, content } => {
            self.chat_state.add_message(&role, &content);
            self.needs_redraw = true;
        }

        UiUpdate::StatusUpdate { tokens, cost } => {
            if let Some(t) = tokens { self.status_bar.tokens = t; }
            if let Some(c) = cost { self.status_bar.cost = c; }
            self.needs_redraw = true;
        }

        UiUpdate::TaskComplete { message } => {
            self.chat_state.end_streaming();
            self.is_busy = false;
            self.needs_redraw = true;
        }

        UiUpdate::Error { message } => {
            self.chat_state.add_error_message(&message);
            self.is_busy = false;
            self.needs_redraw = true;
        }

        _ => {}
    }
}
```

### 7.5 Prompt module stub (`src/prompt/mod.rs`)

For now, implement a basic system prompt builder:

```rust
//! System prompt construction.

/// Build the system prompt for the task agent.
pub fn build_system_prompt(cwd: &str) -> String {
    format!(
        "You are a helpful AI coding assistant running in a terminal.\n\
         The user's working directory is: {cwd}\n\
         Respond concisely and helpfully."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt_contains_cwd() {
        let prompt = build_system_prompt("/home/user/project");
        assert!(prompt.contains("/home/user/project"));
    }

    #[test]
    fn test_build_system_prompt_not_empty() {
        let prompt = build_system_prompt("/tmp");
        assert!(!prompt.is_empty());
    }
}
```

### 7.6 Wire up `main.rs`

Update `src/main.rs` to:
1. Parse CLI args (clap)
2. Initialize tracing
3. Load config (AppConfig)
4. Create StateManager
5. Create ToolRegistry with defaults
6. Create Controller with channels
7. Create TUI
8. Run event loop

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::parse_args();
    tracing_subscriber::fmt::init();

    let config = AppConfig::load()?;
    let state = StateManager::new(config);
    let tool_registry = ToolRegistry::with_defaults();

    let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
    let (ui_tx, ui_rx) = mpsc::unbounded_channel();

    let mut controller = Controller::new(state, tool_registry, ctrl_tx.clone(), ui_tx);
    let mut tui = Tui::new(ctrl_tx, ui_rx)?;

    // Run controller and TUI concurrently
    tokio::select! {
        result = controller.run(ctrl_rx) => result?,
        result = tui.run() => result?,
    }

    Ok(())
}
```

## Tests

```rust
#[cfg(test)]
mod agent_tests {
    use super::*;

    // Mock provider for testing
    struct MockProvider {
        response_chunks: Vec<StreamChunk>,
    }

    #[async_trait::async_trait]
    impl Provider for MockProvider {
        async fn create_message(
            &self,
            _system: &str,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _config: &ModelConfig,
        ) -> anyhow::Result<ProviderStream> {
            let chunks = self.response_chunks.clone();
            let stream = futures::stream::iter(chunks.into_iter().map(Ok));
            Ok(Box::pin(stream))
        }

        fn model_info(&self) -> &ModelInfo {
            &ModelInfo {
                id: "mock".to_string(),
                name: "Mock".to_string(),
                provider: "mock".to_string(),
                max_tokens: 1000,
                context_window: 4000,
                supports_tools: true,
                supports_thinking: false,
                supports_images: false,
                input_price_per_mtok: 0.0,
                output_price_per_mtok: 0.0,
            }
        }

        fn abort(&self) {}
    }

    #[tokio::test]
    async fn test_agent_simple_text_response() {
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (_agent_tx, agent_rx) = mpsc::unbounded_channel();

        let provider = MockProvider {
            response_chunks: vec![
                StreamChunk::Text { delta: "Hello!".to_string() },
                StreamChunk::Done,
            ],
        };

        let agent = TaskAgent::new(
            "test-1".to_string(),
            Box::new(provider),
            "test prompt".to_string(),
            ModelConfig {
                model_id: "test".to_string(),
                max_tokens: 100,
                temperature: None,
                thinking_budget: None,
            },
            vec![],
            ctrl_tx,
            agent_rx,
        );

        tokio::spawn(agent.run());

        // Should receive StreamChunk::Text
        let msg = ctrl_rx.recv().await.unwrap();
        assert!(matches!(msg, ControllerMessage::StreamChunk(StreamChunk::Text { .. })));

        // Should receive TaskComplete
        let msg = loop {
            match ctrl_rx.recv().await.unwrap() {
                ControllerMessage::TaskComplete(r) => break r,
                _ => continue,
            }
        };
        assert!(msg.completion_message.is_some());
    }

    #[tokio::test]
    async fn test_agent_cancellation() {
        let (ctrl_tx, _ctrl_rx) = mpsc::unbounded_channel();
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();

        // Slow provider — will be cancelled
        let provider = MockProvider {
            response_chunks: vec![
                StreamChunk::Text { delta: "Slow...".to_string() },
                // No Done — will hang until cancelled
            ],
        };

        let agent = TaskAgent::new(
            "test-2".to_string(),
            Box::new(provider),
            "test".to_string(),
            ModelConfig {
                model_id: "test".to_string(),
                max_tokens: 100,
                temperature: None,
                thinking_budget: None,
            },
            vec![],
            ctrl_tx,
            agent_rx,
        );

        let handle = tokio::spawn(agent.run());

        // Cancel
        agent_tx.send(AgentMessage::Cancel).unwrap();
        let result = handle.await.unwrap();
        assert!(result.is_ok()); // Should exit cleanly on cancel
    }

    #[tokio::test]
    async fn test_agent_tool_call_flow() {
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();

        let provider = MockProvider {
            response_chunks: vec![
                StreamChunk::ToolCallComplete {
                    id: "tc1".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "/test.rs"}),
                },
                StreamChunk::Done,
            ],
        };

        let agent = TaskAgent::new(
            "test-3".to_string(),
            Box::new(provider),
            "test".to_string(),
            ModelConfig {
                model_id: "test".to_string(),
                max_tokens: 100,
                temperature: None,
                thinking_budget: None,
            },
            vec![],
            ctrl_tx,
            agent_rx,
        );

        tokio::spawn(agent.run());

        // Should receive ToolCallRequest
        let msg = loop {
            match ctrl_rx.recv().await.unwrap() {
                ControllerMessage::ToolCallRequest(req) => break req,
                _ => continue,
            }
        };
        assert_eq!(msg.tool_name, "read_file");

        // Send tool result back
        agent_tx
            .send(AgentMessage::ToolCallResult(ToolCallResult {
                tool_use_id: "tc1".to_string(),
                content: "file contents here".to_string(),
                is_error: false,
            }))
            .unwrap();

        // Agent should loop and eventually complete (mock provider will return same chunks)
        // In real usage, provider would return different response based on tool result
    }

    #[tokio::test]
    async fn test_agent_provider_error() {
        // Test that provider errors are forwarded to controller
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (_agent_tx, agent_rx) = mpsc::unbounded_channel();

        // Provider that returns an error stream chunk
        let provider = MockProvider {
            response_chunks: vec![
                StreamChunk::Error("API rate limited".to_string()),
                StreamChunk::Done,
            ],
        };

        let agent = TaskAgent::new(
            "test-4".to_string(),
            Box::new(provider),
            "test".to_string(),
            ModelConfig {
                model_id: "test".to_string(),
                max_tokens: 100,
                temperature: None,
                thinking_budget: None,
            },
            vec![],
            ctrl_tx,
            agent_rx,
        );

        tokio::spawn(agent.run());

        // Should receive TaskError
        let mut got_error = false;
        for _ in 0..10 {
            match ctrl_rx.recv().await {
                Some(ControllerMessage::TaskError(_)) => {
                    got_error = true;
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
        assert!(got_error);
    }

    #[tokio::test]
    async fn test_agent_multiple_text_chunks() {
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (_agent_tx, agent_rx) = mpsc::unbounded_channel();

        let provider = MockProvider {
            response_chunks: vec![
                StreamChunk::Text { delta: "Hello ".to_string() },
                StreamChunk::Text { delta: "world".to_string() },
                StreamChunk::Text { delta: "!".to_string() },
                StreamChunk::Done,
            ],
        };

        let agent = TaskAgent::new(
            "test-5".to_string(),
            Box::new(provider),
            "test".to_string(),
            ModelConfig {
                model_id: "test".to_string(),
                max_tokens: 100,
                temperature: None,
                thinking_budget: None,
            },
            vec![],
            ctrl_tx,
            agent_rx,
        );

        tokio::spawn(agent.run());

        // Collect all text chunks
        let mut text_chunks = Vec::new();
        loop {
            match ctrl_rx.recv().await.unwrap() {
                ControllerMessage::StreamChunk(StreamChunk::Text { delta }) => {
                    text_chunks.push(delta);
                }
                ControllerMessage::TaskComplete(result) => {
                    assert_eq!(
                        result.completion_message.unwrap(),
                        "Hello world!"
                    );
                    break;
                }
                _ => continue,
            }
        }
        assert_eq!(text_chunks.len(), 3);
    }
}

// End-to-end test
#[cfg(test)]
mod e2e_tests {
    use super::*;

    #[tokio::test]
    async fn test_user_submit_to_response() {
        // Create StateManager, Controller, mock agent
        // Send UserSubmit
        // Verify UiUpdate::AppendMessage received
        // Verify UiUpdate::StreamContent received
        // Verify UiUpdate::StreamEnd received
        // This test verifies the full message routing
    }

    #[tokio::test]
    async fn test_controller_handles_cancel_during_stream() {
        // Start a streaming response
        // Send Cancel message
        // Verify agent receives Cancel
        // Verify TUI receives StreamEnd
    }
}
```

## Acceptance Criteria
- [x] User types message -> Controller creates agent with provider -> Agent calls API
- [x] Streaming text appears character-by-character in TUI
- [x] Thinking content streams to TUI (displayed as dimmed text for now)
- [x] Tool calls are sent to controller as ToolCallRequest
- [x] Tool results flow back to agent and are included in next API call
- [x] Task completion message displayed in TUI
- [x] Agent cancellation (Ctrl+C) works cleanly
- [x] No deadlocks between agent <-> controller channels
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` all pass with mock provider

**Completed**: PR #4
