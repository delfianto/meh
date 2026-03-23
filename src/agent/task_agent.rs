//! Main task agent — runs the conversation loop with an LLM provider.
//!
//! The agent calls the provider, streams the response through the
//! `StreamProcessor`, forwards events to the controller, and loops
//! on tool calls until the task completes or is cancelled.

use crate::controller::messages::{ControllerMessage, TaskResult, ToolCallRequest, ToolCallResult};
use crate::provider::{
    ContentBlock, Message, MessageRole, ModelConfig, Provider, StreamChunk, ToolDefinition,
};
use crate::streaming::{ProcessedEvent, StreamProcessor};
use futures::StreamExt;
use tokio::sync::mpsc;

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

/// Runs a single task's conversation loop as a tokio task.
pub struct TaskAgent {
    task_id: String,
    provider: Box<dyn Provider>,
    system_prompt: String,
    messages: Vec<Message>,
    config: ModelConfig,
    tools: Vec<ToolDefinition>,
    ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
    rx: mpsc::UnboundedReceiver<AgentMessage>,
    stream_processor: StreamProcessor,
    cancelled: bool,
}

impl TaskAgent {
    /// Creates a new task agent.
    pub fn new(
        task_id: String,
        provider: Box<dyn Provider>,
        system_prompt: String,
        config: ModelConfig,
        tools: Vec<ToolDefinition>,
        ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
        rx: mpsc::UnboundedReceiver<AgentMessage>,
    ) -> Self {
        Self {
            task_id,
            provider,
            system_prompt,
            messages: Vec::new(),
            config,
            tools,
            ctrl_tx,
            rx,
            stream_processor: StreamProcessor::new(),
            cancelled: false,
        }
    }

    /// Adds the initial user message to the conversation.
    pub fn add_user_message(&mut self, text: String) {
        self.messages.push(Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text(text)],
        });
    }

    /// Creates a task agent with pre-loaded conversation history (for resume).
    #[allow(clippy::too_many_arguments)]
    pub fn with_history(
        task_id: String,
        messages: Vec<Message>,
        provider: Box<dyn Provider>,
        system_prompt: String,
        config: ModelConfig,
        tools: Vec<ToolDefinition>,
        ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
        rx: mpsc::UnboundedReceiver<AgentMessage>,
    ) -> Self {
        Self {
            task_id,
            provider,
            system_prompt,
            messages,
            config,
            tools,
            ctrl_tx,
            rx,
            stream_processor: StreamProcessor::new(),
            cancelled: false,
        }
    }

    /// Alias for [`add_user_message`](Self::add_user_message) used by sub-agents.
    pub fn add_initial_message(&mut self, text: String) {
        self.add_user_message(text);
    }

    /// Main execution loop. Returns when the task completes or is cancelled.
    #[allow(clippy::too_many_lines)]
    pub async fn run(mut self) -> anyhow::Result<()> {
        loop {
            self.stream_processor.reset();

            let mut stream = match self
                .provider
                .create_message(
                    &self.system_prompt,
                    &self.messages,
                    &self.tools,
                    &self.config,
                )
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    let _ = self
                        .ctrl_tx
                        .send(ControllerMessage::TaskError(e.to_string()));
                    return Err(e);
                }
            };

            let mut assistant_text = String::with_capacity(4096);
            let mut assistant_content_blocks: Vec<ContentBlock> = Vec::new();
            let mut pending_tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();

            let stream_done = self
                .process_stream(&mut stream, &mut assistant_text, &mut pending_tool_calls)
                .await;

            if self.cancelled || !stream_done {
                break;
            }

            self.build_assistant_message(
                &assistant_text,
                &pending_tool_calls,
                &mut assistant_content_blocks,
            );

            if pending_tool_calls.is_empty() {
                let _ = self
                    .ctrl_tx
                    .send(ControllerMessage::TaskComplete(TaskResult {
                        task_id: self.task_id.clone(),
                        completion_message: Some(assistant_text),
                        total_tokens: 0,
                        total_cost: 0.0,
                    }));
                break;
            }

            if !self.execute_tool_calls(&pending_tool_calls).await {
                break;
            }
        }

        Ok(())
    }

    /// Processes the SSE stream, forwarding events to the controller.
    /// Returns `true` if the stream completed normally, `false` on error/cancel.
    async fn process_stream(
        &mut self,
        stream: &mut crate::provider::ProviderStream,
        assistant_text: &mut String,
        pending_tool_calls: &mut Vec<(String, String, serde_json::Value)>,
    ) -> bool {
        loop {
            tokio::select! {
                Some(agent_msg) = self.rx.recv() => {
                    if matches!(agent_msg, AgentMessage::Cancel) {
                        self.provider.abort();
                        self.cancelled = true;
                        return false;
                    }
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(stream_chunk)) => {
                            let done = self.dispatch_events(
                                stream_chunk,
                                assistant_text,
                                pending_tool_calls,
                            );
                            if done { return true; }
                        }
                        Some(Err(e)) => {
                            let _ = self.ctrl_tx.send(ControllerMessage::TaskError(e.to_string()));
                            return false;
                        }
                        None => return true,
                    }
                }
            }

            if self.cancelled {
                return false;
            }
        }
    }

    /// Dispatches processed events to the controller. Returns `true` if stream is done.
    fn dispatch_events(
        &mut self,
        stream_chunk: StreamChunk,
        assistant_text: &mut String,
        pending_tool_calls: &mut Vec<(String, String, serde_json::Value)>,
    ) -> bool {
        let events = self.stream_processor.process(stream_chunk);
        let mut done = false;
        for event in events {
            match event {
                ProcessedEvent::TextBatch(text) => {
                    assistant_text.push_str(&text);
                    let _ = self
                        .ctrl_tx
                        .send(ControllerMessage::StreamChunk(StreamChunk::Text {
                            delta: text,
                        }));
                }
                ProcessedEvent::ThinkingDelta(delta) => {
                    let _ =
                        self.ctrl_tx
                            .send(ControllerMessage::StreamChunk(StreamChunk::Thinking {
                                delta,
                                signature: None,
                                redacted: false,
                            }));
                }
                ProcessedEvent::ThinkingComplete(block) => {
                    assistant_text.push_str(&block.content);
                }
                ProcessedEvent::ToolCallReady {
                    id,
                    name,
                    arguments,
                } => {
                    pending_tool_calls.push((id, name, arguments));
                }
                ProcessedEvent::Usage(usage) => {
                    let _ = self
                        .ctrl_tx
                        .send(ControllerMessage::StreamChunk(StreamChunk::Usage(usage)));
                }
                ProcessedEvent::Done => {
                    done = true;
                }
                ProcessedEvent::Error(e) => {
                    let _ = self.ctrl_tx.send(ControllerMessage::TaskError(e));
                }
                ProcessedEvent::ToolCallPreview { .. } => {}
            }
        }
        done
    }

    /// Builds the assistant message from accumulated text and tool calls.
    fn build_assistant_message(
        &mut self,
        assistant_text: &str,
        pending_tool_calls: &[(String, String, serde_json::Value)],
        content_blocks: &mut Vec<ContentBlock>,
    ) {
        if !assistant_text.is_empty() {
            content_blocks.push(ContentBlock::Text(assistant_text.to_string()));
        }
        for (id, name, args) in pending_tool_calls {
            content_blocks.push(ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: args.clone(),
            });
        }
        if !content_blocks.is_empty() {
            self.messages.push(Message {
                role: MessageRole::Assistant,
                content: std::mem::take(content_blocks),
            });
        }
    }

    /// Sends tool call requests to the controller and waits for results.
    /// Returns `false` if cancelled during execution.
    async fn execute_tool_calls(
        &mut self,
        pending_tool_calls: &[(String, String, serde_json::Value)],
    ) -> bool {
        let mut result_blocks = Vec::new();

        for (id, name, arguments) in pending_tool_calls {
            let _ = self
                .ctrl_tx
                .send(ControllerMessage::ToolCallRequest(ToolCallRequest {
                    tool_use_id: id.clone(),
                    tool_name: name.clone(),
                    arguments: arguments.clone(),
                    description: format!("{name}({arguments})"),
                }));

            let result = self.wait_for_tool_result(id).await;
            let is_cancel = result.is_error && result.content == "Cancelled";
            result_blocks.push(result);

            if self.cancelled || is_cancel {
                return false;
            }
        }

        self.messages.push(Message {
            role: MessageRole::User,
            content: result_blocks
                .into_iter()
                .map(|r| ContentBlock::ToolResult {
                    tool_use_id: r.tool_use_id,
                    content: r.content,
                    is_error: r.is_error,
                })
                .collect(),
        });

        true
    }

    /// Waits for the controller to send back a tool result matching the given ID.
    async fn wait_for_tool_result(&mut self, tool_use_id: &str) -> ToolCallResult {
        loop {
            match self.rx.recv().await {
                Some(AgentMessage::ToolCallResult(r)) if r.tool_use_id == tool_use_id => {
                    return r;
                }
                Some(AgentMessage::Cancel) => {
                    self.cancelled = true;
                    return ToolCallResult {
                        tool_use_id: tool_use_id.to_string(),
                        content: "Cancelled".to_string(),
                        is_error: true,
                    };
                }
                Some(_) => {}
                None => {
                    self.cancelled = true;
                    return ToolCallResult {
                        tool_use_id: tool_use_id.to_string(),
                        content: "Channel closed".to_string(),
                        is_error: true,
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ModelConfig, ModelInfo, Provider, ProviderStream, StreamChunk};

    struct MockProvider {
        response_chunks: Vec<StreamChunk>,
        model_info: ModelInfo,
    }

    impl MockProvider {
        fn new(chunks: Vec<StreamChunk>) -> Self {
            Self {
                response_chunks: chunks,
                model_info: ModelInfo {
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
                    cache_read_price_per_mtok: None,
                    cache_write_price_per_mtok: None,
                    thinking_price_per_mtok: None,
                },
            }
        }
    }

    #[async_trait::async_trait]
    impl Provider for MockProvider {
        async fn create_message(
            &self,
            _system: &str,
            _messages: &[crate::provider::Message],
            _tools: &[crate::provider::ToolDefinition],
            _config: &ModelConfig,
        ) -> anyhow::Result<ProviderStream> {
            let chunks = self.response_chunks.clone();
            let stream = futures::stream::iter(chunks.into_iter().map(Ok));
            Ok(Box::pin(stream))
        }

        fn model_info(&self) -> &ModelInfo {
            &self.model_info
        }

        fn abort(&self) {}
    }

    /// Helper to create a test agent.
    fn make_agent(
        chunks: Vec<StreamChunk>,
        ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
        rx: mpsc::UnboundedReceiver<AgentMessage>,
    ) -> TaskAgent {
        let mut agent = TaskAgent::new(
            "test".to_string(),
            Box::new(MockProvider::new(chunks)),
            "test prompt".to_string(),
            ModelConfig {
                model_id: "test".to_string(),
                max_tokens: 100,
                temperature: None,
                thinking_budget: None,
            },
            vec![],
            ctrl_tx,
            rx,
        );
        agent.add_user_message("hello".to_string());
        agent
    }

    #[tokio::test]
    async fn agent_simple_text_response() {
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (_agent_tx, agent_rx) = mpsc::unbounded_channel();

        let agent = make_agent(
            vec![
                StreamChunk::Text {
                    delta: "Hello!".to_string(),
                },
                StreamChunk::Done,
            ],
            ctrl_tx,
            agent_rx,
        );

        tokio::spawn(agent.run());

        let mut got_text = false;
        let mut got_complete = false;

        while let Some(msg) = ctrl_rx.recv().await {
            match msg {
                ControllerMessage::StreamChunk(StreamChunk::Text { .. }) => got_text = true,
                ControllerMessage::TaskComplete(r) => {
                    assert!(r.completion_message.is_some());
                    got_complete = true;
                    break;
                }
                _ => {}
            }
        }

        assert!(got_text);
        assert!(got_complete);
    }

    #[tokio::test]
    async fn agent_cancellation() {
        let (ctrl_tx, _ctrl_rx) = mpsc::unbounded_channel();
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();

        let agent = make_agent(
            vec![StreamChunk::Text {
                delta: "Slow...".to_string(),
            }],
            ctrl_tx,
            agent_rx,
        );

        let handle = tokio::spawn(agent.run());

        agent_tx.send(AgentMessage::Cancel).unwrap();
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn agent_tool_call_flow() {
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();

        let agent = make_agent(
            vec![
                StreamChunk::ToolCallComplete {
                    id: "tc1".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "/test.rs"}),
                },
                StreamChunk::Done,
            ],
            ctrl_tx,
            agent_rx,
        );

        tokio::spawn(agent.run());

        let req = loop {
            match ctrl_rx.recv().await.unwrap() {
                ControllerMessage::ToolCallRequest(req) => break req,
                _ => continue,
            }
        };
        assert_eq!(req.tool_name, "read_file");

        agent_tx
            .send(AgentMessage::ToolCallResult(ToolCallResult {
                tool_use_id: "tc1".to_string(),
                content: "file contents here".to_string(),
                is_error: false,
            }))
            .unwrap();
    }

    #[tokio::test]
    async fn agent_provider_error() {
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (_agent_tx, agent_rx) = mpsc::unbounded_channel();

        let agent = make_agent(
            vec![
                StreamChunk::Error("API rate limited".to_string()),
                StreamChunk::Done,
            ],
            ctrl_tx,
            agent_rx,
        );

        tokio::spawn(agent.run());

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
    async fn agent_multiple_text_chunks() {
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (_agent_tx, agent_rx) = mpsc::unbounded_channel();

        let agent = make_agent(
            vec![
                StreamChunk::Text {
                    delta: "Hello ".to_string(),
                },
                StreamChunk::Text {
                    delta: "world".to_string(),
                },
                StreamChunk::Text {
                    delta: "!".to_string(),
                },
                StreamChunk::Done,
            ],
            ctrl_tx,
            agent_rx,
        );

        tokio::spawn(agent.run());

        let mut text_chunks = Vec::new();
        loop {
            match ctrl_rx.recv().await.unwrap() {
                ControllerMessage::StreamChunk(StreamChunk::Text { delta }) => {
                    text_chunks.push(delta);
                }
                ControllerMessage::TaskComplete(result) => {
                    assert_eq!(result.completion_message.unwrap(), "Hello world!");
                    break;
                }
                _ => continue,
            }
        }
        assert!(!text_chunks.is_empty());
    }

    #[tokio::test]
    async fn agent_with_history_resumes() {
        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (_agent_tx, agent_rx) = mpsc::unbounded_channel();

        let existing_messages = vec![
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::Text("original question".to_string())],
            },
            Message {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::Text("original answer".to_string())],
            },
            Message {
                role: MessageRole::User,
                content: vec![ContentBlock::Text("follow up".to_string())],
            },
        ];

        let agent = TaskAgent::with_history(
            "resumed-task".to_string(),
            existing_messages,
            Box::new(MockProvider::new(vec![
                StreamChunk::Text {
                    delta: "Follow up answer".to_string(),
                },
                StreamChunk::Done,
            ])),
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

        let mut got_complete = false;
        while let Some(msg) = ctrl_rx.recv().await {
            if let ControllerMessage::TaskComplete(result) = msg {
                assert_eq!(result.task_id, "resumed-task");
                assert!(
                    result
                        .completion_message
                        .as_deref()
                        .is_some_and(|m| m.contains("Follow up answer"))
                );
                got_complete = true;
                break;
            }
        }
        assert!(got_complete);
    }
}
