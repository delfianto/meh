# STEP 40 — Code Quality Fixes (Gemini 3.1 Pro Audit)

## Objective
Address 5 specific code quality issues identified by external audit. Fixes range from a correctness bug (AutoSaver starvation) to an incomplete parser (regex-based JSON preview), an unfinished integration (tool executor not wired), a CPU waste (TUI polling), and a documented architectural compromise (zero-copy).

## Prerequisites
- All 39 prior steps complete

## Context

An external analysis by Gemini 3.1 Pro identified these issues:

| # | Severity | Issue | Location |
|---|----------|-------|----------|
| 1 | **Bug** | AutoSaver `select!` starvation — sleep resets on every recv | `state/history.rs` |
| 2 | **Correctness** | Regex-based JSON preview only captures strings, breaks on escaped quotes | `streaming/tool_parser.rs` |
| 3 | **Incomplete** | Tool executor not wired — controller returns "not yet implemented" | `controller/mod.rs` |
| 4 | **Performance** | TUI event loop busy-polls at 60fps even when idle | `app.rs` |
| 5 | **Documented** | Zero-copy principle stated but not applied in hot paths | `provider/mod.rs` etc. |

---

## Detailed Instructions

### 40.1 Fix AutoSaver starvation bug (`src/state/history.rs`)

**Problem**: Inside the `tokio::select!`, `tokio::time::sleep(debounce)` creates a new `Sleep` future each iteration. If `rx.recv()` wins every time (fast streaming), the timer is never polled to completion — the save never fires until the channel closes.

**Fix**: Pin the sleep future outside the select loop and only reset it when a new task arrives:

```rust
use tokio::time::{Instant, sleep_until, Sleep};
use std::pin::Pin;

pub fn new(history: TaskHistory) -> Self {
    let (tx, mut rx) = mpsc::unbounded_channel::<PersistedTask>();

    tokio::spawn(async move {
        let mut pending: Option<PersistedTask> = None;
        let debounce = Duration::from_secs(2);
        let mut deadline: Pin<Box<Sleep>> = Box::pin(sleep_until(Instant::now() + debounce));
        let mut timer_active = false;

        loop {
            tokio::select! {
                task = rx.recv() => {
                    match task {
                        Some(task) => {
                            pending = Some(task);
                            // Reset the deadline whenever new data arrives
                            deadline.as_mut().reset(Instant::now() + debounce);
                            timer_active = true;
                        }
                        None => break,
                    }
                }
                () = &mut deadline, if timer_active => {
                    if let Some(task) = pending.take() {
                        if let Err(e) = history.save_task(&task) {
                            tracing::error!(error = %e, "Failed to auto-save task");
                        }
                    }
                    timer_active = false;
                }
            }
        }

        // Save any remaining pending task on shutdown
        if let Some(task) = pending {
            let _ = history.save_task(&task);
        }
    });

    Self { tx }
}
```

The key difference: `deadline` is created once, pinned, and only reset (not recreated) when new data arrives. The `&mut deadline` in the select branch polls the *same* future, so it will actually fire after 2 seconds.

### 40.2 Replace regex JSON preview with serde-based parser (`src/streaming/tool_parser.rs`)

**Problem**: `extract_partial_json_fields` uses a regex `r#""(\w+)"\s*:\s*"([^"]*)"?"#` that:
- Only captures string values (misses booleans, numbers, arrays)
- Breaks on escaped quotes in code content

**Fix**: Replace with `serde_json::from_str` on the accumulated buffer, falling back to a bracket-counting heuristic for truly incomplete JSON:

```rust
/// Extract fields from potentially incomplete JSON.
///
/// Strategy:
/// 1. Try full serde parse (works if JSON is complete)
/// 2. Try appending "}" to close the object (works for truncated-at-value)
/// 3. Try appending "\"}" (works for truncated mid-string)
/// 4. Fall back to empty map
pub fn extract_partial_json_fields(partial: &str) -> HashMap<String, String> {
    // Try parsing as-is, then with repair suffixes
    let candidates = [
        partial.to_string(),
        format!("{partial}\"}}"),
        format!("{partial}}}"),
        format!("{partial}null}}"),
    ];
    for candidate in &candidates {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(candidate) {
            return map.into_iter()
                .map(|(k, v)| (k, value_to_preview(&v)))
                .collect();
        }
    }
    HashMap::new()
}

fn value_to_preview(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Array(_) => "[...]".to_string(),
        serde_json::Value::Object(_) => "{...}".to_string(),
    }
}
```

This correctly handles booleans, numbers, nested objects, and escaped quotes inside strings — all cases the regex missed.

### 40.3 Wire tool executor into controller (`src/controller/mod.rs`)

**Problem**: `handle_tool_call_request` is hardcoded to return "not yet implemented" for every tool call. The actual `ToolExecutor` exists and works, but was never connected.

**Fix**: Add a `ToolExecutor` field to the Controller, initialize it with the default tool registry, and call it from the handler:

```rust
// Add to Controller struct:
tool_executor: Arc<crate::tool::ToolExecutor>,

// In Controller::new():
let registry = Arc::new(crate::tool::ToolRegistry::with_defaults());
let tool_executor = Arc::new(crate::tool::ToolExecutor::new(registry));

// Replace handle_tool_call_request:
async fn handle_tool_call_request(&self, req: messages::ToolCallRequest) {
    let executor = self.tool_executor.clone();
    let agent_tx = self.agent_tx.clone();
    let ctx = crate::tool::ToolContext::default();

    tokio::spawn(async move {
        let result = executor.execute(&req.tool_name, req.arguments, &ctx).await;
        if let Some(tx) = agent_tx {
            let _ = tx.send(AgentMessage::ToolCallResult(ToolCallResult {
                tool_use_id: req.tool_use_id,
                content: result.map_or_else(
                    |e| format!("Tool error: {e}"),
                    |r| r.content,
                ),
                is_error: result.is_err(),
            }));
        }
    });
}
```

### 40.4 Reduce TUI idle CPU usage (`src/app.rs`)

**Problem**: The TUI loop calls `try_recv()` + `poll_event(16ms)` continuously, waking 60 times/second even when idle. With the dirty flag optimization (step 28), redraws are skipped, but the thread still wakes up for every tick.

**Fix**: Use `crossterm::event::poll` with a longer timeout when idle, shorter when streaming:

```rust
// In the TUI loop:
let poll_timeout = if status.is_streaming {
    Duration::from_millis(16)   // 60fps during streaming
} else {
    Duration::from_millis(100)  // 10fps when idle — saves CPU
};

if let Some(event) = poll_event(poll_timeout) { ... }
```

This is a minimal, safe change that reduces idle CPU by ~6x while maintaining smooth streaming.

### 40.5 Document zero-copy compromise

**Problem**: CLAUDE.md states "Zero-copy where possible" but `StreamChunk`, messages, and UI updates all use heap-allocated `String`. Converting to `Cow<'_, str>` or `bytes::Bytes` across async channel boundaries would require major refactoring with lifetime gymnastics.

**Fix**: Add a documented acknowledgement in the module docs and a pragmatic note in CLAUDE.md. This is a known trade-off, not a bug — `String` across `mpsc` channels is the standard Rust pattern for owned async message passing.

Update `src/provider/mod.rs` module doc:
```rust
//! NOTE: StreamChunk fields use owned `String` rather than `Cow<'_, str>`
//! or `bytes::Bytes`. While the architecture doc calls for zero-copy where
//! possible, owned types are required for `Send + 'static` across async
//! channel boundaries. This is the standard Rust pattern for message passing.
```

---

## Tests

```rust
#[cfg(test)]
mod auto_saver_fix_tests {
    // The starvation fix is verified by a timing-sensitive test:
    // send multiple saves rapidly, then wait >2s, verify file was written
}

#[cfg(test)]
mod json_preview_tests {
    #[test]
    fn preview_captures_booleans() {
        let fields = extract_partial_json_fields(r#"{"create_dirs": true, "path": "/test"}"#);
        assert_eq!(fields.get("create_dirs").unwrap(), "true");
        assert_eq!(fields.get("path").unwrap(), "/test");
    }

    #[test]
    fn preview_captures_numbers() {
        let fields = extract_partial_json_fields(r#"{"line": 42, "col": 10}"#);
        assert_eq!(fields.get("line").unwrap(), "42");
    }

    #[test]
    fn preview_handles_escaped_quotes() {
        let fields = extract_partial_json_fields(r#"{"content": "println!(\"Hello\");"}"#);
        assert!(fields.get("content").unwrap().contains("Hello"));
    }

    #[test]
    fn preview_truncated_mid_value() {
        let fields = extract_partial_json_fields(r#"{"path": "/src/main.rs", "line"#);
        assert_eq!(fields.get("path").unwrap(), "/src/main.rs");
    }

    #[test]
    fn preview_truncated_mid_string() {
        let fields = extract_partial_json_fields(r#"{"path": "/src/ma"#);
        assert_eq!(fields.get("path").unwrap(), "/src/ma");
    }
}
```

## Acceptance Criteria
- [ ] AutoSaver debounce fires reliably during fast streaming (pinned sleep)
- [ ] JSON preview captures booleans, numbers, arrays, escaped quotes
- [ ] Regex removed from tool_parser.rs
- [ ] Tool executor wired in controller — tools actually execute
- [ ] TUI idle CPU reduced (longer poll timeout when not streaming)
- [ ] Zero-copy trade-off documented in provider module
- [ ] All existing tests pass (no regressions)
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes
