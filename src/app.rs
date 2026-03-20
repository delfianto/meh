//! Application entry point — owns Controller and TUI, runs the main event loop.

use crate::Cli;
use crate::controller::Controller;
use crate::controller::messages::{ControllerMessage, UiUpdate};
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
    /// Create a new application instance from CLI arguments.
    pub async fn new(cli: Cli) -> anyhow::Result<Self> {
        let state = StateManager::new(cli.config.clone()).await?;
        let config = state.config().await;
        tracing::debug!(?config, "Loaded configuration");
        Ok(Self { cli, state })
    }

    /// Run the application main loop.
    pub async fn run(self) -> anyhow::Result<()> {
        tracing::info!("meh starting up");

        let (controller, ctrl_tx, ui_rx) = Controller::new();

        let controller_handle = tokio::spawn(controller.run());

        let config = self.state.config().await;
        let initial_prompt = self.cli.prompt.clone();
        let tui_result = tokio::task::spawn_blocking(move || {
            run_tui(&ctrl_tx, ui_rx, &config.provider.default, initial_prompt)
        })
        .await?;

        controller_handle.abort();
        tui_result
    }
}

/// Apply a single UI update to the TUI state.
/// Returns `true` if the app should quit.
fn apply_ui_update(
    update: UiUpdate,
    chat_state: &mut ChatViewState,
    status: &mut StatusBarState,
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
        UiUpdate::StreamContent { delta } => {
            if let Some(last) = chat_state.messages.last_mut() {
                last.content.push_str(&delta);
            }
        }
        UiUpdate::StreamEnd => {
            if let Some(last) = chat_state.messages.last_mut() {
                last.streaming = false;
            }
        }
        UiUpdate::StatusUpdate {
            mode,
            tokens,
            cost,
            is_streaming,
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
        }
        UiUpdate::ThinkingContent { .. } | UiUpdate::ToolApproval { .. } => {}
        UiUpdate::Quit => return true,
    }
    false
}

/// The TUI event loop. Runs on a blocking thread.
///
/// This function owns the terminal and handles rendering the UI,
/// polling for keyboard/mouse events, and draining UI updates
/// from the controller.
fn run_tui(
    ctrl_tx: &mpsc::UnboundedSender<ControllerMessage>,
    mut ui_rx: mpsc::UnboundedReceiver<UiUpdate>,
    default_provider: &str,
    initial_prompt: Option<String>,
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
            if apply_ui_update(update, &mut chat_state, &mut status) {
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
