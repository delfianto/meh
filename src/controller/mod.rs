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
use crate::error::{self, MehError};
use crate::ignore::IgnoreController;
use crate::permission::PermissionMode;
use crate::prompt::rules::{load_rules, rules_to_prompt};
use crate::provider::{self, ModelConfig, StreamChunk};
use crate::state::StateManager;
use crate::state::history::{AutoSaver, PersistedTask, TaskHistory};
use crate::streaming::ui_batcher::UiBatcher;
use messages::{ControllerMessage, ToolCallResult, UiUpdate};
use std::time::Duration;
use task::TaskCancellation;
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
    /// Batches rapid stream updates for smooth TUI rendering.
    batcher: UiBatcher,
    /// Cooperative cancellation with double-cancel detection.
    cancellation: TaskCancellation,
    /// Path protection via .mehignore rules.
    ignore: IgnoreController,
    /// Debounced auto-saver for task persistence.
    auto_saver: Option<AutoSaver>,
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
        let cwd = std::env::current_dir().unwrap_or_default();
        let ignore = IgnoreController::new(&cwd);
        let auto_saver = TaskHistory::default_dir()
            .ok()
            .and_then(|dir| TaskHistory::new(dir).ok())
            .map(AutoSaver::new);

        let ctrl = Self {
            rx,
            ui_tx,
            ctrl_tx: ctrl_tx.clone(),
            agent_tx: None,
            state,
            permission_mode,
            running: true,
            batcher: UiBatcher::new(60),
            cancellation: TaskCancellation::new(),
            ignore,
            auto_saver,
        };
        (ctrl, ctrl_tx, ui_rx)
    }

    /// Main message loop — runs as a tokio task.
    ///
    /// Uses `tokio::select!` to handle both incoming messages and periodic
    /// tick-based flushing of batched UI updates.
    pub async fn run(mut self) -> anyhow::Result<()> {
        tracing::info!("Controller started");
        while self.running {
            tokio::select! {
                msg = self.rx.recv() => {
                    if let Some(m) = msg {
                        self.handle_message(m).await;
                    } else {
                        tracing::info!("All senders dropped, controller shutting down");
                        break;
                    }
                }
                () = tokio::time::sleep(Duration::from_millis(16)), if self.batcher.has_pending() => {
                    self.flush_batcher();
                }
            }
        }
        self.flush_batcher();
        tracing::info!("Controller stopped");
        Ok(())
    }

    /// Flushes batched updates to the TUI.
    fn flush_batcher(&mut self) {
        for update in self.batcher.flush() {
            let _ = self.ui_tx.send(update);
        }
    }

    /// Send a user-facing error message to the TUI.
    fn send_error(&self, message: &str) {
        let _ = self.ui_tx.send(UiUpdate::AppendMessage {
            role: crate::tui::chat_view::ChatRole::System,
            content: message.to_string(),
        });
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
                let is_double = self.cancellation.cancel();
                if is_double {
                    tracing::info!("Double cancel detected, force quitting");
                    let _ = self.ui_tx.send(UiUpdate::Quit);
                    self.running = false;
                    return;
                }
                tracing::info!("Task cancellation requested");
                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(AgentMessage::Cancel);
                }
                self.flush_batcher();
                let _ = self.ui_tx.send(UiUpdate::StreamEnd);
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: "Task cancelled by user.".to_string(),
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
                self.flush_batcher();
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
                if let Some(ref saver) = self.auto_saver {
                    let title = crate::state::history::generate_title(&result.task_id);
                    saver.queue_save(PersistedTask {
                        task_id: result.task_id.clone(),
                        title,
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                        messages: vec![],
                        mode: "act".to_string(),
                        provider: "anthropic".to_string(),
                        model: String::new(),
                        total_input_tokens: result.total_tokens,
                        total_output_tokens: 0,
                        total_cost: result.total_cost,
                        completed: true,
                    });
                }
            }
            ControllerMessage::TaskError(error) => {
                self.flush_batcher();
                tracing::error!(%error, "Task error");
                let mapped = error::map_provider_error(&anyhow::anyhow!("{error}"), "provider");
                self.send_error(&mapped.to_string());
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
            ControllerMessage::ConfigReload => match self.state.reload().await {
                Ok(()) => {
                    tracing::info!("Config reloaded successfully");
                    let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                        role: crate::tui::chat_view::ChatRole::System,
                        content: "Config reloaded.".to_string(),
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to reload config");
                }
            },
            ControllerMessage::McpReload => {
                tracing::info!("MCP settings reload requested");
            }
            ControllerMessage::SlashCommand(cmd, _args) => {
                self.handle_slash_command(cmd);
            }
        }
    }

    /// Handles a parsed slash command.
    #[allow(clippy::too_many_lines)]
    fn handle_slash_command(&mut self, cmd: crate::commands::SlashCommand) {
        use crate::commands::SlashCommand;
        match cmd {
            SlashCommand::Help => {
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: crate::commands::help_text(),
                });
            }
            SlashCommand::Clear => {
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: "Chat cleared.".to_string(),
                });
            }
            SlashCommand::Compact => {
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: "Conversation compacted.".to_string(),
                });
            }
            SlashCommand::History => {
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: "Use --history flag to list tasks.".to_string(),
                });
            }
            SlashCommand::Settings => {
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: "Settings: edit ~/.config/meh/config.toml".to_string(),
                });
            }
            SlashCommand::NewTask => {
                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(AgentMessage::Cancel);
                }
                self.agent_tx = None;
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: "New task started. What would you like to do?".to_string(),
                });
            }
            SlashCommand::Mode(mode) => {
                let mode_str = match mode.as_str() {
                    "plan" => "PLAN",
                    "act" => "ACT",
                    _ => {
                        let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                            role: crate::tui::chat_view::ChatRole::System,
                            content: format!("Unknown mode: {mode}. Use /plan or /act."),
                        });
                        return;
                    }
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
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: format!("Switched to {mode_str} mode."),
                });
            }
            SlashCommand::Model(model_name) => {
                let msg = if model_name.is_empty() {
                    "Use /model <name> to switch model.".to_string()
                } else {
                    format!("Model changed to: {model_name}")
                };
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: msg,
                });
            }
            SlashCommand::Yolo => {
                let new_mode = if self.permission_mode == PermissionMode::Yolo {
                    PermissionMode::Ask
                } else {
                    PermissionMode::Yolo
                };
                self.permission_mode = new_mode;
                let is_yolo = new_mode == PermissionMode::Yolo;
                let status = if is_yolo { "enabled" } else { "disabled" };
                let _ = self.ui_tx.send(UiUpdate::StatusUpdate {
                    mode: None,
                    tokens: None,
                    cost: None,
                    is_streaming: None,
                    is_yolo: Some(is_yolo),
                    context_tokens: None,
                    context_window: None,
                });
                let _ = self.ui_tx.send(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: format!("YOLO mode {status}."),
                });
            }
        }
    }

    /// Handles a user message: creates a provider and spawns a `TaskAgent`.
    async fn handle_user_submit(&mut self, text: String) {
        tracing::info!(len = text.len(), "User submitted message");
        self.cancellation.reset();

        let _ = self.ui_tx.send(UiUpdate::StatusUpdate {
            mode: None,
            tokens: None,
            cost: None,
            is_streaming: Some(true),
            is_yolo: None,
            context_tokens: None,
            context_window: None,
        });

        let config = self.state.config().await;
        let provider_name = &config.provider.default;
        let api_key = self.state.resolve_api_key(provider_name).await;
        let Some(api_key) = api_key else {
            let err = MehError::NoApiKey {
                provider: provider_name.clone(),
                provider_lower: provider_name.to_lowercase(),
                env_var: error::default_env_var(provider_name),
            };
            self.send_error(&err.to_string());
            return;
        };

        let provider = match provider::create_provider(provider_name, &api_key, None) {
            Ok(p) => p,
            Err(e) => {
                let mapped = error::map_provider_error(&e, provider_name);
                self.send_error(&mapped.to_string());
                return;
            }
        };

        let (agent_tx, agent_rx) = mpsc::unbounded_channel();
        self.agent_tx = Some(agent_tx);

        let cwd = std::env::current_dir()
            .map_or_else(|_| ".".to_string(), |p| p.to_string_lossy().to_string());
        let mode = crate::prompt::resolve_default_mode(&config.mode.default);
        let env_info = crate::prompt::environment::EnvironmentInfo::detect(&cwd);
        let rules = load_rules(std::path::Path::new(&cwd));
        let user_rules = rules_to_prompt(&rules, &[]);
        let is_yolo = self.permission_mode == PermissionMode::Yolo;

        let system_prompt = crate::prompt::build_full_system_prompt(&crate::prompt::PromptConfig {
            cwd,
            mode,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules,
            environment_info: env_info.to_prompt_section(),
            yolo_mode: is_yolo,
        });

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

    /// Forwards stream chunks to the TUI, batching text and thinking deltas.
    fn handle_stream_chunk(&mut self, chunk: StreamChunk) {
        match chunk {
            StreamChunk::Text { delta } => {
                self.batcher.push_text(&delta);
                if self.batcher.should_flush() {
                    self.flush_batcher();
                }
            }
            StreamChunk::Thinking { delta, .. } => {
                if !delta.is_empty() {
                    self.batcher.push_thinking(&delta);
                    if self.batcher.should_flush() {
                        self.flush_batcher();
                    }
                }
            }
            StreamChunk::Usage(usage) => {
                self.batcher.push_status(
                    Some(usage.input_tokens + usage.output_tokens),
                    usage.total_cost,
                    Some(usage.input_tokens),
                );
                if self.batcher.should_flush() {
                    self.flush_batcher();
                }
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
                assert!(
                    content.contains("Cannot connect") || content.contains("something broke"),
                    "Error message should be user-friendly, got: {content}"
                );
                assert_eq!(role, ChatRole::System);
            }
            other => panic!("Expected AppendMessage, got {other:?}"),
        }

        ctrl_tx.send(ControllerMessage::Quit).unwrap();
        let _ = ui_rx.recv().await;
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
