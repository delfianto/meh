//! Scrollable chat view showing conversation messages.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

/// A single message in the chat.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// If true, content is still streaming (show cursor/spinner).
    pub streaming: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
    System,
    Tool,
}

/// State for the chat view (scroll position, messages).
pub struct ChatViewState {
    pub messages: Vec<ChatMessage>,
    pub scroll_offset: u16,
    pub auto_scroll: bool,
}

impl ChatViewState {
    pub const fn new() -> Self {
        Self {
            messages: Vec::new(),
            scroll_offset: 0,
            auto_scroll: true,
        }
    }

    /// Push a new message. If `auto_scroll` is on, scroll to bottom.
    pub fn push_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
        if self.auto_scroll {
            self.scroll_offset = u16::MAX;
        }
    }

    /// Update the content of the last message (for streaming updates).
    pub fn update_last_message(&mut self, content: &str) {
        if let Some(last) = self.messages.last_mut() {
            last.content = content.to_string();
        }
    }

    /// Scroll up by `amount` lines. Disables `auto_scroll`.
    pub const fn scroll_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        self.auto_scroll = false;
    }

    /// Scroll down by `amount` lines. Re-enables `auto_scroll` if at bottom.
    pub fn scroll_down(&mut self, amount: u16, max: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount).min(max);
        if self.scroll_offset >= max {
            self.auto_scroll = true;
        }
    }

    /// Jump to the bottom and re-enable `auto_scroll`.
    pub const fn scroll_to_bottom(&mut self) {
        self.scroll_offset = u16::MAX;
        self.auto_scroll = true;
    }
}

/// Renders the chat view as a Ratatui widget.
pub fn render_chat_view(frame: &mut Frame, area: Rect, state: &ChatViewState) {
    let mut lines: Vec<Line<'_>> = Vec::new();

    for (i, msg) in state.messages.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }

        let (prefix, style) = match msg.role {
            ChatRole::User => ("You: ", Style::default().fg(Color::Cyan).bold()),
            ChatRole::Assistant => ("Assistant: ", Style::default().fg(Color::Green)),
            ChatRole::System => ("System: ", Style::default().fg(Color::DarkGray).italic()),
            ChatRole::Tool => ("Tool: ", Style::default().fg(Color::Yellow)),
        };

        let mut content = msg.content.clone();
        if msg.streaming {
            content.push('\u{258C}'); // ▌ block cursor
        }

        let message_lines: Vec<&str> = content.split('\n').collect();
        for (j, line) in message_lines.iter().enumerate() {
            if j == 0 {
                lines.push(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::raw(line.to_string()),
                ]));
            } else {
                let indent = " ".repeat(prefix.len());
                lines.push(Line::from(format!("{indent}{line}")));
            }
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Chat ");

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((state.scroll_offset, 0));

    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_view_state_new() {
        let state = ChatViewState::new();
        assert!(state.messages.is_empty());
        assert!(state.auto_scroll);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn push_message() {
        let mut state = ChatViewState::new();
        state.push_message(ChatMessage {
            role: ChatRole::User,
            content: "hello".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: false,
        });
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].content, "hello");
    }

    #[test]
    fn push_message_auto_scrolls() {
        let mut state = ChatViewState::new();
        state.push_message(ChatMessage {
            role: ChatRole::User,
            content: "msg1".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: false,
        });
        assert_eq!(state.scroll_offset, u16::MAX);
        assert!(state.auto_scroll);
    }

    #[test]
    fn update_last_message() {
        let mut state = ChatViewState::new();
        state.push_message(ChatMessage {
            role: ChatRole::Assistant,
            content: "He".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: true,
        });
        state.update_last_message("Hello, world!");
        assert_eq!(state.messages[0].content, "Hello, world!");
    }

    #[test]
    fn update_last_message_empty_vec() {
        let mut state = ChatViewState::new();
        state.update_last_message("no-op");
        assert!(state.messages.is_empty());
    }

    #[test]
    fn scroll_up_disables_auto_scroll() {
        let mut state = ChatViewState::new();
        state.scroll_offset = 10;
        state.scroll_up(5);
        assert_eq!(state.scroll_offset, 5);
        assert!(!state.auto_scroll);
    }

    #[test]
    fn scroll_up_saturates_at_zero() {
        let mut state = ChatViewState::new();
        state.scroll_offset = 2;
        state.scroll_up(10);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn scroll_down_clamps_to_max() {
        let mut state = ChatViewState::new();
        state.auto_scroll = false;
        state.scroll_down(100, 50);
        assert_eq!(state.scroll_offset, 50);
        assert!(state.auto_scroll);
    }

    #[test]
    fn scroll_to_bottom_re_enables() {
        let mut state = ChatViewState::new();
        state.scroll_up(5);
        assert!(!state.auto_scroll);
        state.scroll_to_bottom();
        assert!(state.auto_scroll);
        assert_eq!(state.scroll_offset, u16::MAX);
    }

    #[test]
    fn chat_role_equality() {
        assert_eq!(ChatRole::User, ChatRole::User);
        assert_ne!(ChatRole::User, ChatRole::Assistant);
    }
}
