//! Central orchestrator — routes messages between TUI, agents, and tools.
//!
//! ```text
//!   TUI ──► ctrl_tx ──► Controller ──► ui_tx ──► TUI
//!                            │
//!                            ├── spawns TaskAgent (tokio task)
//!                            │     └── agent_tx / agent_rx channels
//!                            │
//!                            ├── forwards StreamChunk → UiUpdate
//!                            ├── forwards ToolCallRequest → agent result
//!                            └── handles TaskComplete / TaskError
//! ```

pub mod messages;
pub mod task;

use crate::agent::{AgentMessage, TaskAgent};
use crate::permission::PermissionMode;
use crate::provider::{self, ModelConfig, StreamChunk};
use crate::state::StateManager;
use messages::{ControllerMessage, ToolCallResult, UiUpdate};
use tokio::sync::mpsc;

/// The Controller is the central message router.
///
/// All components send messages to the controller via clones of the
/// `UnboundedSender<ControllerMessage>` returned by [`Controller::new`].
/// The controller sends UI updates via `ui_tx`.
pub struct Controller {
    /// Receives messages from all components.
    rx: mpsc::UnboundedReceiver<ControllerMessage>,
    /// Sends updates to the TUI.
    ui_tx: mpsc::UnboundedSender<UiUpdate>,
    /// Clone of the controller's own sender (given to agents).
    ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
    /// Sender to the active agent (if any).
    agent_tx: Option<mpsc::UnboundedSender<AgentMessage>>,
    /// Application state.
    state: StateManager,
    /// Current permission mode (tracked for YOLO toggle).
    permission_mode: PermissionMode,
    /// Whether the controller is running.
    running: bool,
}

impl Controller {
    /// Creates a new Controller and returns `(controller, ctrl_tx, ui_rx)`.
    pub fn new(
        state: StateManager,
        permission_mode: PermissionMode,
    ) -> (
        Self,
        mpsc::UnboundedSender<ControllerMessage>,
        mpsc::UnboundedReceiver<UiUpdate>,
    ) {
        let (ctrl_tx, rx) = mpsc::unbounded_channel();
        let (ui_tx, ui_rx) = mpsc::unbounded_channel();
        let ctrl = Self {
            rx,
            ui_tx,
            ctrl_tx: ctrl_tx.clone(),
            agent_tx: None,
            state,
            permission_mode,
            running: true,
        };
        (ctrl, ctrl_tx, ui_rx)
    }

    /// Main message loop — runs as a tokio task.
    pub async fn run(mut self) -> anyhow::Result<()> {
        tracing::info!("Controller started");
        while self.running {
            if let Some(msg) = self.rx.recv().await {
                self.handle_message(msg).await;
            } else {
                tracing::info!("All senders dropped, controller shutting down");
                break;
            }
        }
        tracing::info!("Controller stopped");
        Ok(())
    }

    /// Dispatches a single message to the appropriate handler.
    #[allow(clippy::too_many_lines)]
    async fn handle_message(&mut self, msg: ControllerMessage) {
        match msg {
            ControllerMessage::UserSubmit { text, .. } => {
                self.handle_user_submit(text).await;
            }
            ControllerMessage::Quit => {
                tracing::info!("Quit requested");
                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(AgentMessage::Cancel);
                }
                let _ = self.ui_tx.send(UiUpdate::Quit);
                self.running = false;
            }
            ControllerMessage::CancelTask => {
                tracing::info!("Task cancellation requested");
                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(AgentMessage::Cancel);
                }
            }
            ControllerMessage::ToggleThinking => {
                tracing::info!("Toggle thinking visibility");
            }
            ControllerMessage::ToggleYolo => {
                let new_mode = if self.permission_mode == PermissionMode::Yolo {
                    PermissionMode::Ask
                } else {
                    PermissionMode::Yolo
                };
                self.permission_mode = new_mode;
                let is_yolo = new_mode == PermissionMode::Yolo;
                tracing::info!(?new_mode, "Permission mode toggled");
                let _ = self.ui_tx.send(UiUpdate::StatusUpdate {
                    mode: None,
                    tokens: None,
                    cost: None,
                    is_streaming: None,
                    is_yolo: Some(is_yolo),
                    context_tokens: None,
                    context_window: None,
                });
            }
            ControllerMessage::SwitchMode(mode) => {
                tracing::info!(?mode, "Mode switch requested");
                let mode_str = match mode {
                    crate::state::task_state::Mode::Plan => "PLAN",
                    crate::state::task_state::Mode::Act => "ACT",
                };
                let _ = self.ui_tx.send(UiUpdate::StatusUpdate {
                    mode: Some(mode_str.to_string()),
                    tokens: None,
                    cost: None,
                    is_streaming: None,
                    is_yolo: None,
                    context_tokens: None,
                    context_window: None,
                });
            }
            ControllerMessage::ApprovalResponse {
                tool_use_id,
                approved,
                ..
            } => {
                tracing::info!(tool_use_id, approved, "Approval response received");
            }
            ControllerMessage::StreamChunk(chunk) => {
                self.handle_stream_chunk(chunk);
            }
            ControllerMessage::ToolCallRequest(req) => {
                self.handle_tool_call_request(req);
            }
            ControllerMessage::ToolCallResult(_) => {}
            ControllerMessage::TaskComplete(result) => {
                tracing::info!(task_id = result.task_id, "Task completed");
                self.agent_tx = None;
                let _ = self.ui_tx.send(UiUpdate::StreamEnd);
                if let Some(msg) = result.completion_message {
                    let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                        role: crate::tui::chat_view::ChatRole::System,
                        content: msg,
                    });
                }
                let _ = self.ui_tx.send(UiUpdate::StatusUpdate {
                    mode: None,
                    tokens: Some(result.total_tokens),
                    cost: Some(result.total_cost),
                    is_streaming: Some(false),
                    is_yolo: None,
                    context_tokens: None,
                    context_window: None,
                });
            }
            ControllerMessage::TaskError(error) => {
                tracing::error!(%error, "Task error");
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: format!("Error: {error}"),
                });
                let _ = self.ui_tx.send(UiUpdate::StatusUpdate {
                    mode: None,
                    tokens: None,
                    cost: None,
                    is_streaming: Some(false),
                    is_yolo: None,
                    context_tokens: None,
                    context_window: None,
                });
            }
        }
    }

    /// Handles a user message: creates a provider and spawns a `TaskAgent`.
    async fn handle_user_submit(&mut self, text: String) {
        tracing::info!(len = text.len(), "User submitted message");

        let _ = self.ui_tx.send(UiUpdate::StatusUpdate {
            mode: None,
            tokens: None,
            cost: None,
            is_streaming: Some(true),
            is_yolo: None,
            context_tokens: None,
            context_window: None,
        });

        let api_key = self.state.resolve_api_key("anthropic").await;
        let Some(api_key) = api_key else {
            let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                role: crate::tui::chat_view::ChatRole::System,
                content: "No API key configured. Set ANTHROPIC_API_KEY environment variable."
                    .to_string(),
            });
            return;
        };

        let config = self.state.config().await;
        let provider = match provider::create_provider("anthropic", &api_key, None) {
            Ok(p) => p,
            Err(e) => {
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: format!("Failed to create provider: {e}"),
                });
                return;
            }
        };

        let (agent_tx, agent_rx) = mpsc::unbounded_channel();
        self.agent_tx = Some(agent_tx);

        let mode = crate::prompt::resolve_default_mode(&config.mode.default);
        let system_prompt = crate::prompt::build_system_prompt(".", mode);
        let model_config = ModelConfig {
            model_id: config
                .provider
                .anthropic
                .model
                .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string()),
            max_tokens: 8192,
            temperature: None,
            thinking_budget: None,
        };

        let mut agent = TaskAgent::new(
            uuid::Uuid::new_v4().to_string(),
            provider,
            system_prompt,
            model_config,
            vec![],
            self.ctrl_tx.clone(),
            agent_rx,
        );
        agent.add_user_message(text);

        tokio::spawn(agent.run());
    }

    /// Forwards stream chunks to the TUI as appropriate `UiUpdate`s.
    fn handle_stream_chunk(&self, chunk: StreamChunk) {
        match chunk {
            StreamChunk::Text { delta } => {
                let _ = self.ui_tx.send(UiUpdate::StreamContent { delta });
            }
            StreamChunk::Thinking { delta, .. } => {
                if !delta.is_empty() {
                    let _ = self.ui_tx.send(UiUpdate::ThinkingContent { delta });
                }
            }
            StreamChunk::Usage(usage) => {
                let _ = self.ui_tx.send(UiUpdate::StatusUpdate {
                    mode: None,
                    tokens: Some(usage.input_tokens + usage.output_tokens),
                    cost: usage.total_cost,
                    is_streaming: None,
                    is_yolo: None,
                    context_tokens: Some(usage.input_tokens),
                    context_window: None,
                });
            }
            _ => {}
        }
    }

    /// Handles a tool call request — auto-executes for now (permission system in STEP 13).
    fn handle_tool_call_request(&self, req: messages::ToolCallRequest) {
        tracing::info!(
            tool = req.tool_name,
            "Tool call requested (auto-responding)"
        );
        if let Some(tx) = &self.agent_tx {
            let _ = tx.send(AgentMessage::ToolCallResult(ToolCallResult {
                tool_use_id: req.tool_use_id,
                content: format!("Tool '{}' not yet implemented", req.tool_name),
                is_error: true,
            }));
        }
    }
}

#[cfg(test)]
mod controller_tests {
    use super::Controller;
    use super::messages::{ControllerMessage, TaskResult, UiUpdate};
    use crate::permission::PermissionMode;
    use crate::state::StateManager;
    use crate::tui::chat_view::ChatRole;

    async fn make_controller() -> (
        Controller,
        tokio::sync::mpsc::UnboundedSender<ControllerMessage>,
        tokio::sync::mpsc::UnboundedReceiver<UiUpdate>,
    ) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let state = StateManager::new(Some(path)).await.unwrap();
        Controller::new(state, PermissionMode::Ask)
    }

    #[tokio::test]
    async fn controller_shuts_down_on_quit() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx.send(ControllerMessage::Quit).unwrap();

        let update = ui_rx.recv().await.unwrap();
        assert!(matches!(update, UiUpdate::Quit));

        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn controller_mode_switch() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::SwitchMode(
                crate::state::task_state::Mode::Plan,
            ))
            .unwrap();

        let update = ui_rx.recv().await.unwrap();
        match update {
            UiUpdate::StatusUpdate { mode, .. } => {
                assert_eq!(mode, Some("PLAN".to_string()));
            }
            other => panic!("Expected StatusUpdate, got {other:?}"),
        }

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = ui_rx.recv().await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn controller_task_complete() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::TaskComplete(TaskResult {
                task_id: "task-1".to_string(),
                completion_message: Some("Done!".to_string()),
                total_tokens: 500,
                total_cost: 0.01,
            }))
            .unwrap();

        let update1 = ui_rx.recv().await.unwrap();
        assert!(matches!(update1, UiUpdate::StreamEnd));

        let update2 = ui_rx.recv().await.unwrap();
        match update2 {
            UiUpdate::AppendMessage { content, role } => {
                assert_eq!(content, "Done!");
                assert_eq!(role, ChatRole::System);
            }
            other => panic!("Expected AppendMessage, got {other:?}"),
        }

        let update3 = ui_rx.recv().await.unwrap();
        match update3 {
            UiUpdate::StatusUpdate {
                tokens,
                cost,
                is_streaming,
                ..
            } => {
                assert_eq!(tokens, Some(500));
                assert_eq!(cost, Some(0.01));
                assert_eq!(is_streaming, Some(false));
            }
            other => panic!("Expected StatusUpdate, got {other:?}"),
        }

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = ui_rx.recv().await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn controller_task_error() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::TaskError("something broke".to_string()))
            .unwrap();

        let update = ui_rx.recv().await.unwrap();
        match update {
            UiUpdate::AppendMessage { content, role } => {
                assert!(content.contains("something broke"));
                assert_eq!(role, ChatRole::System);
            }
            other => panic!("Expected AppendMessage, got {other:?}"),
        }

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = ui_rx.recv().await;
        let _ = ui_rx.recv().await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn controller_stream_chunk_text() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::StreamChunk(
                crate::provider::StreamChunk::Text {
                    delta: "hello".to_string(),
                },
            ))
            .unwrap();

        let update = ui_rx.recv().await.unwrap();
        match update {
            UiUpdate::StreamContent { delta } => {
                assert_eq!(delta, "hello");
            }
            other => panic!("Expected StreamContent, got {other:?}"),
        }

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = ui_rx.recv().await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn controller_stream_chunk_thinking() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx
            .send(ControllerMessage::StreamChunk(
                crate::provider::StreamChunk::Thinking {
                    delta: "reasoning...".to_string(),
                    signature: None,
                    redacted: false,
                },
            ))
            .unwrap();

        let update = ui_rx.recv().await.unwrap();
        match update {
            UiUpdate::ThinkingContent { delta } => {
                assert_eq!(delta, "reasoning...");
            }
            other => panic!("Expected ThinkingContent, got {other:?}"),
        }

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = ui_rx.recv().await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn controller_toggle_yolo() {
        let (controller, ctrl_tx, mut ui_rx) = make_controller().await;
        let handle = tokio::spawn(controller.run());

        ctrl_tx.send(ControllerMessage::ToggleYolo).unwrap();

        let update = ui_rx.recv().await.unwrap();
        match update {
            UiUpdate::StatusUpdate { is_yolo, .. } => {
                assert_eq!(is_yolo, Some(true));
            }
            other => panic!("Expected StatusUpdate, got {other:?}"),
        }

        ctrl_tx.send(ControllerMessage::ToggleYolo).unwrap();

        let update = ui_rx.recv().await.unwrap();
        match update {
            UiUpdate::StatusUpdate { is_yolo, .. } => {
                assert_eq!(is_yolo, Some(false));
            }
            other => panic!("Expected StatusUpdate, got {other:?}"),
        }

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = ui_rx.recv().await;
        handle.await.unwrap().unwrap();
    }
}
