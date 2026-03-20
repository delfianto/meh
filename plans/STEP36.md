# STEP 36 — Graceful Cancellation

## Objective
Implement proper mid-stream cancellation when the user presses Ctrl+C. The stream must be aborted, partial messages cleaned up, and the TUI must remain responsive.

## Prerequisites
- STEP 07 (agent + streaming), STEP 05 (provider abort)

## Detailed Instructions

### 36.1 Signal handling

Install Ctrl+C handler that sends cancellation through channels rather than killing the process:

```rust
// In app.rs or main.rs
let ctrl_tx_signal = ctrl_tx.clone();
tokio::spawn(async move {
    tokio::signal::ctrl_c().await.ok();
    let _ = ctrl_tx_signal.send(ControllerMessage::CancelTask);
});
```

But ALSO handle Ctrl+C in the TUI event loop (crossterm captures it as a key event):
```rust
TuiEvent::Key(key) if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) => {
    if is_streaming {
        ctrl_tx.send(ControllerMessage::CancelTask)?;
        // Show "Cancelling..." in status bar
    } else {
        ctrl_tx.send(ControllerMessage::Quit)?;
    }
}
```

### 36.2 CancellationToken integration

Use `tokio_util::sync::CancellationToken` for cooperative cancellation:

```rust
use tokio_util::sync::CancellationToken;

pub struct TaskCancellation {
    token: CancellationToken,
    last_cancel: Option<std::time::Instant>,
}

impl TaskCancellation {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            last_cancel: None,
        }
    }

    /// Cancel the current task. Returns true if this is a double-cancel (force quit).
    pub fn cancel(&mut self) -> bool {
        let now = std::time::Instant::now();
        let is_double = self.last_cancel
            .map(|last| now.duration_since(last) < std::time::Duration::from_secs(2))
            .unwrap_or(false);
        self.last_cancel = Some(now);
        self.token.cancel();
        is_double
    }

    /// Get a clone of the token for passing to async tasks.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Reset for a new task.
    pub fn reset(&mut self) {
        self.token = CancellationToken::new();
        self.last_cancel = None;
    }

    /// Check if cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}
```

### 36.3 Controller cancellation flow

```rust
ControllerMessage::CancelTask => {
    let is_double = self.cancellation.cancel();
    if is_double {
        // Double Ctrl+C within 2 seconds — force quit
        tracing::info!("Double cancel detected, force quitting");
        self.ui_tx.send(UiUpdate::Quit)?;
        return Ok(());
    }

    // Single cancel — abort current task
    if let Some(agent_tx) = &self.agent_tx {
        let _ = agent_tx.send(AgentMessage::Cancel);
    }

    // Abort the provider stream
    if let Some(provider) = &self.active_provider {
        provider.abort();
    }

    // Update TUI
    self.ui_tx.send(UiUpdate::StreamEnd)?;
    self.ui_tx.send(UiUpdate::AppendMessage {
        role: ChatRole::System,
        content: "Task cancelled by user.".to_string(),
    })?;
    self.ui_tx.send(UiUpdate::StatusUpdate {
        is_streaming: Some(false),
        ..Default::default()
    })?;
}
```

### 36.4 Agent cancellation

The agent checks for cancellation in its select loop (already in STEP 07). On cancel:
1. `Provider.abort()` called (drops the HTTP connection via `CancellationToken`)
2. Partial assistant message preserved in history (marked incomplete)
3. Agent loop exits cleanly
4. No tool results sent for pending tool calls

```rust
// In the agent's streaming loop:
tokio::select! {
    chunk = stream.next() => {
        match chunk {
            Some(Ok(chunk)) => self.handle_chunk(chunk)?,
            Some(Err(e)) if self.cancellation.is_cancelled() => {
                // Expected — stream was aborted
                break;
            }
            Some(Err(e)) => return Err(e.into()),
            None => break,
        }
    }
    _ = self.cancellation.token().cancelled() => {
        tracing::info!("Agent cancelled by user");
        break;
    }
    msg = self.agent_rx.recv() => {
        match msg {
            Some(AgentMessage::Cancel) => {
                self.cancellation.cancel();
                break;
            }
            _ => {}
        }
    }
}
```

### 36.5 Partial message handling

When cancelled mid-stream, the partial text should still be visible:
```rust
if self.cancellation.is_cancelled() {
    // Save whatever text we have so far
    if !assistant_text.is_empty() {
        assistant_content_blocks.push(ContentBlock::Text(assistant_text));
    }
    // Mark the message as incomplete
    self.messages.push(Message {
        role: MessageRole::Assistant,
        content: assistant_content_blocks,
    });
    // Do NOT process any pending tool calls — they were never completed
    break;
}
```

### 36.6 Double Ctrl+C = force quit

If user presses Ctrl+C again within 2 seconds of the first:
```rust
// In TUI event handler:
TuiEvent::Key(key) if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) => {
    ctrl_tx.send(ControllerMessage::CancelTask)?;
    // The Controller's TaskCancellation.cancel() returns true on double-cancel,
    // which triggers UiUpdate::Quit
}
```

### 36.7 Provider abort mechanism

Each provider wraps its HTTP client with a CancellationToken:
```rust
impl Provider for AnthropicProvider {
    fn abort(&self) {
        self.cancel_token.cancel();
    }
}

// In the streaming method:
async fn stream(&self, ...) -> Result<impl Stream<...>> {
    let token = self.cancel_token.clone();
    // ... set up reqwest stream ...
    let stream = async_stream::stream! {
        loop {
            tokio::select! {
                chunk = response.chunk() => { /* yield chunk */ }
                _ = token.cancelled() => {
                    tracing::debug!("HTTP stream aborted by cancellation");
                    break;
                }
            }
        }
    };
    Ok(stream)
}
```

## Tests

```rust
#[cfg(test)]
mod cancellation_tests {
    use super::*;

    #[test]
    fn test_single_cancel() {
        let mut tc = TaskCancellation::new();
        let is_double = tc.cancel();
        assert!(!is_double);
        assert!(tc.is_cancelled());
    }

    #[test]
    fn test_double_cancel_within_2s() {
        let mut tc = TaskCancellation::new();
        let _ = tc.cancel();
        let is_double = tc.cancel(); // Immediate second cancel
        assert!(is_double);
    }

    #[test]
    fn test_reset() {
        let mut tc = TaskCancellation::new();
        tc.cancel();
        assert!(tc.is_cancelled());
        tc.reset();
        assert!(!tc.is_cancelled());
    }

    #[test]
    fn test_token_propagation() {
        let mut tc = TaskCancellation::new();
        let token = tc.token();
        assert!(!token.is_cancelled());
        tc.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn test_reset_creates_new_token() {
        let mut tc = TaskCancellation::new();
        let old_token = tc.token();
        tc.cancel();
        tc.reset();
        let new_token = tc.token();
        // Old token still cancelled
        assert!(old_token.is_cancelled());
        // New token is fresh
        assert!(!new_token.is_cancelled());
    }

    #[tokio::test]
    async fn test_cancellation_token_in_select() {
        let mut tc = TaskCancellation::new();
        let token = tc.token();

        // Spawn a task that waits for cancellation
        let handle = tokio::spawn(async move {
            token.cancelled().await;
            true
        });

        // Cancel after a brief moment
        tc.cancel();
        let result = handle.await.unwrap();
        assert!(result);
    }
}
```

## Acceptance Criteria
- [ ] Ctrl+C during streaming cancels the current API call
- [ ] Provider abort drops the HTTP connection via CancellationToken
- [ ] Partial assistant text preserved in chat history
- [ ] TUI shows "Task cancelled by user" message
- [ ] Status bar streaming indicator cleared
- [ ] Double Ctrl+C within 2s force-quits the application
- [ ] Ctrl+C when idle quits the app
- [ ] Pending tool calls discarded on cancellation
- [ ] CancellationToken reset between tasks
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass
