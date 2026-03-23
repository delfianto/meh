//! Context window management — tracks token budget and triggers summarization.
//!
//! LLMs have finite context windows. As conversations grow, the message
//! history must be managed to stay within budget. This module provides
//! two strategies that work together:
//!
//! ```text
//!   Messages[]  ──►  ContextManager  ──►  Messages[] (trimmed)
//!                         │
//!                    ┌────┴────┐
//!                    │         │
//!                truncation  summarizer
//!                    │         │
//!                 drop old   LLM call to
//!                 messages   condense history
//! ```
//!
//! **Truncation** drops the oldest messages (preserving the system prompt
//! and the most recent turns) when the context approaches the model's
//! token limit.
//!
//! **Summarization** uses a separate LLM call to condense the conversation
//! history into a compact summary, replacing many messages with a single
//! summary message. This preserves more semantic content than raw
//! truncation at the cost of an extra API call.

pub mod summarizer;
pub mod truncation;

/// Tracks token budget and determines when summarization/truncation is needed.
pub struct ContextManager {
    /// Model's total context window (tokens).
    context_window: u32,
    /// Reserve tokens for the model's response.
    response_reserve: u32,
    /// Reserve tokens for system prompt + tool definitions.
    system_reserve: u32,
    /// Threshold percentage to trigger summarization (e.g., 0.85 = 85%).
    summarize_threshold: f64,
    /// Current estimated token usage.
    current_usage: u64,
    /// Range of messages that have been deleted/summarized.
    deleted_range: Option<(usize, usize)>,
}

impl ContextManager {
    /// Create a new context manager from model info.
    pub const fn new(context_window: u32, max_tokens: u32) -> Self {
        Self {
            context_window,
            response_reserve: max_tokens,
            system_reserve: 0,
            summarize_threshold: 0.85,
            current_usage: 0,
            deleted_range: None,
        }
    }

    /// Set the system prompt token count (call once after building prompt).
    pub const fn set_system_reserve(&mut self, system_tokens: u32) {
        self.system_reserve = system_tokens;
    }

    /// Available tokens for conversation messages.
    #[allow(clippy::cast_lossless)]
    pub const fn available_tokens(&self) -> u64 {
        let total = self.context_window as u64;
        let reserved = (self.response_reserve + self.system_reserve) as u64;
        total.saturating_sub(reserved)
    }

    /// Update current usage estimate.
    pub const fn update_usage(&mut self, tokens: u64) {
        self.current_usage = tokens;
    }

    /// Check if summarization should be triggered.
    #[allow(clippy::cast_precision_loss)]
    pub fn needs_summarization(&self) -> bool {
        let available = self.available_tokens();
        if available == 0 {
            return true;
        }
        (self.current_usage as f64 / available as f64) >= self.summarize_threshold
    }

    /// Calculate how many tokens to free (target: reduce to 60% of available).
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    pub fn tokens_to_free(&self) -> u64 {
        let target = (self.available_tokens() as f64 * 0.60) as u64;
        self.current_usage.saturating_sub(target)
    }

    /// Record that messages in range `[start, end)` were deleted.
    pub const fn record_deletion(&mut self, start: usize, end: usize) {
        self.deleted_range = Some((start, end));
    }

    /// Get the deleted range for persistence.
    pub const fn deleted_range(&self) -> Option<(usize, usize)> {
        self.deleted_range
    }

    /// Context utilization as percentage.
    #[allow(clippy::cast_precision_loss)]
    pub fn utilization_percent(&self) -> f64 {
        let available = self.available_tokens();
        if available == 0 {
            return 100.0;
        }
        (self.current_usage as f64 / available as f64) * 100.0
    }

    /// Current estimated token usage.
    pub const fn current_usage(&self) -> u64 {
        self.current_usage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_tokens_calculation() {
        let mut cm = ContextManager::new(200_000, 8192);
        cm.set_system_reserve(5000);
        assert_eq!(cm.available_tokens(), 200_000 - 8192 - 5000);
    }

    #[test]
    fn available_tokens_zero_reserves() {
        let cm = ContextManager::new(100_000, 0);
        assert_eq!(cm.available_tokens(), 100_000);
    }

    #[test]
    fn available_tokens_saturates() {
        let mut cm = ContextManager::new(1000, 800);
        cm.set_system_reserve(500);
        assert_eq!(cm.available_tokens(), 0);
    }

    #[test]
    fn needs_summarization_under_threshold() {
        let mut cm = ContextManager::new(200_000, 8192);
        cm.update_usage(50_000);
        assert!(!cm.needs_summarization());
    }

    #[test]
    fn needs_summarization_over_threshold() {
        let mut cm = ContextManager::new(200_000, 8192);
        cm.update_usage(170_000);
        assert!(cm.needs_summarization());
    }

    #[test]
    fn needs_summarization_zero_available() {
        let mut cm = ContextManager::new(1000, 1000);
        cm.update_usage(100);
        assert!(cm.needs_summarization());
    }

    #[test]
    fn tokens_to_free_positive() {
        let mut cm = ContextManager::new(100_000, 5000);
        cm.update_usage(80_000);
        let to_free = cm.tokens_to_free();
        assert!(to_free > 0);
    }

    #[test]
    fn tokens_to_free_zero_when_under_target() {
        let mut cm = ContextManager::new(100_000, 5000);
        cm.update_usage(10_000);
        assert_eq!(cm.tokens_to_free(), 0);
    }

    #[test]
    fn utilization_percent_half() {
        let mut cm = ContextManager::new(100_000, 0);
        cm.update_usage(50_000);
        assert!((cm.utilization_percent() - 50.0).abs() < 0.1);
    }

    #[test]
    fn utilization_percent_zero_available() {
        let mut cm = ContextManager::new(100, 100);
        cm.update_usage(50);
        assert!((cm.utilization_percent() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn record_and_get_deletion() {
        let mut cm = ContextManager::new(100_000, 0);
        assert!(cm.deleted_range().is_none());
        cm.record_deletion(0, 5);
        assert_eq!(cm.deleted_range(), Some((0, 5)));
    }

    #[test]
    fn current_usage_accessor() {
        let mut cm = ContextManager::new(100_000, 0);
        assert_eq!(cm.current_usage(), 0);
        cm.update_usage(42_000);
        assert_eq!(cm.current_usage(), 42_000);
    }
}
