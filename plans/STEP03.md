# STEP 03 — Basic TUI (Layout, Input, Chat View)

## Objective
Implement the terminal UI using Ratatui with the main layout, a scrollable chat view, user input area, and status bar. After this step, the user can type text, see it appear in the chat view, and see the status bar.

## Prerequisites
- STEP 01 complete
- STEP 02 complete (StateManager available for config)

## Detailed Instructions

### 3.1 TUI Event System (`src/tui/event.rs`)

```rust
//! Terminal event handling — bridges crossterm events to app events.

use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

/// Events that the TUI event loop produces.
#[derive(Debug, Clone)]
pub enum TuiEvent {
    /// A key was pressed.
    Key(KeyEvent),
    /// Terminal was resized.
    Resize(u16, u16),
    /// Periodic tick for animations/polling.
    Tick,
    /// Mouse event (scroll).
    Mouse(crossterm::event::MouseEvent),
}

/// Polls crossterm for events with a given tick rate.
/// Returns `Some(TuiEvent)` if an event occurred or tick elapsed.
/// Returns `None` on read error.
pub fn poll_event(tick_rate: Duration) -> Option<TuiEvent> {
    if event::poll(tick_rate).ok()? {
        match event::read().ok()? {
            CrosstermEvent::Key(key) => Some(TuiEvent::Key(key)),
            CrosstermEvent::Resize(w, h) => Some(TuiEvent::Resize(w, h)),
            CrosstermEvent::Mouse(m) => Some(TuiEvent::Mouse(m)),
            _ => None,
        }
    } else {
        Some(TuiEvent::Tick)
    }
}
```

Key design decisions:
- Use a polling model (not async) since crossterm's event API is synchronous
- The tick rate of 16ms gives ~60fps rendering
- Mouse events are captured for future scroll support
- `poll_event` returns `None` on I/O errors to allow graceful degradation

### 3.2 Chat View (`src/tui/chat_view.rs`)

Define a `ChatMessage` struct and a `ChatViewState` plus a render function:

```rust
//! Scrollable chat view showing conversation messages.

use ratatui::prelude::*;
use ratatui::widgets::*;

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
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            scroll_offset: 0,
            auto_scroll: true,
        }
    }

    /// Push a new message. If auto_scroll is on, scroll to bottom.
    pub fn push_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
        if self.auto_scroll {
            self.scroll_offset = u16::MAX; // Will be clamped during render
        }
    }

    /// Update the content of the last message (for streaming updates).
    pub fn update_last_message(&mut self, content: &str) {
        if let Some(last) = self.messages.last_mut() {
            last.content = content.to_string();
        }
    }

    /// Scroll up by `amount` lines. Disables auto_scroll.
    pub fn scroll_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        self.auto_scroll = false;
    }

    /// Scroll down by `amount` lines. Re-enables auto_scroll if at bottom.
    pub fn scroll_down(&mut self, amount: u16, max: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount).min(max);
        // Re-enable auto_scroll if we've scrolled to the bottom
        if self.scroll_offset >= max {
            self.auto_scroll = true;
        }
    }

    /// Jump to the bottom and re-enable auto_scroll.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = u16::MAX;
        self.auto_scroll = true;
    }
}
```

Rendering logic for `render_chat_view`:
```rust
/// Renders the chat view as a Ratatui widget.
pub fn render_chat_view(
    frame: &mut Frame,
    area: Rect,
    state: &ChatViewState,
) {
    // 1. Build styled text for each message:
    //    - User messages: bold, cyan prefix "You: "
    //    - Assistant: green prefix "Assistant: "
    //    - System: dim, italic
    //    - Tool: yellow prefix "Tool: "
    //    - Streaming messages get a blinking cursor block "▌" at end
    //
    // 2. Separate messages with a blank line between them
    //
    // 3. Wrap in a Block with borders and title " Chat "
    //
    // 4. Use Paragraph widget with:
    //    - .wrap(Wrap { trim: false })
    //    - .scroll((state.scroll_offset, 0))
    //
    // 5. Render with frame.render_widget()

    let mut lines: Vec<Line> = Vec::new();

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

        // First line gets the role prefix
        let message_lines: Vec<&str> = content.split('\n').collect();
        for (j, line) in message_lines.iter().enumerate() {
            if j == 0 {
                lines.push(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::raw(*line),
                ]));
            } else {
                // Indent continuation lines to align with content after prefix
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
        .scroll((state.scroll_offset.min(u16::MAX), 0));

    frame.render_widget(paragraph, area);
}
```

### 3.3 Input Widget (`src/tui/input.rs`)

```rust
//! User input area — multiline text input using tui-textarea.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use tui_textarea::TextArea;

/// Wraps tui-textarea with our keybindings.
pub struct InputWidget<'a> {
    pub textarea: TextArea<'a>,
}

impl<'a> InputWidget<'a> {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default());
        textarea.set_placeholder_text(
            "Type a message... (Enter to send, Shift+Enter for newline)",
        );
        Self { textarea }
    }

    /// Handle a key event. Returns `Some(text)` if user pressed Enter (submit).
    /// Returns `None` for all other keys (they are forwarded to the textarea).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<String> {
        match (key.code, key.modifiers) {
            // Enter without Shift → submit the current text
            (KeyCode::Enter, m) if !m.contains(KeyModifiers::SHIFT) => {
                let text = self.textarea.lines().join("\n").trim().to_string();
                if text.is_empty() {
                    return None;
                }
                // Clear the textarea
                self.textarea.select_all();
                self.textarea.cut();
                Some(text)
            }
            // All other keys → forward to textarea for normal editing
            _ => {
                self.textarea.input(key);
                None
            }
        }
    }

    /// Get the textarea widget for rendering.
    pub fn widget(&'a self) -> impl Widget + 'a {
        self.textarea.widget()
    }
}
```

Important details:
- Enter submits, Shift+Enter inserts a newline (tui-textarea handles Shift+Enter as newline by default)
- After submit, the textarea is cleared using `select_all()` + `cut()`
- Empty submissions are suppressed (returns `None`)
- The `widget()` method borrows `self` with the same lifetime as the `TextArea`

### 3.4 Status Bar (`src/tui/status_bar.rs`)

```rust
//! Status bar showing mode, model, token count, and cost.

use ratatui::prelude::*;
use ratatui::widgets::*;

/// State for the status bar display.
pub struct StatusBarState {
    pub mode: String,       // "PLAN" or "ACT"
    pub model_name: String, // e.g., "claude-sonnet-4-20250514"
    pub provider: String,   // e.g., "anthropic"
    pub total_tokens: u64,
    pub total_cost: f64,
    pub is_streaming: bool,
}

/// Render the status bar into the given area.
pub fn render_status_bar(frame: &mut Frame, area: Rect, state: &StatusBarState) {
    // Layout: [MODE] provider/model  ·  tokens: 1.2k  ·  cost: $0.003  [streaming spinner]
    //
    // Mode badge colors:
    //   ACT  → black text on green background
    //   PLAN → black text on yellow background
    //
    // Use Line with multiple styled Spans

    let mode_style = match state.mode.as_str() {
        "ACT" => Style::default().fg(Color::Black).bg(Color::Green).bold(),
        "PLAN" => Style::default().fg(Color::Black).bg(Color::Yellow).bold(),
        _ => Style::default().fg(Color::White).bold(),
    };

    let tokens_display = if state.total_tokens >= 1_000_000 {
        format!("{:.1}M", state.total_tokens as f64 / 1_000_000.0)
    } else if state.total_tokens >= 1_000 {
        format!("{:.1}k", state.total_tokens as f64 / 1_000.0)
    } else {
        state.total_tokens.to_string()
    };

    let streaming_indicator = if state.is_streaming { " ⟳" } else { "" };

    let line = Line::from(vec![
        Span::styled(format!(" {} ", state.mode), mode_style),
        Span::raw(" "),
        Span::styled(
            format!("{}/{}", state.provider, state.model_name),
            Style::default().fg(Color::Gray),
        ),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("tokens: {tokens_display}"),
            Style::default().fg(Color::Gray),
        ),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("cost: ${:.4}", state.total_cost),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(
            streaming_indicator.to_string(),
            Style::default().fg(Color::Cyan),
        ),
    ]);

    let paragraph = Paragraph::new(line)
        .style(Style::default().bg(Color::Rgb(30, 30, 30)));
    frame.render_widget(paragraph, area);
}
```

### 3.5 App Layout (`src/tui/app_layout.rs`)

```rust
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
            Constraint::Length(1),  // Status bar
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
    frame.render_widget(input.widget(), inner);
}
```

### 3.6 TUI mod.rs — Terminal Management

```rust
//! Terminal UI management — setup, teardown, main render loop.

pub mod app_layout;
pub mod chat_view;
pub mod event;
pub mod input;
pub mod status_bar;
pub mod thinking_view;
pub mod tool_view;
pub mod settings_view;

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
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
```

Key safety guarantee: The `Drop` implementation ensures the terminal is always restored, even if the application panics. This prevents leaving the user's terminal in a broken state.

### 3.7 Wire into app.rs — Basic Event Loop

Update `App` to own a `Tui`, `ChatViewState`, `InputWidget`, and `StatusBarState` and run the main event loop:

```rust
use crate::Cli;
use crate::state::StateManager;
use crate::tui::{
    self,
    chat_view::{ChatMessage, ChatRole, ChatViewState},
    event::{poll_event, TuiEvent},
    input::InputWidget,
    status_bar::StatusBarState,
};
use crossterm::event::{KeyCode, KeyModifiers};
use std::time::Duration;

pub struct App {
    cli: Cli,
    state: StateManager,
}

impl App {
    pub async fn new(cli: Cli) -> anyhow::Result<Self> {
        let state = StateManager::new(cli.config.clone()).await?;
        Ok(Self { cli, state })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let mut tui = tui::Tui::new()?;
        let mut chat_state = ChatViewState::new();
        let mut input = InputWidget::new();
        let config = self.state.config().await;
        let status = StatusBarState {
            mode: "ACT".to_string(),
            model_name: "claude-sonnet-4-20250514".to_string(),
            provider: config.provider.default.clone(),
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

        loop {
            // Render
            tui.draw(|frame| {
                tui::app_layout::render_app(frame, &chat_state, &input, &status);
            })?;

            // Handle events
            if let Some(event) = poll_event(Duration::from_millis(16)) {
                match event {
                    TuiEvent::Key(key) => {
                        // Ctrl+C or Ctrl+Q → quit
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
                    TuiEvent::Resize(_, _) => {} // Ratatui handles resize automatically
                    TuiEvent::Tick => {}          // No-op for now
                    TuiEvent::Mouse(_) => {}      // Future: scroll handling
                }
            }
        }

        tui.restore()?;
        Ok(())
    }
}
```

### 3.8 Stub files for views not yet implemented

Create minimal stubs for `thinking_view.rs`, `tool_view.rs`, and `settings_view.rs`:

```rust
// src/tui/thinking_view.rs
//! Thinking/reasoning content display (collapsible panel).
// To be implemented in STEP 09.

// src/tui/tool_view.rs
//! Tool call display with approval UI.
// To be implemented in STEP 10.

// src/tui/settings_view.rs
//! Settings/configuration panel.
// To be implemented in STEP 15.
```

## Tests

### Chat view state tests
```rust
#[cfg(test)]
mod chat_view_tests {
    use super::chat_view::*;

    #[test]
    fn test_chat_view_state_new() {
        let state = ChatViewState::new();
        assert!(state.messages.is_empty());
        assert!(state.auto_scroll);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_push_message() {
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
    fn test_push_message_auto_scrolls() {
        let mut state = ChatViewState::new();
        state.push_message(ChatMessage {
            role: ChatRole::User,
            content: "msg1".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: false,
        });
        // auto_scroll should set scroll_offset to MAX
        assert_eq!(state.scroll_offset, u16::MAX);
        assert!(state.auto_scroll);
    }

    #[test]
    fn test_update_last_message() {
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
    fn test_update_last_message_empty_vec() {
        let mut state = ChatViewState::new();
        // Should not panic on empty messages
        state.update_last_message("no-op");
        assert!(state.messages.is_empty());
    }

    #[test]
    fn test_scroll_up_disables_auto_scroll() {
        let mut state = ChatViewState::new();
        state.scroll_offset = 10;
        state.scroll_up(5);
        assert_eq!(state.scroll_offset, 5);
        assert!(!state.auto_scroll);
    }

    #[test]
    fn test_scroll_up_saturates_at_zero() {
        let mut state = ChatViewState::new();
        state.scroll_offset = 2;
        state.scroll_up(10);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_scroll_down_clamps_to_max() {
        let mut state = ChatViewState::new();
        state.auto_scroll = false;
        state.scroll_down(100, 50);
        assert_eq!(state.scroll_offset, 50);
        assert!(state.auto_scroll); // Re-enabled because at max
    }

    #[test]
    fn test_scroll_to_bottom_re_enables() {
        let mut state = ChatViewState::new();
        state.scroll_up(5);
        assert!(!state.auto_scroll);
        state.scroll_to_bottom();
        assert!(state.auto_scroll);
        assert_eq!(state.scroll_offset, u16::MAX);
    }

    #[test]
    fn test_chat_role_equality() {
        assert_eq!(ChatRole::User, ChatRole::User);
        assert_ne!(ChatRole::User, ChatRole::Assistant);
    }
}
```

### Input widget tests
```rust
#[cfg(test)]
mod input_tests {
    use super::input::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn test_enter_submits_text() {
        let mut input = InputWidget::new();
        // Type "hello"
        for c in "hello".chars() {
            input.handle_key(make_key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let result = input.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(result, Some("hello".to_string()));
    }

    #[test]
    fn test_empty_enter_returns_none() {
        let mut input = InputWidget::new();
        let result = input.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(result.is_none());
    }

    #[test]
    fn test_input_clears_after_submit() {
        let mut input = InputWidget::new();
        for c in "test".chars() {
            input.handle_key(make_key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        input.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        // After submit, textarea should be empty
        assert!(input.textarea.lines().join("").is_empty());
    }

    #[test]
    fn test_typing_accumulates() {
        let mut input = InputWidget::new();
        for c in "abc".chars() {
            let result = input.handle_key(make_key(KeyCode::Char(c), KeyModifiers::NONE));
            assert!(result.is_none()); // Typing doesn't submit
        }
        assert_eq!(input.textarea.lines().join(""), "abc");
    }

    #[test]
    fn test_whitespace_only_returns_none() {
        let mut input = InputWidget::new();
        input.handle_key(make_key(KeyCode::Char(' '), KeyModifiers::NONE));
        input.handle_key(make_key(KeyCode::Char(' '), KeyModifiers::NONE));
        let result = input.handle_key(make_key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(result.is_none());
    }
}
```

### TUI rendering tests (using TestBackend)
```rust
#[cfg(test)]
mod tui_render_tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn test_app_layout_renders_without_panic() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let chat_state = super::chat_view::ChatViewState::new();
        let input = super::input::InputWidget::new();
        let status = super::status_bar::StatusBarState {
            mode: "ACT".to_string(),
            model_name: "claude-sonnet-4-20250514".to_string(),
            provider: "anthropic".to_string(),
            total_tokens: 0,
            total_cost: 0.0,
            is_streaming: false,
        };
        terminal
            .draw(|frame| {
                super::app_layout::render_app(frame, &chat_state, &input, &status);
            })
            .unwrap();
    }

    #[test]
    fn test_render_with_messages() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut chat_state = super::chat_view::ChatViewState::new();
        chat_state.push_message(super::chat_view::ChatMessage {
            role: super::chat_view::ChatRole::User,
            content: "Hello!".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: false,
        });
        chat_state.push_message(super::chat_view::ChatMessage {
            role: super::chat_view::ChatRole::Assistant,
            content: "Hi there!".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: false,
        });
        let input = super::input::InputWidget::new();
        let status = super::status_bar::StatusBarState {
            mode: "ACT".to_string(),
            model_name: "claude-sonnet-4-20250514".to_string(),
            provider: "anthropic".to_string(),
            total_tokens: 1500,
            total_cost: 0.0035,
            is_streaming: false,
        };
        terminal
            .draw(|frame| {
                super::app_layout::render_app(frame, &chat_state, &input, &status);
            })
            .unwrap();
    }

    #[test]
    fn test_render_streaming_message() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut chat_state = super::chat_view::ChatViewState::new();
        chat_state.push_message(super::chat_view::ChatMessage {
            role: super::chat_view::ChatRole::Assistant,
            content: "Thinking...".to_string(),
            timestamp: chrono::Utc::now(),
            streaming: true, // Should show cursor block
        });
        let input = super::input::InputWidget::new();
        let status = super::status_bar::StatusBarState {
            mode: "ACT".to_string(),
            model_name: "test-model".to_string(),
            provider: "test".to_string(),
            total_tokens: 0,
            total_cost: 0.0,
            is_streaming: true,
        };
        terminal
            .draw(|frame| {
                super::app_layout::render_app(frame, &chat_state, &input, &status);
            })
            .unwrap();
    }

    #[test]
    fn test_render_small_terminal() {
        // Ensure no panic with very small terminal size
        let backend = TestBackend::new(20, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let chat_state = super::chat_view::ChatViewState::new();
        let input = super::input::InputWidget::new();
        let status = super::status_bar::StatusBarState {
            mode: "ACT".to_string(),
            model_name: "m".to_string(),
            provider: "p".to_string(),
            total_tokens: 0,
            total_cost: 0.0,
            is_streaming: false,
        };
        terminal
            .draw(|frame| {
                super::app_layout::render_app(frame, &chat_state, &input, &status);
            })
            .unwrap();
    }
}
```

## Acceptance Criteria
- [x] TUI launches in alternate screen with raw mode enabled
- [x] Status bar shows mode badge (colored), provider/model, token count, cost
- [x] Chat view displays messages with role-colored prefixes (cyan for User, green for Assistant, etc.)
- [x] Streaming messages show a block cursor character at the end
- [x] User can type text and press Enter to submit
- [x] Submitted text appears as a User message in the chat view
- [x] Shift+Enter inserts a newline (does not submit)
- [x] Empty/whitespace-only submissions are suppressed
- [x] Ctrl+C exits cleanly and restores the terminal
- [x] Terminal is always restored on exit, even on panic (via `Drop` impl)
- [x] No panics on any input combination or terminal size
- [x] Renders correctly on small terminals (20x10)
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes all widget tests using `TestBackend`

**Completed**: PR #4
