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
    event::{TuiEvent, poll_event},
    input::InputWidget,
    status_bar::StatusBarState,
};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
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

        let controller_handle = tokio::spawn(controller.run());

        let initial_prompt = self.cli.prompt.clone();
        let default_provider = config.provider.default.clone();
        let tui_result = tokio::task::spawn_blocking(move || {
            run_tui(&ctrl_tx, ui_rx, &default_provider, initial_prompt, yolo)
        })
        .await?;

        controller_handle.abort();
        tui_result
    }
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
        UiUpdate::ToolApproval { .. } | UiUpdate::SubAgentUpdate { .. } => {}
        UiUpdate::Quit => return true,
    }
    false
}

/// The TUI event loop. Runs on a blocking thread.
fn run_tui(
    ctrl_tx: &mpsc::UnboundedSender<ControllerMessage>,
    mut ui_rx: mpsc::UnboundedReceiver<UiUpdate>,
    default_provider: &str,
    initial_prompt: Option<String>,
    yolo: bool,
) -> anyhow::Result<()> {
    let mut tui = tui::Tui::new()?;
    let mut chat_state = ChatViewState::new();
    let mut input = InputWidget::new();
    let mut status = StatusBarState {
        mode: "ACT".to_string(),
        model_name: "claude-sonnet-4-20250514".to_string(),
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

    loop {
        while let Ok(update) = ui_rx.try_recv() {
            if apply_ui_update(update, &mut chat_state, &mut status, &mut stream_state) {
                tui.restore()?;
                return Ok(());
            }
        }

        tui.draw(|frame| {
            tui::app_layout::render_app(frame, &chat_state, &input, &status);
        })?;

        if let Some(event) = poll_event(Duration::from_millis(16)) {
            match event {
                TuiEvent::Key(key) => {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        let _ = ctrl_tx.send(ControllerMessage::Quit);
                        continue;
                    }
                    if key.code == KeyCode::Char('y')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        let _ = ctrl_tx.send(ControllerMessage::ToggleYolo);
                        continue;
                    }
                    if let Some(text) = input.handle_key(key) {
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
                TuiEvent::Resize(_, _) | TuiEvent::Tick | TuiEvent::Mouse(_) => {}
            }
        }
    }
}
