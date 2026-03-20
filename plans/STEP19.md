# STEP 19 — Thinking View (Collapsible, Toggleable)

## Objective
Implement a dedicated thinking/chain-of-thought view in the TUI that shows the LLM's internal reasoning. Thinking blocks are collapsible and can be globally toggled on/off with Ctrl+T.

## Prerequisites
- STEP 03 complete (TUI)
- STEP 06 complete (thinking parser)

## Detailed Instructions

### 19.1 Thinking View Widget (`src/tui/thinking_view.rs`)

```rust
//! Collapsible thinking/reasoning display.

use ratatui::prelude::*;
use ratatui::widgets::*;

/// State for a single thinking block.
#[derive(Debug, Clone)]
pub struct ThinkingBlock {
    pub content: String,
    pub collapsed: bool,
    pub redacted: bool,
    /// Whether this block is still actively streaming.
    pub streaming: bool,
}

/// State for the thinking view (manages multiple thinking blocks).
pub struct ThinkingViewState {
    /// All thinking blocks in the current conversation.
    pub blocks: Vec<ThinkingBlock>,
    /// Whether thinking is globally visible (toggled by Ctrl+T).
    pub visible: bool,
    /// Whether to show thinking blocks by default during streaming.
    pub show_during_streaming: bool,
}

impl ThinkingViewState {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            visible: true,
            show_during_streaming: true,
        }
    }

    /// Start a new thinking block (called when streaming thinking begins).
    pub fn start_block(&mut self) {
        self.blocks.push(ThinkingBlock {
            content: String::new(),
            collapsed: false,
            redacted: false,
            streaming: true,
        });
    }

    /// Append content to the current (last) thinking block.
    pub fn append(&mut self, delta: &str) {
        if let Some(block) = self.blocks.last_mut() {
            block.content.push_str(delta);
        }
    }

    /// Finalize the current thinking block.
    pub fn finalize_current(&mut self) {
        if let Some(block) = self.blocks.last_mut() {
            block.streaming = false;
            // Auto-collapse after streaming ends (save screen space)
            block.collapsed = true;
        }
    }

    /// Toggle global visibility.
    pub fn toggle_visibility(&mut self) {
        self.visible = !self.visible;
    }

    /// Toggle collapse on a specific block.
    pub fn toggle_block(&mut self, index: usize) {
        if let Some(block) = self.blocks.get_mut(index) {
            block.collapsed = !block.collapsed;
        }
    }

    /// Collapse all blocks.
    pub fn collapse_all(&mut self) {
        for block in &mut self.blocks {
            block.collapsed = true;
        }
    }

    /// Clear all blocks (for new task).
    pub fn clear(&mut self) {
        self.blocks.clear();
    }
}
```

### 19.2 Render thinking blocks inline in chat view

Thinking blocks are rendered inline in the chat view between assistant messages. They appear as dimmed, indented text with a toggle indicator:

```
 Assistant: Let me analyze this...

 ▶ Thinking (245 tokens) [click to expand]

 Or when expanded:

 ▼ Thinking (245 tokens)
 │ First, I need to understand the file structure.
 │ The main entry point is in src/main.rs which calls...
 │ I should look at the error handling in...

 Assistant: Based on my analysis, here's what I found...
```

Rendering logic:
```rust
pub fn render_thinking_block(
    block: &ThinkingBlock,
    area: Rect,
    buf: &mut Buffer,
    visible: bool,
) -> u16 { // Returns number of lines consumed
    if !visible { return 0; }

    let style = Style::default().fg(Color::DarkGray).italic();

    if block.collapsed {
        // Single line: "▶ Thinking (N tokens)"
        let token_est = block.content.split_whitespace().count(); // rough estimate
        let line = format!("  ▶ Thinking (~{token_est} tokens)");
        // Render with style
        return 1;
    }

    // Expanded: header + content lines
    let header = if block.streaming {
        "  ▼ Thinking... (streaming)"
    } else {
        "  ▼ Thinking"
    };
    // Render header
    // Render each content line prefixed with "  │ "
    // Return total lines
}
```

### 19.3 Integration with chat view

Update `ChatViewState` to include thinking blocks interleaved with messages:

```rust
/// Entry in the chat display (message or thinking block).
#[derive(Debug, Clone)]
pub enum ChatEntry {
    Message(ChatMessage),
    Thinking(usize), // Index into ThinkingViewState.blocks
}
```

Or simpler: embed thinking state directly in `ChatMessage`:
```rust
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub streaming: bool,
    /// Optional thinking block that preceded this message.
    pub thinking: Option<ThinkingBlock>,
}
```

### 19.4 Key bindings

- `Ctrl+T`: Toggle global thinking visibility
- When thinking is visible and a thinking block is at the current scroll position, `Enter` toggles that block's collapse state

### 19.5 Handle UiUpdate::ThinkingContent

Update TUI event loop:
```rust
UiUpdate::ThinkingContent { delta } => {
    if thinking_state.blocks.is_empty() || !thinking_state.blocks.last().map(|b| b.streaming).unwrap_or(false) {
        thinking_state.start_block();
    }
    thinking_state.append(&delta);
}
```

And handle thinking finalization when `UiUpdate::StreamEnd` or `UiUpdate::AppendMessage` arrives.

## Tests

```rust
#[cfg(test)]
mod thinking_view_tests {
    use super::*;

    #[test]
    fn test_thinking_state_new() {
        let state = ThinkingViewState::new();
        assert!(state.visible);
        assert!(state.blocks.is_empty());
    }

    #[test]
    fn test_start_and_append() {
        let mut state = ThinkingViewState::new();
        state.start_block();
        state.append("Let me think...");
        state.append(" More thinking.");
        assert_eq!(state.blocks.len(), 1);
        assert_eq!(state.blocks[0].content, "Let me think... More thinking.");
        assert!(state.blocks[0].streaming);
    }

    #[test]
    fn test_finalize_collapses() {
        let mut state = ThinkingViewState::new();
        state.start_block();
        state.append("thinking");
        state.finalize_current();
        assert!(!state.blocks[0].streaming);
        assert!(state.blocks[0].collapsed);
    }

    #[test]
    fn test_toggle_visibility() {
        let mut state = ThinkingViewState::new();
        assert!(state.visible);
        state.toggle_visibility();
        assert!(!state.visible);
        state.toggle_visibility();
        assert!(state.visible);
    }

    #[test]
    fn test_toggle_block() {
        let mut state = ThinkingViewState::new();
        state.start_block();
        assert!(!state.blocks[0].collapsed);
        state.toggle_block(0);
        assert!(state.blocks[0].collapsed);
    }

    #[test]
    fn test_collapse_all() {
        let mut state = ThinkingViewState::new();
        state.start_block();
        state.start_block();
        state.collapse_all();
        assert!(state.blocks.iter().all(|b| b.collapsed));
    }

    #[test]
    fn test_clear() {
        let mut state = ThinkingViewState::new();
        state.start_block();
        state.append("data");
        state.clear();
        assert!(state.blocks.is_empty());
    }

    #[test]
    fn test_multiple_blocks() {
        let mut state = ThinkingViewState::new();
        state.start_block();
        state.append("block 1");
        state.finalize_current();
        state.start_block();
        state.append("block 2");
        assert_eq!(state.blocks.len(), 2);
        assert!(!state.blocks[1].collapsed); // Not finalized yet
    }

    // Rendering test with TestBackend
    #[test]
    fn test_render_collapsed_block() {
        let block = ThinkingBlock {
            content: "Some long thinking content here".to_string(),
            collapsed: true,
            redacted: false,
            streaming: false,
        };
        // Use ratatui TestBackend to verify rendering
        let backend = ratatui::backend::TestBackend::new(80, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| {
            let area = frame.area();
            // render_thinking_block would be called here
            // Verify it produces the collapsed format
        }).unwrap();
    }
}
```

## Acceptance Criteria
- [ ] Thinking blocks displayed inline in chat with dimmed italic style
- [ ] Blocks auto-collapse after streaming ends
- [ ] Ctrl+T toggles all thinking visibility globally
- [ ] Individual blocks can be toggled collapsed/expanded
- [ ] Streaming thinking shows live content with "..." indicator
- [ ] Redacted thinking shows placeholder text
- [ ] Token count estimate shown in collapsed header
- [ ] Works correctly with scrolling
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass
