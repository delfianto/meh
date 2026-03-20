//! Collapsible thinking/reasoning display.
//!
//! Thinking blocks show the LLM's internal chain-of-thought reasoning
//! inline in the chat view. They are rendered with dimmed italic styling
//! and can be collapsed/expanded individually or toggled globally with Ctrl+T.
//!
//! ```text
//!   ▶ Thinking (~45 tokens)          [collapsed]
//!
//!   ▼ Thinking                        [expanded]
//!   │ First, I need to understand...
//!   │ The main entry point is in...
//!   │ I should look at the error...
//! ```

/// State for a single thinking block.
#[derive(Debug, Clone)]
pub struct ThinkingBlock {
    /// The thinking content text.
    pub content: String,
    /// Whether this block is collapsed (showing only header).
    pub collapsed: bool,
    /// Whether this block's content was redacted by the API.
    pub redacted: bool,
    /// Whether this block is still actively streaming.
    pub streaming: bool,
}

impl ThinkingBlock {
    /// Estimate token count based on whitespace-separated words.
    pub fn estimated_tokens(&self) -> usize {
        self.content.split_whitespace().count()
    }

    /// Get the collapsed header text for display.
    pub fn collapsed_header(&self) -> String {
        if self.redacted {
            "\u{25b6} Thinking (redacted)".to_string()
        } else if self.streaming {
            "\u{25bc} Thinking... (streaming)".to_string()
        } else {
            format!("\u{25b6} Thinking (~{} tokens)", self.estimated_tokens())
        }
    }

    /// Get the expanded header text for display.
    pub fn expanded_header(&self) -> String {
        if self.streaming {
            "\u{25bc} Thinking... (streaming)".to_string()
        } else {
            "\u{25bc} Thinking".to_string()
        }
    }
}

/// State for the thinking view (manages multiple thinking blocks).
#[derive(Debug)]
pub struct ThinkingViewState {
    /// All thinking blocks in the current conversation.
    pub blocks: Vec<ThinkingBlock>,
    /// Whether thinking is globally visible (toggled by Ctrl+T).
    pub visible: bool,
    /// Whether to show thinking blocks by default during streaming.
    pub show_during_streaming: bool,
}

impl ThinkingViewState {
    /// Create a new thinking view state with default settings.
    pub const fn new() -> Self {
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

    /// Mark the current thinking block as redacted.
    pub fn mark_redacted(&mut self) {
        if let Some(block) = self.blocks.last_mut() {
            block.redacted = true;
            block.content = "(thinking content redacted by API)".to_string();
        }
    }

    /// Finalize the current thinking block (auto-collapses).
    pub fn finalize_current(&mut self) {
        if let Some(block) = self.blocks.last_mut() {
            block.streaming = false;
            block.collapsed = true;
        }
    }

    /// Toggle global visibility.
    pub const fn toggle_visibility(&mut self) {
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

    /// Expand all blocks.
    pub fn expand_all(&mut self) {
        for block in &mut self.blocks {
            block.collapsed = false;
        }
    }

    /// Clear all blocks (for new task).
    pub fn clear(&mut self) {
        self.blocks.clear();
    }

    /// Check if there is an active (streaming) thinking block.
    pub fn has_active_block(&self) -> bool {
        self.blocks.last().is_some_and(|b| b.streaming)
    }

    /// Get the total number of estimated tokens across all blocks.
    pub fn total_estimated_tokens(&self) -> usize {
        self.blocks
            .iter()
            .map(ThinkingBlock::estimated_tokens)
            .sum()
    }

    /// Calculate the number of display lines needed for all visible blocks.
    pub fn display_line_count(&self, max_width: usize) -> usize {
        if !self.visible {
            return 0;
        }

        self.blocks
            .iter()
            .map(|block| {
                if block.collapsed {
                    1
                } else {
                    let content_lines = if max_width == 0 {
                        block.content.lines().count()
                    } else {
                        block
                            .content
                            .lines()
                            .map(|line| (line.len() / max_width) + 1)
                            .sum()
                    };
                    1 + content_lines.max(1)
                }
            })
            .sum()
    }
}

impl Default for ThinkingViewState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
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
        state.toggle_block(0);
        assert!(!state.blocks[0].collapsed);
    }

    #[test]
    fn test_toggle_block_out_of_bounds() {
        let mut state = ThinkingViewState::new();
        state.toggle_block(99);
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
    fn test_expand_all() {
        let mut state = ThinkingViewState::new();
        state.start_block();
        state.finalize_current();
        state.start_block();
        state.finalize_current();
        assert!(state.blocks.iter().all(|b| b.collapsed));
        state.expand_all();
        assert!(state.blocks.iter().all(|b| !b.collapsed));
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
        assert!(!state.blocks[1].collapsed);
    }

    #[test]
    fn test_has_active_block() {
        let mut state = ThinkingViewState::new();
        assert!(!state.has_active_block());
        state.start_block();
        assert!(state.has_active_block());
        state.finalize_current();
        assert!(!state.has_active_block());
    }

    #[test]
    fn test_estimated_tokens() {
        let block = ThinkingBlock {
            content: "one two three four five".to_string(),
            collapsed: false,
            redacted: false,
            streaming: false,
        };
        assert_eq!(block.estimated_tokens(), 5);
    }

    #[test]
    fn test_total_estimated_tokens() {
        let mut state = ThinkingViewState::new();
        state.start_block();
        state.append("one two three");
        state.finalize_current();
        state.start_block();
        state.append("four five");
        assert_eq!(state.total_estimated_tokens(), 5);
    }

    #[test]
    fn test_collapsed_header() {
        let block = ThinkingBlock {
            content: "word1 word2 word3".to_string(),
            collapsed: true,
            redacted: false,
            streaming: false,
        };
        let header = block.collapsed_header();
        assert!(header.contains("3 tokens"));
        assert!(header.contains('\u{25b6}'));
    }

    #[test]
    fn test_collapsed_header_redacted() {
        let block = ThinkingBlock {
            content: String::new(),
            collapsed: true,
            redacted: true,
            streaming: false,
        };
        assert!(block.collapsed_header().contains("redacted"));
    }

    #[test]
    fn test_expanded_header_streaming() {
        let block = ThinkingBlock {
            content: String::new(),
            collapsed: false,
            redacted: false,
            streaming: true,
        };
        assert!(block.expanded_header().contains("streaming"));
    }

    #[test]
    fn test_mark_redacted() {
        let mut state = ThinkingViewState::new();
        state.start_block();
        state.append("secret thinking");
        state.mark_redacted();
        assert!(state.blocks[0].redacted);
        assert!(state.blocks[0].content.contains("redacted"));
    }

    #[test]
    fn test_display_line_count_hidden() {
        let mut state = ThinkingViewState::new();
        state.start_block();
        state.append("some content");
        state.visible = false;
        assert_eq!(state.display_line_count(80), 0);
    }

    #[test]
    fn test_display_line_count_collapsed() {
        let mut state = ThinkingViewState::new();
        state.start_block();
        state.append("some content");
        state.finalize_current();
        assert_eq!(state.display_line_count(80), 1);
    }

    #[test]
    fn test_display_line_count_expanded() {
        let mut state = ThinkingViewState::new();
        state.start_block();
        state.append("line one\nline two\nline three");
        assert!(state.display_line_count(80) >= 4);
    }

    #[test]
    fn test_default_impl() {
        let state = ThinkingViewState::default();
        assert!(state.visible);
        assert!(state.blocks.is_empty());
    }
}
