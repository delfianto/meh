//! User input area — multiline text input using ratatui-textarea.

use ratatui::crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui_textarea::TextArea;

/// Wraps `ratatui-textarea` with our keybindings.
pub struct InputWidget<'a> {
    pub textarea: TextArea<'a>,
}

impl InputWidget<'_> {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(ratatui::style::Style::default());
        textarea.set_placeholder_text("Type a message... (Enter to send, Shift+Enter for newline)");
        Self { textarea }
    }

    /// Handle a key event. Returns `Some(text)` if user pressed Enter (submit).
    /// Returns `None` for all other keys (they are forwarded to the textarea).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<String> {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, m) if !m.contains(KeyModifiers::SHIFT) => {
                let text = self.textarea.lines().join("\n").trim().to_string();
                if text.is_empty() {
                    return None;
                }
                self.textarea.select_all();
                self.textarea.cut();
                Some(text)
            }
            _ => {
                self.textarea.input(CrosstermEvent::Key(key));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn enter_submits_text() {
        let mut input = InputWidget::new();
        for c in "hello".chars() {
            input.handle_key(make_key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let result = input.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(result, Some("hello".to_string()));
    }

    #[test]
    fn empty_enter_returns_none() {
        let mut input = InputWidget::new();
        let result = input.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(result.is_none());
    }

    #[test]
    fn input_clears_after_submit() {
        let mut input = InputWidget::new();
        for c in "test".chars() {
            input.handle_key(make_key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        input.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(input.textarea.lines().join("").is_empty());
    }

    #[test]
    fn typing_accumulates() {
        let mut input = InputWidget::new();
        for c in "abc".chars() {
            let result = input.handle_key(make_key(KeyCode::Char(c), KeyModifiers::NONE));
            assert!(result.is_none());
        }
        assert_eq!(input.textarea.lines().join(""), "abc");
    }

    #[test]
    fn whitespace_only_returns_none() {
        let mut input = InputWidget::new();
        input.handle_key(make_key(KeyCode::Char(' '), KeyModifiers::NONE));
        input.handle_key(make_key(KeyCode::Char(' '), KeyModifiers::NONE));
        let result = input.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(result.is_none());
    }
}
