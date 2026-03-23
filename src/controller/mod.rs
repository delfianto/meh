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
use crate::permission::auto_approve::AutoApproveRules;
use crate::permission::command_perms::CommandPermissions;
use crate::permission::{PermissionController, PermissionMode, PermissionResult};
use crate::prompt::rules::{load_rules, rules_to_prompt};
use crate::provider::{self, ModelConfig, StreamChunk};
use crate::state::StateManager;
use crate::state::history::{AutoSaver, PersistedTask, TaskHistory};
use crate::streaming::ui_batcher::UiBatcher;
use crate::tool::executor::ToolExecutor;
use crate::tool::{ToolContext, ToolRegistry};
use messages::{ControllerMessage, ToolCallRequest, ToolCallResult, UiUpdate};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use task::TaskCancellation;
use tokio::sync::mpsc;

/// The Controller is the central message router.
///
/// All components send messages to the controller via clones of the
/// `UnboundedSender<ControllerMessage>` returned by [`Controller::new`].
/// The controller sends UI updates via `ui_tx`.
pub struct Controller {
    /// Receives messages from all components (unbounded — agents send many chunks).
    rx: mpsc::UnboundedReceiver<ControllerMessage>,
    /// Sends updates to the TUI (bounded for backpressure).
    ui_tx: mpsc::Sender<UiUpdate>,
    /// Clone of the controller's own sender (given to agents).
    ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
    /// Sender to the active agent (if any).
    agent_tx: Option<mpsc::UnboundedSender<AgentMessage>>,
    /// Application state.
    state: StateManager,
    /// Permission controller with auto-approve rules.
    permission_ctrl: PermissionController,
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
    /// Executes tool calls via the tool registry.
    tool_executor: Arc<ToolExecutor>,
    /// Working directory for the current session.
    cwd: String,
    /// Tool calls awaiting user approval.
    pending_tool_calls: HashMap<String, ToolCallRequest>,
    /// Handle to the running agent task for abort safety.
    agent_handle: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl Controller {
    /// Creates a new Controller and returns `(controller, ctrl_tx, ui_rx)`.
    ///
    /// Uses bounded channels (4096 capacity) for backpressure safety.
    pub fn new(
        state: StateManager,
        permission_mode: PermissionMode,
    ) -> (
        Self,
        mpsc::UnboundedSender<ControllerMessage>,
        mpsc::Receiver<UiUpdate>,
    ) {
        let (ctrl_tx, rx) = mpsc::unbounded_channel();
        let (ui_tx, ui_rx) = mpsc::channel(4096);
        let cwd_path = std::env::current_dir().unwrap_or_default();
        let cwd = cwd_path.to_string_lossy().to_string();
        let ignore = IgnoreController::new(&cwd_path);
        let auto_saver = TaskHistory::default_dir()
            .ok()
            .and_then(|dir| TaskHistory::new(dir).ok())
            .map(AutoSaver::new);
        let tool_executor = Arc::new(ToolExecutor::new(Arc::new(ToolRegistry::with_defaults())));
        let permission_ctrl = PermissionController::new(
            permission_mode,
            AutoApproveRules::default(),
            CommandPermissions::default(),
        );

        let ctrl = Self {
            rx,
            ui_tx,
            ctrl_tx: ctrl_tx.clone(),
            agent_tx: None,
            state,
            permission_ctrl,
            running: true,
            batcher: UiBatcher::new(60),
            cancellation: TaskCancellation::new(),
            ignore,
            auto_saver,
            tool_executor,
            cwd,
            pending_tool_calls: HashMap::new(),
            agent_handle: None,
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
            let _ = self.ui_tx.try_send(update);
        }
    }

    /// Send a UI update, dropping if channel is full.
    fn send_ui(&self, update: UiUpdate) {
        if self.ui_tx.try_send(update).is_err() {
            tracing::warn!("UI channel full, dropping update");
        }
    }

    /// Send a user-facing error message to the TUI.
    fn send_error(&self, message: &str) {
        self.send_ui(UiUpdate::AppendMessage {
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
                self.send_ui(UiUpdate::Quit);
                self.running = false;
            }
            ControllerMessage::CancelTask => {
                let is_double = self.cancellation.cancel();
                if is_double {
                    tracing::info!("Double cancel detected, force quitting");
                    self.send_ui(UiUpdate::Quit);
                    self.running = false;
                    return;
                }
                tracing::info!("Task cancellation requested");
                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(AgentMessage::Cancel);
                }
                if let Some(handle) = self.agent_handle.take() {
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        if !handle.is_finished() {
                            tracing::warn!("Agent did not exit within 5s, aborting");
                            handle.abort();
                        }
                    });
                }
                self.flush_batcher();
                self.send_ui(UiUpdate::StreamEnd);
                self.send_ui(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: "Task cancelled by user.".to_string(),
                });
                self.send_ui(UiUpdate::StatusUpdate {
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
                let new_mode = if self.permission_ctrl.mode() == PermissionMode::Yolo {
                    PermissionMode::Ask
                } else {
                    PermissionMode::Yolo
                };
                self.permission_ctrl.set_mode(new_mode);
                let is_yolo = new_mode == PermissionMode::Yolo;
                tracing::info!(?new_mode, "Permission mode toggled");
                self.send_ui(UiUpdate::StatusUpdate {
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
                self.send_ui(UiUpdate::StatusUpdate {
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
                always_allow,
            } => {
                if let Some(req) = self.pending_tool_calls.remove(&tool_use_id) {
                    if approved {
                        if always_allow {
                            self.permission_ctrl.always_allow_tool(&req.tool_name);
                        }
                        self.execute_tool(req);
                    } else if let Some(tx) = &self.agent_tx {
                        let _ = tx.send(AgentMessage::ToolCallResult(ToolCallResult {
                            tool_use_id,
                            content: "Tool call denied by user.".to_string(),
                            is_error: true,
                        }));
                    }
                }
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
                self.agent_handle = None;
                self.send_ui(UiUpdate::StreamEnd);
                if let Some(msg) = result.completion_message {
                    self.send_ui(UiUpdate::AppendMessage {
                        role: crate::tui::chat_view::ChatRole::System,
                        content: msg,
                    });
                }
                self.send_ui(UiUpdate::StatusUpdate {
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
                self.send_ui(UiUpdate::StatusUpdate {
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
                    self.send_ui(UiUpdate::AppendMessage {
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
            ControllerMessage::SettingsChange(change) => {
                self.apply_settings_change(change).await;
            }
        }
    }

    /// Apply a settings change from the settings UI.
    async fn apply_settings_change(&self, change: crate::tui::settings_view::SettingsChange) {
        let key = change.key.clone();
        let value = change.value.clone();

        let result = self
            .state
            .update_config(|config| match key.as_str() {
                "provider.default" => config.provider.default.clone_from(&value),
                "provider.anthropic.api_key" => {
                    if value.starts_with('$') {
                        config.provider.anthropic.api_key_env =
                            Some(value.trim_start_matches('$').to_string());
                        config.provider.anthropic.api_key = None;
                    } else {
                        config.provider.anthropic.api_key = Some(value.clone());
                    }
                }
                "provider.anthropic.model" => {
                    config.provider.anthropic.model = Some(value.clone());
                }
                "provider.openai.api_key" => {
                    if value.starts_with('$') {
                        config.provider.openai.api_key_env =
                            Some(value.trim_start_matches('$').to_string());
                        config.provider.openai.api_key = None;
                    } else {
                        config.provider.openai.api_key = Some(value.clone());
                    }
                }
                "provider.gemini.api_key" => {
                    if value.starts_with('$') {
                        config.provider.gemini.api_key_env =
                            Some(value.trim_start_matches('$').to_string());
                        config.provider.gemini.api_key = None;
                    } else {
                        config.provider.gemini.api_key = Some(value.clone());
                    }
                }
                "provider.openrouter.api_key" => {
                    if value.starts_with('$') {
                        config.provider.openrouter.api_key_env =
                            Some(value.trim_start_matches('$').to_string());
                        config.provider.openrouter.api_key = None;
                    } else {
                        config.provider.openrouter.api_key = Some(value.clone());
                    }
                }
                "permissions.mode" => config.permissions.mode.clone_from(&value),
                "permissions.auto_approve.read_files" => {
                    config.permissions.auto_approve.read_files = value == "true";
                }
                "permissions.auto_approve.edit_files" => {
                    config.permissions.auto_approve.edit_files = value == "true";
                }
                "permissions.auto_approve.execute_safe_commands" => {
                    config.permissions.auto_approve.execute_safe_commands = value == "true";
                }
                "permissions.auto_approve.execute_all_commands" => {
                    config.permissions.auto_approve.execute_all_commands = value == "true";
                }
                "mode.default" => config.mode.default.clone_from(&value),
                "mode.strict_plan" => config.mode.strict_plan = value == "true",
                _ => tracing::warn!(key, "Unknown settings key"),
            })
            .await;

        if let Err(e) = result {
            tracing::error!(error = %e, "Failed to apply settings change");
            return;
        }

        if let Err(e) = self.state.persist().await {
            tracing::error!(error = %e, "Failed to persist settings");
        }

        self.send_ui(UiUpdate::AppendMessage {
            role: crate::tui::chat_view::ChatRole::System,
            content: format!("Setting updated: {}", change.key),
        });

        let updated = self.state.config().await;
        self.send_ui(UiUpdate::ConfigUpdated(Box::new(updated)));
    }

    /// Handles a parsed slash command.
    #[allow(clippy::too_many_lines)]
    fn handle_slash_command(&mut self, cmd: crate::commands::SlashCommand) {
        use crate::commands::SlashCommand;
        match cmd {
            SlashCommand::Help => {
                self.send_ui(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: crate::commands::help_text(),
                });
            }
            SlashCommand::Clear => {
                self.send_ui(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: "Chat cleared.".to_string(),
                });
            }
            SlashCommand::Compact => {
                self.send_ui(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: "Conversation compacted.".to_string(),
                });
            }
            SlashCommand::History => {
                self.send_ui(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: "Use --history flag to list tasks.".to_string(),
                });
            }
            SlashCommand::Settings => {
                self.send_ui(UiUpdate::ShowSettings);
            }
            SlashCommand::NewTask => {
                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(AgentMessage::Cancel);
                }
                self.agent_tx = None;
                self.send_ui(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: "New task started. What would you like to do?".to_string(),
                });
            }
            SlashCommand::Mode(mode) => {
                let mode_str = match mode.as_str() {
                    "plan" => "PLAN",
                    "act" => "ACT",
                    _ => {
                        self.send_ui(UiUpdate::AppendMessage {
                            role: crate::tui::chat_view::ChatRole::System,
                            content: format!("Unknown mode: {mode}. Use /plan or /act."),
                        });
                        return;
                    }
                };
                self.send_ui(UiUpdate::StatusUpdate {
                    mode: Some(mode_str.to_string()),
                    tokens: None,
                    cost: None,
                    is_streaming: None,
                    is_yolo: None,
                    context_tokens: None,
                    context_window: None,
                });
                self.send_ui(UiUpdate::AppendMessage {
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
                self.send_ui(UiUpdate::AppendMessage {
                    role: crate::tui::chat_view::ChatRole::System,
                    content: msg,
                });
            }
            SlashCommand::Yolo => {
                let new_mode = if self.permission_ctrl.mode() == PermissionMode::Yolo {
                    PermissionMode::Ask
                } else {
                    PermissionMode::Yolo
                };
                self.permission_ctrl.set_mode(new_mode);
                let is_yolo = new_mode == PermissionMode::Yolo;
                let status = if is_yolo { "enabled" } else { "disabled" };
                self.send_ui(UiUpdate::StatusUpdate {
                    mode: None,
                    tokens: None,
                    cost: None,
                    is_streaming: None,
                    is_yolo: Some(is_yolo),
                    context_tokens: None,
                    context_window: None,
                });
                self.send_ui(UiUpdate::AppendMessage {
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

        self.send_ui(UiUpdate::StatusUpdate {
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

        let mode = crate::prompt::resolve_default_mode(&config.mode.default);
        let env_info = crate::prompt::environment::EnvironmentInfo::detect(&self.cwd);
        let rules = load_rules(std::path::Path::new(&self.cwd));
        let user_rules = rules_to_prompt(&rules, &[]);
        let is_yolo = self.permission_ctrl.mode() == PermissionMode::Yolo;

        let system_prompt = crate::prompt::build_full_system_prompt(&crate::prompt::PromptConfig {
            cwd: self.cwd.clone(),
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
                .unwrap_or_else(|| "claude-sonnet-4-6".to_string()),
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

        self.agent_handle = Some(tokio::spawn(agent.run()));
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

    /// Handles a tool call request — checks permissions before execution.
    fn handle_tool_call_request(&mut self, req: ToolCallRequest) {
        let category = self.tool_executor.registry().get(&req.tool_name).map_or(
            crate::tool::ToolCategory::Command,
            crate::tool::ToolHandler::category,
        );

        let result = self
            .permission_ctrl
            .check_tool(&req.tool_name, category, &req.description);

        match result {
            PermissionResult::Approved => {
                self.execute_tool(req);
            }
            PermissionResult::NeedsApproval {
                tool_name,
                description,
            } => {
                tracing::info!(tool = tool_name, "Requesting tool approval");
                self.send_ui(UiUpdate::ToolApproval {
                    tool_use_id: req.tool_use_id.clone(),
                    tool_name,
                    description,
                });
                self.pending_tool_calls.insert(req.tool_use_id.clone(), req);
            }
            PermissionResult::Denied { reason } => {
                tracing::warn!(tool = req.tool_name, reason, "Tool call denied");
                if let Some(tx) = &self.agent_tx {
                    let _ = tx.send(AgentMessage::ToolCallResult(ToolCallResult {
                        tool_use_id: req.tool_use_id,
                        content: format!("Permission denied: {reason}"),
                        is_error: true,
                    }));
                }
            }
        }
    }

    /// Execute a tool call (after permission check passed).
    fn execute_tool(&self, req: ToolCallRequest) {
        tracing::info!(tool = req.tool_name, "Executing tool call");
        let executor = self.tool_executor.clone();
        let agent_tx = self.agent_tx.clone();
        let ctx = ToolContext {
            cwd: self.cwd.clone(),
            auto_approved: self.permission_ctrl.mode() == PermissionMode::Yolo,
        };

        tokio::spawn(async move {
            let response = executor.execute(&req.tool_name, req.arguments, &ctx).await;
            if let Some(tx) = agent_tx {
                let (content, is_error) = match response {
                    Ok(r) => (r.content, r.is_error),
                    Err(e) => (format!("Tool error: {e}"), true),
                };
                let _ = tx.send(AgentMessage::ToolCallResult(ToolCallResult {
                    tool_use_id: req.tool_use_id,
                    content,
                    is_error,
                }));
            }
        });
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
        tokio::sync::mpsc::Receiver<UiUpdate>,
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
