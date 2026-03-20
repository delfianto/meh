//! Accumulates reasoning/thinking blocks from streaming chunks.
//!
//! Lifecycle: `append()` for each delta → `set_signature()` when
//! signature arrives → `finalize()` when the block ends → `reset()`
//! to prepare for the next block.

/// Tracks accumulated thinking content during streaming.
#[derive(Debug, Default)]
pub struct ThinkingAccumulator {
    content: String,
    signature: Option<String>,
    is_redacted: bool,
    is_active: bool,
}

impl ThinkingAccumulator {
    /// Creates a new empty accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a thinking delta.
    pub fn append(&mut self, delta: &str) {
        self.is_active = true;
        self.content.push_str(delta);
    }

    /// Sets the signature (used for multi-turn thinking verification).
    pub fn set_signature(&mut self, sig: String) {
        self.signature = Some(sig);
    }

    /// Marks as redacted (the API redacted the reasoning content).
    pub const fn set_redacted(&mut self) {
        self.is_redacted = true;
    }

    /// Returns the current accumulated content.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Returns whether thinking is currently being streamed.
    pub const fn is_active(&self) -> bool {
        self.is_active
    }

    /// Returns the current length of accumulated content.
    pub fn len(&self) -> usize {
        self.content.len()
    }

    /// Returns whether no content has been accumulated.
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Finalizes and returns the complete thinking block.
    pub fn finalize(&mut self) -> ThinkingBlock {
        self.is_active = false;
        ThinkingBlock {
            content: std::mem::take(&mut self.content),
            signature: self.signature.take(),
            redacted: self.is_redacted,
        }
    }

    /// Resets all state for the next thinking block.
    pub fn reset(&mut self) {
        self.content.clear();
        self.signature = None;
        self.is_redacted = false;
        self.is_active = false;
    }
}

/// A complete thinking block (finalized accumulator output).
#[derive(Debug, Clone)]
pub struct ThinkingBlock {
    pub content: String,
    pub signature: Option<String>,
    pub redacted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_new() {
        let acc = ThinkingAccumulator::new();
        assert!(!acc.is_active());
        assert!(acc.is_empty());
        assert_eq!(acc.len(), 0);
    }

    #[test]
    fn thinking_accumulation() {
        let mut acc = ThinkingAccumulator::new();
        assert!(!acc.is_active());
        acc.append("Let me ");
        assert!(acc.is_active());
        assert!(!acc.is_empty());
        acc.append("think about this.");
        assert_eq!(acc.content(), "Let me think about this.");
        assert_eq!(acc.len(), 24);
    }

    #[test]
    fn thinking_finalize() {
        let mut acc = ThinkingAccumulator::new();
        acc.append("Reasoning here.");
        acc.set_signature("sig123".to_string());
        let block = acc.finalize();
        assert_eq!(block.content, "Reasoning here.");
        assert_eq!(block.signature, Some("sig123".to_string()));
        assert!(!block.redacted);
        assert!(!acc.is_active());
        assert!(acc.content().is_empty());
    }

    #[test]
    fn thinking_redacted() {
        let mut acc = ThinkingAccumulator::new();
        acc.set_redacted();
        let block = acc.finalize();
        assert!(block.redacted);
        assert!(block.content.is_empty());
    }

    #[test]
    fn thinking_reset() {
        let mut acc = ThinkingAccumulator::new();
        acc.append("data");
        acc.set_signature("sig".to_string());
        acc.reset();
        assert!(!acc.is_active());
        assert!(acc.content().is_empty());
        assert!(acc.is_empty());
    }

    #[test]
    fn thinking_multiple_cycles() {
        let mut acc = ThinkingAccumulator::new();

        acc.append("First thought.");
        let block1 = acc.finalize();
        assert_eq!(block1.content, "First thought.");

        acc.reset();
        assert!(!acc.is_active());

        acc.append("Second thought.");
        let block2 = acc.finalize();
        assert_eq!(block2.content, "Second thought.");
    }

    #[test]
    fn thinking_finalize_empty() {
        let mut acc = ThinkingAccumulator::new();
        let block = acc.finalize();
        assert!(block.content.is_empty());
        assert!(block.signature.is_none());
        assert!(!block.redacted);
    }
}
