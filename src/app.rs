//! Application entry point — owns Controller and TUI, runs the main event loop.

use crate::Cli;
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

        let mut tui = tui::Tui::new()?;
        let mut chat_state = ChatViewState::new();
        let mut input = InputWidget::new();
        let config = self.state.config().await;
        let status = StatusBarState {
            mode: "ACT".to_string(),
            model_name: "claude-sonnet-4-20250514".to_string(),
            provider: config.provider.default,
            total_tokens: 0,
            total_cost: 0.0,
            is_streaming: false,
        };

        // Add a welcome message
        chat_state.push_message(ChatMessage {
            role: ChatRole::System,
            content: "Welcome to meh. Type a message to begin.".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: false,
        });

        // If an initial prompt was provided, submit it immediately
        if let Some(ref prompt) = self.cli.prompt {
            chat_state.push_message(ChatMessage {
                role: ChatRole::User,
                content: prompt.clone(),
                timestamp: chrono::Utc::now(),
                streaming: false,
            });
        }

        loop {
            // Render
            tui.draw(|frame| {
                tui::app_layout::render_app(frame, &chat_state, &input, &status);
            })?;

            // Handle events
            if let Some(event) = poll_event(Duration::from_millis(16)) {
                match event {
                    TuiEvent::Key(key) => {
                        // Ctrl+C → quit
                        if key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            break;
                        }
                        // Forward to input
                        if let Some(text) = input.handle_key(key) {
                            chat_state.push_message(ChatMessage {
                                role: ChatRole::User,
                                content: text,
                                timestamp: chrono::Utc::now(),
                                streaming: false,
                            });
                        }
                    }
                    TuiEvent::Resize(_, _) | TuiEvent::Tick | TuiEvent::Mouse(_) => {}
                }
            }
        }

        tui.restore()?;
        Ok(())
    }
}
