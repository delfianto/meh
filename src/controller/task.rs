//! Task lifecycle management — cancellation, double-cancel detection.
//!
//! `TaskCancellation` wraps a `CancellationToken` with double-cancel
//! detection: the first Ctrl+C cancels the current task, a second
//! within 2 seconds force-quits the application.

use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Manages cooperative cancellation with double-cancel detection.
pub struct TaskCancellation {
    token: CancellationToken,
    last_cancel: Option<Instant>,
}

impl TaskCancellation {
    /// Create a new cancellation tracker.
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            last_cancel: None,
        }
    }

    /// Cancel the current task. Returns `true` if this is a double-cancel
    /// (two cancels within 2 seconds), indicating a force-quit.
    pub fn cancel(&mut self) -> bool {
        let now = Instant::now();
        let is_double = self
            .last_cancel
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(2));
        self.last_cancel = Some(now);
        self.token.cancel();
        is_double
    }

    /// Get a clone of the token for passing to async tasks.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Reset for a new task — creates a fresh token and clears cancel history.
    pub fn reset(&mut self) {
        self.token = CancellationToken::new();
        self.last_cancel = None;
    }

    /// Check if currently cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_cancel() {
        let mut tc = TaskCancellation::new();
        let is_double = tc.cancel();
        assert!(!is_double);
        assert!(tc.is_cancelled());
    }

    #[test]
    fn double_cancel_within_2s() {
        let mut tc = TaskCancellation::new();
        let _ = tc.cancel();
        let is_double = tc.cancel();
        assert!(is_double);
    }

    #[test]
    fn reset_clears_state() {
        let mut tc = TaskCancellation::new();
        tc.cancel();
        assert!(tc.is_cancelled());
        tc.reset();
        assert!(!tc.is_cancelled());
    }

    #[test]
    fn token_propagation() {
        let mut tc = TaskCancellation::new();
        let token = tc.token();
        assert!(!token.is_cancelled());
        tc.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn reset_creates_new_token() {
        let mut tc = TaskCancellation::new();
        let old_token = tc.token();
        tc.cancel();
        tc.reset();
        let new_token = tc.token();
        assert!(old_token.is_cancelled());
        assert!(!new_token.is_cancelled());
    }

    #[tokio::test]
    async fn cancellation_token_in_select() {
        let mut tc = TaskCancellation::new();
        let token = tc.token();

        let handle = tokio::spawn(async move {
            token.cancelled().await;
            true
        });

        tc.cancel();
        let result = handle.await.unwrap();
        assert!(result);
    }

    #[test]
    fn cancel_after_reset_is_not_double() {
        let mut tc = TaskCancellation::new();
        tc.cancel();
        tc.reset();
        let is_double = tc.cancel();
        assert!(!is_double);
    }
}
