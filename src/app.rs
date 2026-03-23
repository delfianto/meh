//! Application entry point — owns Controller and TUI, runs the main event loop.

use crate::Cli;
use crate::controller::Controller;
use crate::controller::messages::{ControllerMessage, UiUpdate};
use crate::permission::PermissionMode;
use crate::permission::yolo::is_yolo_mode;
use crate::state::StateManager;
use crate::tui::{
    self,
    chat_view::{ChatMessage, ChatRole, ChatViewState},
    input::InputWidget,
    settings_view::{SettingsAction, SettingsView},
    status_bar::StatusBarState,
};
use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers};
use futures::StreamExt;
use ratatui::layout::{Constraint, Layout};
use std::time::Duration;
use tokio::sync::mpsc;

/// Top-level application struct.
pub struct App {
    cli: Cli,
    state: StateManager,
}

impl App {
    /// Creates a new application instance from CLI arguments.
    pub async fn new(cli: Cli) -> anyhow::Result<Self> {
        let state = StateManager::new(cli.config.clone()).await?;
        let config = state.config().await;
        tracing::debug!(?config, "Loaded configuration");
        Ok(Self { cli, state })
    }

    /// Runs the application main loop.
    pub async fn run(self) -> anyhow::Result<()> {
        tracing::info!("meh starting up");

        let config = self.state.config().await;

        for err in crate::error::validate_config(&config) {
            tracing::warn!("{err}");
        }

        let yolo = is_yolo_mode(&config.permissions, self.cli.yolo);
        let permission_mode = if yolo {
            PermissionMode::Yolo
        } else {
            match config.permissions.mode.as_str() {
                "auto" => PermissionMode::Auto,
                _ => PermissionMode::Ask,
            }
        };

        let (controller, ctrl_tx, ui_rx) = Controller::new(self.state.clone(), permission_mode);

        let config_path = self.state.config_path().await;
        crate::state::watcher::spawn_config_watcher(config_path, ctrl_tx.clone());

        let controller_handle = tokio::spawn(controller.run());

        let mut initial_prompt = self.cli.prompt.clone();

        if let Some(ref task_id) = self.cli.resume {
            match load_resume_context(task_id) {
                Ok(context) => initial_prompt = Some(context),
                Err(e) => {
                    tracing::error!(task_id, error = %e, "Failed to resume task");
                    anyhow::bail!("Failed to resume task '{task_id}': {e}");
                }
            }
        }

        let default_provider = config.provider.default.clone();
        let tui_result = run_tui_async(
            &ctrl_tx,
            ui_rx,
            &default_provider,
            initial_prompt,
            yolo,
            config,
        )
        .await;

        controller_handle.abort();
        tui_result
    }
}

/// Load resume context from task history.
fn load_resume_context(task_id: &str) -> anyhow::Result<String> {
    let history_dir = crate::state::history::TaskHistory::default_dir()?;
    let history = crate::state::history::TaskHistory::new(history_dir)?;
    let task = history.load_task(task_id)?;

    let last_user_text = task
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.first())
        .and_then(|c| match c {
            crate::state::history::PersistedContent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();

    Ok(format!(
        "[Resuming task: {}]\n{}",
        task.title, last_user_text
    ))
}

/// Tracks whether we're currently in a streaming assistant message.
struct StreamState {
    is_streaming: bool,
}

impl StreamState {
    /// Ensures a streaming assistant message exists in the chat.
    fn ensure_streaming_message(&mut self, chat_state: &mut ChatViewState) {
        if !self.is_streaming {
            chat_state.push_message(ChatMessage {
                role: ChatRole::Assistant,
                content: String::new(),
                timestamp: chrono::Utc::now(),
                streaming: true,
            });
            self.is_streaming = true;
        }
    }
}

/// Applies a single UI update to the TUI state. Returns `true` if the app should quit.
fn apply_ui_update(
    update: UiUpdate,
    chat_state: &mut ChatViewState,
    status: &mut StatusBarState,
    stream_state: &mut StreamState,
) -> bool {
    match update {
        UiUpdate::AppendMessage { role, content } => {
            chat_state.push_message(ChatMessage {
                role,
                content,
                timestamp: chrono::Utc::now(),
                streaming: false,
            });
        }
        UiUpdate::StreamContent { delta } | UiUpdate::ThinkingContent { delta } => {
            stream_state.ensure_streaming_message(chat_state);
            if let Some(last) = chat_state.messages.last_mut() {
                last.content.push_str(&delta);
            }
        }
        UiUpdate::StreamEnd => {
            if let Some(last) = chat_state.messages.last_mut() {
                last.streaming = false;
            }
            stream_state.is_streaming = false;
        }
        UiUpdate::StatusUpdate {
            mode,
            tokens,
            cost,
            is_streaming,
            is_yolo,
            context_tokens,
            context_window,
        } => {
            if let Some(m) = mode {
                status.mode = m;
            }
            if let Some(t) = tokens {
                status.total_tokens = t;
            }
            if let Some(c) = cost {
                status.total_cost = c;
            }
            if let Some(s) = is_streaming {
                status.is_streaming = s;
            }
            if let Some(y) = is_yolo {
                status.is_yolo = y;
            }
            if let Some(ct) = context_tokens {
                status.context_tokens = ct;
            }
            if let Some(cw) = context_window {
                status.context_window = cw;
            }
        }
        UiUpdate::ToolApproval { .. }
        | UiUpdate::SubAgentUpdate { .. }
        | UiUpdate::ShowSettings
        | UiUpdate::ConfigUpdated(_) => {}
        UiUpdate::Quit => return true,
    }
    false
}

/// Async TUI event loop using `crossterm::EventStream` + `tokio::select!`.
///
/// Zero CPU when idle — sleeps until a terminal event, UI update, or render tick fires.
/// Wrapped in `catch_unwind` for panic safety.
#[allow(clippy::too_many_lines)]
async fn run_tui_async(
    ctrl_tx: &mpsc::UnboundedSender<ControllerMessage>,
    mut ui_rx: mpsc::Receiver<UiUpdate>,
    default_provider: &str,
    initial_prompt: Option<String>,
    yolo: bool,
    app_config: crate::state::config::AppConfig,
) -> anyhow::Result<()> {
    let mut tui = tui::Tui::new()?;
    let mut chat_state = ChatViewState::new();
    let mut input = InputWidget::new();
    let mut status = StatusBarState {
        mode: "ACT".to_string(),
        model_name: "claude-sonnet-4-6".to_string(),
        provider: default_provider.to_string(),
        total_tokens: 0,
        total_cost: 0.0,
        is_streaming: false,
        is_yolo: yolo,
        context_tokens: 0,
        context_window: 0,
    };
    let mut stream_state = StreamState {
        is_streaming: false,
    };

    chat_state.push_message(ChatMessage {
        role: ChatRole::System,
        content: "Welcome to meh. Type a message to begin.".to_string(),
        timestamp: chrono::Utc::now(),
        streaming: false,
    });

    if let Some(prompt) = initial_prompt {
        chat_state.push_message(ChatMessage {
            role: ChatRole::User,
            content: prompt.clone(),
            timestamp: chrono::Utc::now(),
            streaming: false,
        });
        let _ = ctrl_tx.send(ControllerMessage::UserSubmit {
            text: prompt,
            images: vec![],
        });
    }

    let mut event_stream = EventStream::new();
    let mut render_tick = tokio::time::interval(Duration::from_millis(16));
    let mut dirty = true;
    let mut settings_view: Option<SettingsView> = None;
    let mut current_config = app_config;

    loop {
        tokio::select! {
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        dirty = true;

                        // Global: Ctrl+C always cancels/quits
                        if key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            let _ = ctrl_tx.send(ControllerMessage::CancelTask);
                            continue;
                        }

                        // Route to settings view if active
                        if let Some(ref mut sv) = settings_view {
                            match sv.handle_key(key, &current_config) {
                                SettingsAction::Close => {
                                    sv.editing = None;
                                    settings_view = None;
                                }
                                SettingsAction::Apply(change) => {
                                    sv.editing = None;
                                    let _ = ctrl_tx.send(
                                        ControllerMessage::SettingsChange(change)
                                    );
                                }
                                SettingsAction::Continue => {}
                            }
                            continue;
                        }

                        // Chat mode key handling
                        if key.code == KeyCode::Char('y')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            let _ = ctrl_tx.send(ControllerMessage::ToggleYolo);
                            continue;
                        }
                        if let Some(text) = input.handle_key(key) {
                            if let Some((cmd, args)) =
                                crate::commands::parse_slash_command(&text)
                            {
                                let _ = ctrl_tx
                                    .send(ControllerMessage::SlashCommand(cmd, args));
                            } else {
                                chat_state.push_message(ChatMessage {
                                    role: ChatRole::User,
                                    content: text.clone(),
                                    timestamp: chrono::Utc::now(),
                                    streaming: false,
                                });
                                let _ = ctrl_tx.send(ControllerMessage::UserSubmit {
                                    text,
                                    images: vec![],
                                });
                            }
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => dirty = true,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => tracing::warn!(error = %e, "Event stream error"),
                    None => break,
                }
            }

            update = ui_rx.recv() => {
                match update {
                    Some(UiUpdate::ShowSettings) => {
                        settings_view = Some(SettingsView::new(&current_config));
                        dirty = true;
                    }
                    Some(UiUpdate::ConfigUpdated(new_config)) => {
                        current_config = *new_config;
                        if let Some(ref mut sv) = settings_view {
                            sv.rebuild_rows(&current_config);
                        }
                        dirty = true;
                    }
                    Some(u) => {
                        if apply_ui_update(u, &mut chat_state, &mut status, &mut stream_state) {
                            break;
                        }
                        dirty = true;
                    }
                    None => break,
                }
            }

            _ = render_tick.tick() => {
                if dirty {
                    tui.draw(|frame| {
                        if let Some(ref sv) = settings_view {
                            let chunks = Layout::vertical([
                                Constraint::Percentage(40),
                                Constraint::Percentage(60),
                            ]).split(frame.area());
                            tui::app_layout::render_app_in(
                                frame, chunks[0], &chat_state, &input, &status,
                            );
                            crate::tui::settings_view::render_settings(
                                frame, chunks[1], sv,
                            );
                        } else {
                            tui::app_layout::render_app(
                                frame, &chat_state, &input, &status,
                            );
                        }
                    })?;
                    dirty = false;
                }
            }
        }
    }

    tui.restore()?;
    Ok(())
}
