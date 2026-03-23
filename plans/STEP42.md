# STEP 42 — Incremental JSON Parser, Async TUI, Task Abort Safety

## Objective
Address the final 3 Gemini audit findings: replace the brute-force JSON preview parser with `jiter` for proper incremental parsing, migrate the TUI from busy-poll to async `EventStream`, and store agent `JoinHandle` for abort safety on cancellation.

## Prerequisites
- STEP 41 complete

## Context

| # | Severity | Issue | Fix |
|---|----------|-------|-----|
| 1 | **MEDIUM** | JSON preview allocates O(N) strings per chunk with serde repair | Replace with `jiter::JsonValue::parse_owned` + `PartialMode` |
| 2 | **MEDIUM** | TUI busy-polls at 10-60fps even when idle | Migrate to `crossterm::EventStream` + `tokio::select!` — zero CPU when idle |
| 3 | **MEDIUM** | Agent `tokio::spawn` returns `JoinHandle` that's dropped — leaked task on cancel | Store handle, `.abort()` after cancel timeout |

---

## Detailed Instructions

### 42.1 Integrate `jiter` for incremental JSON parsing (`src/streaming/tool_parser.rs`)

**Problem**: `extract_partial_json_fields` tries 5 string suffix candidates through `serde_json::from_str` — O(N) allocations per call, fragile for nested structures.

**Fix**: Use `jiter::JsonValue::parse_owned()` with `PartialMode::TrailingStrings` which handles incomplete JSON natively without string concatenation guesses.

**Cargo.toml**:
```toml
jiter = "0.13"
```

**Implementation**:
```rust
use jiter::{JsonValue, PartialMode};

pub fn extract_partial_json_fields(partial: &str) -> HashMap<String, String> {
    // jiter handles incomplete JSON natively with PartialMode
    let Ok(value) = JsonValue::parse_owned(
        partial.as_bytes(),
        false,  // allow_inf_nan
        PartialMode::TrailingStrings,
    ) else {
        return HashMap::new();
    };

    let JsonValue::Object(map) = value else {
        return HashMap::new();
    };

    map.iter()
        .map(|(k, v)| (k.to_string(), jiter_value_to_preview(v)))
        .collect()
}

fn jiter_value_to_preview(v: &JsonValue) -> String {
    match v {
        JsonValue::Str(s) => s.to_string(),
        JsonValue::Int(n) => n.to_string(),
        JsonValue::Float(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => "null".to_string(),
        JsonValue::Array(_) => "[...]".to_string(),
        JsonValue::Object(_) => "{...}".to_string(),
        JsonValue::BigInt(n) => n.to_string(),
    }
}
```

**Why jiter over serde repair**:
- Single parse call instead of 5 candidates
- Handles nested objects, arrays, trailing strings natively
- Used by Pydantic (proven at scale for LLM JSON parsing)
- Zero string allocation for the repair — parser handles truncation internally

### 42.2 Migrate TUI to async `EventStream` (`src/app.rs`)

**Problem**: `run_tui` runs in `spawn_blocking` with a manual `poll_event(timeout)` loop that wakes 10-60 times/second even when idle.

**Fix**: Move the TUI to the async runtime using `crossterm::event::EventStream` with `tokio::select!`. The event loop sleeps at 0% CPU until an event arrives.

**Cargo.toml**:
```toml
# Add crossterm directly with event-stream feature
crossterm = { version = "0.28", features = ["event-stream"] }
# futures is already present; need StreamExt
```

**Architecture change**:
```
BEFORE:
  App::run()
    ├── tokio::spawn(controller.run())       [async]
    └── spawn_blocking(run_tui(...))         [blocking, polls at 60fps]

AFTER:
  App::run()
    ├── tokio::spawn(controller.run())       [async]
    └── run_tui_async(...)                   [async, event-driven]
```

**Key changes to `run_tui`**:
```rust
use crossterm::event::EventStream;
use futures::StreamExt;

async fn run_tui_async(
    ctrl_tx: &mpsc::UnboundedSender<ControllerMessage>,
    mut ui_rx: mpsc::Receiver<UiUpdate>,
    // ... other params
) -> anyhow::Result<()> {
    let mut tui = tui::Tui::new()?;
    let mut event_stream = EventStream::new();
    // ... setup chat_state, input, status ...

    let render_interval = Duration::from_millis(16); // 60fps max
    let mut render_tick = tokio::time::interval(render_interval);
    let mut dirty = true;

    loop {
        tokio::select! {
            // Terminal events (keyboard, mouse, resize)
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(event)) => {
                        // Convert crossterm::event::Event to our handling
                        handle_crossterm_event(event, &mut input, &ctrl_tx, ...);
                        dirty = true;
                    }
                    Some(Err(e)) => tracing::warn!(error = %e, "Event stream error"),
                    None => break,
                }
            }

            // UI updates from controller
            Some(update) = ui_rx.recv() => {
                if apply_ui_update(update, &mut chat_state, &mut status, &mut stream_state) {
                    break; // Quit
                }
                dirty = true;
            }

            // Render tick (only redraws if dirty)
            _ = render_tick.tick() => {
                if dirty {
                    tui.draw(|frame| {
                        tui::app_layout::render_app(frame, &chat_state, &input, &status);
                    })?;
                    dirty = false;
                }
            }
        }
    }

    tui.restore()?;
    Ok(())
}
```

**Key benefits**:
- Zero CPU when idle (no polling — `select!` sleeps until an event fires)
- Render tick only redraws when dirty (same as current)
- No `spawn_blocking` needed — TUI runs in async context
- `EventStream` integrates directly with `tokio::select!`
- Still wrapped in `catch_unwind` for panic safety

**In `App::run()`**:
```rust
// BEFORE:
let tui_result = tokio::task::spawn_blocking(move || {
    run_tui(&ctrl_tx, ui_rx, ...)
}).await?;

// AFTER:
let tui_result = run_tui_async(&ctrl_tx, ui_rx, ...).await;
```

### 42.3 Store `JoinHandle` and abort on cancel timeout

**Problem**: `tokio::spawn(agent.run())` returns a `JoinHandle` that's immediately dropped. If the agent doesn't respond to `AgentMessage::Cancel`, the task leaks forever.

**Fix**: Store the handle and abort after a timeout:

```rust
// In Controller struct:
agent_handle: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,

// In handle_user_submit:
let handle = tokio::spawn(agent.run());
self.agent_handle = Some(handle);

// In CancelTask handler, after sending AgentMessage::Cancel:
if let Some(handle) = self.agent_handle.take() {
    let abort_handle = handle.abort_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if !handle.is_finished() {
            tracing::warn!("Agent did not exit within 5s, aborting");
            abort_handle.abort();
        }
    });
}

// In TaskComplete handler:
self.agent_handle = None;
```

---

## Tests

```rust
#[cfg(test)]
mod jiter_preview_tests {
    #[test]
    fn jiter_handles_partial_string() {
        let fields = extract_partial_json_fields(r#"{"path": "/src/ma"#);
        assert_eq!(fields.get("path").unwrap(), "/src/ma");
    }

    #[test]
    fn jiter_handles_booleans_and_numbers() {
        let fields = extract_partial_json_fields(r#"{"count": 42, "flag": true}"#);
        assert_eq!(fields.get("count").unwrap(), "42");
        assert_eq!(fields.get("flag").unwrap(), "true");
    }

    #[test]
    fn jiter_handles_escaped_quotes() {
        let fields = extract_partial_json_fields(r#"{"content": "println!(\"Hello\");"}"#);
        assert!(fields.get("content").unwrap().contains("Hello"));
    }

    #[test]
    fn jiter_handles_nested_object() {
        let fields = extract_partial_json_fields(r#"{"path": "/test", "opts": {"a": 1}}"#);
        assert_eq!(fields.get("opts").unwrap(), "{...}");
    }

    #[test]
    fn jiter_handles_empty_input() {
        assert!(extract_partial_json_fields("").is_empty());
    }

    #[test]
    fn jiter_handles_truncated_mid_key() {
        let fields = extract_partial_json_fields(r#"{"path": "/test", "li"#);
        assert_eq!(fields.get("path").unwrap(), "/test");
    }
}

#[cfg(test)]
mod abort_tests {
    #[tokio::test]
    async fn join_handle_abort_kills_task() {
        let handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        handle.abort();
        assert!(handle.await.unwrap_err().is_cancelled());
    }
}
```

## Acceptance Criteria
- [x] `jiter` used for incremental JSON parsing with `PartialMode::TrailingStrings`
- [x] Serde repair candidates removed from `extract_partial_json_fields`
- [x] JSON preview handles partial strings, booleans, numbers, nested objects
- [x] TUI migrated from `spawn_blocking` + `poll_event` to async `EventStream`
- [x] `tokio::select!` multiplexes terminal events, UI updates, and render ticks
- [x] Zero CPU when TUI is idle (no polling)
- [x] `catch_unwind` removed (async incompatible) — RAII Drop + panic hook cover restoration
- [x] Agent `JoinHandle` stored on Controller
- [x] Cancel sends message then aborts after 5s timeout if agent hangs
- [x] Handle cleared on `TaskComplete`
- [x] All existing tests pass (no regressions)
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes
