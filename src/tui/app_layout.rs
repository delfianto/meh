//! Main TUI layout — composes chat view, status bar, and input.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders};

/// Renders the complete application layout.
///
/// Layout (top to bottom):
///   1. Status bar (1 line)
///   2. Chat view (fills remaining space, min 5 lines)
///   3. Input area (3 lines with border)
pub fn render_app(
    frame: &mut Frame,
    chat_state: &super::chat_view::ChatViewState,
    input: &super::input::InputWidget,
    status: &super::status_bar::StatusBarState,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Status bar
            Constraint::Min(5),    // Chat view (fills remaining)
            Constraint::Length(3), // Input area
        ])
        .split(frame.area());

    super::status_bar::render_status_bar(frame, chunks[0], status);
    super::chat_view::render_chat_view(frame, chunks[1], chat_state);

    // Render input with a border
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Input ");
    let inner = input_block.inner(chunks[2]);
    frame.render_widget(input_block, chunks[2]);
    frame.render_widget(&input.textarea, inner);
}
