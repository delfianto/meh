//! Terminal UI management — setup, teardown, rendering.

pub mod app_layout;
pub mod chat_view;
pub mod event;
pub mod input;
pub mod settings_view;
pub mod status_bar;
pub mod thinking_view;
pub mod tool_view;

use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use std::io;

/// Manages the terminal state and provides the render surface.
///
/// On creation:
/// - Enables raw mode (no line buffering, no echo)
/// - Enters the alternate screen buffer
///
/// On drop (or explicit `restore()`):
/// - Leaves the alternate screen
/// - Disables raw mode
/// - Shows the cursor
pub struct Tui {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl Tui {
    /// Initialize the terminal for TUI rendering.
    pub fn new() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }

    /// Draw a frame using the provided closure.
    pub fn draw<F>(&mut self, f: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut Frame),
    {
        self.terminal.draw(f)?;
        Ok(())
    }

    /// Restore the terminal to its original state.
    pub fn restore(&mut self) -> anyhow::Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::chat_view::*;
    use super::input::InputWidget;
    use super::status_bar::StatusBarState;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn make_status() -> StatusBarState {
        StatusBarState {
            mode: "ACT".to_string(),
            model_name: "claude-sonnet-4-20250514".to_string(),
            provider: "anthropic".to_string(),
            total_tokens: 0,
            total_cost: 0.0,
            is_streaming: false,
        }
    }

    #[test]
    fn app_layout_renders_without_panic() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let chat_state = ChatViewState::new();
        let input = InputWidget::new();
        let status = make_status();
        terminal
            .draw(|frame| {
                super::app_layout::render_app(frame, &chat_state, &input, &status);
            })
            .unwrap();
    }

    #[test]
    fn render_with_messages() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut chat_state = ChatViewState::new();
        chat_state.push_message(ChatMessage {
            role: ChatRole::User,
            content: "Hello!".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: false,
        });
        chat_state.push_message(ChatMessage {
            role: ChatRole::Assistant,
            content: "Hi there!".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: false,
        });
        let input = InputWidget::new();
        let status = StatusBarState {
            total_tokens: 1500,
            total_cost: 0.0035,
            ..make_status()
        };
        terminal
            .draw(|frame| {
                super::app_layout::render_app(frame, &chat_state, &input, &status);
            })
            .unwrap();
    }

    #[test]
    fn render_streaming_message() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut chat_state = ChatViewState::new();
        chat_state.push_message(ChatMessage {
            role: ChatRole::Assistant,
            content: "Thinking...".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: true,
        });
        let input = InputWidget::new();
        let status = StatusBarState {
            model_name: "test-model".to_string(),
            provider: "test".to_string(),
            is_streaming: true,
            ..make_status()
        };
        terminal
            .draw(|frame| {
                super::app_layout::render_app(frame, &chat_state, &input, &status);
            })
            .unwrap();
    }

    #[test]
    fn render_small_terminal() {
        let backend = TestBackend::new(20, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let chat_state = ChatViewState::new();
        let input = InputWidget::new();
        let status = StatusBarState {
            model_name: "m".to_string(),
            provider: "p".to_string(),
            ..make_status()
        };
        terminal
            .draw(|frame| {
                super::app_layout::render_app(frame, &chat_state, &input, &status);
            })
            .unwrap();
    }

    #[test]
    fn render_all_chat_roles() {
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut chat_state = ChatViewState::new();
        for role in [
            ChatRole::User,
            ChatRole::Assistant,
            ChatRole::System,
            ChatRole::Tool,
        ] {
            chat_state.push_message(ChatMessage {
                role,
                content: format!("{role:?} message"),
                timestamp: chrono::Utc::now(),
                streaming: false,
            });
        }
        let input = InputWidget::new();
        let status = make_status();
        terminal
            .draw(|frame| {
                super::app_layout::render_app(frame, &chat_state, &input, &status);
            })
            .unwrap();
    }

    #[test]
    fn render_multiline_message() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut chat_state = ChatViewState::new();
        chat_state.push_message(ChatMessage {
            role: ChatRole::User,
            content: "line one\nline two\nline three".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: false,
        });
        let input = InputWidget::new();
        let status = make_status();
        terminal
            .draw(|frame| {
                super::app_layout::render_app(frame, &chat_state, &input, &status);
            })
            .unwrap();
    }

    #[test]
    fn render_plan_mode_status() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let chat_state = ChatViewState::new();
        let input = InputWidget::new();
        let status = StatusBarState {
            mode: "PLAN".to_string(),
            ..make_status()
        };
        terminal
            .draw(|frame| {
                super::app_layout::render_app(frame, &chat_state, &input, &status);
            })
            .unwrap();
    }
}
