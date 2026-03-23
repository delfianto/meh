# STEP 41 — Security & Robustness Fixes (Gemini Audit Round 2)

## Objective
Fix 4 confirmed issues from the second Gemini 3.1 Pro audit. The permission bypass is a critical security flaw. The remaining 3 are robustness improvements.

## Prerequisites
- STEP 40 complete

## Context

| # | Severity | Issue | Location |
|---|----------|-------|----------|
| 1 | **CRITICAL** | Permission system bypassed — tools execute without user approval | `controller/mod.rs` |
| 2 | **HIGH** | All channels unbounded — no backpressure, OOM risk | Throughout |
| 3 | **MEDIUM** | Working directory not tracked per-task — `cd` has no effect | `controller/mod.rs`, `state/task_state.rs` |
| 4 | **MEDIUM** | TUI panic in draw loop crashes app — no `catch_unwind` | `app.rs` |

Finding #3 from the audit (JSON parsing) was already fixed in STEP 40 and confirmed as good.

---

## Detailed Instructions

### 41.1 Wire the permission system into tool execution (CRITICAL)

**Problem**: `handle_tool_call_request` calls `executor.execute()` immediately without checking `PermissionController`. The `ApprovalResponse` handler logs and drops. The `PermissionController` exists (`src/permission/mod.rs`) but is dead code.

**Fix**: Implement a pending-tool-call state machine in the controller:

1. Add a `pending_tool_calls` map to hold requests awaiting approval:
```rust
use std::collections::HashMap;

// In Controller struct:
pending_tool_calls: HashMap<String, messages::ToolCallRequest>,
```

2. When a `ToolCallRequest` arrives, check permissions first:
```rust
fn handle_tool_call_request(&mut self, req: messages::ToolCallRequest) {
    // In YOLO mode, execute immediately
    if self.permission_mode == PermissionMode::Yolo {
        self.execute_tool(req);
        return;
    }

    // Check auto-approve rules
    let permission = self.check_auto_approve(&req.tool_name);
    if permission {
        self.execute_tool(req);
        return;
    }

    // Otherwise, ask the user
    tracing::info!(tool = req.tool_name, "Requesting tool approval");
    let _ = self.ui_tx.send(UiUpdate::ToolApproval {
        tool_use_id: req.tool_use_id.clone(),
        tool_name: req.tool_name.clone(),
        description: req.description.clone(),
    });
    self.pending_tool_calls.insert(req.tool_use_id.clone(), req);
}
```

3. Handle the `ApprovalResponse` by executing or denying:
```rust
ControllerMessage::ApprovalResponse { tool_use_id, approved, always_allow } => {
    if let Some(req) = self.pending_tool_calls.remove(&tool_use_id) {
        if approved {
            self.execute_tool(req);
        } else {
            // Send denial back to agent
            if let Some(tx) = &self.agent_tx {
                let _ = tx.send(AgentMessage::ToolCallResult(ToolCallResult {
                    tool_use_id,
                    content: "Tool call denied by user.".to_string(),
                    is_error: true,
                }));
            }
        }
    }
}
```

4. Extract the execution logic into a separate `execute_tool` method:
```rust
fn execute_tool(&self, req: messages::ToolCallRequest) {
    tracing::info!(tool = req.tool_name, "Executing tool call");
    let executor = self.tool_executor.clone();
    let agent_tx = self.agent_tx.clone();
    let ctx = ToolContext { cwd: self.cwd.clone(), auto_approved: false };

    tokio::spawn(async move {
        let response = executor.execute(&req.tool_name, req.arguments, &ctx).await;
        if let Some(tx) = agent_tx {
            let (content, is_error) = match response {
                Ok(r) => (r.content, r.is_error),
                Err(e) => (format!("Tool error: {e}"), true),
            };
            let _ = tx.send(AgentMessage::ToolCallResult(ToolCallResult {
                tool_use_id: req.tool_use_id,
                content,
                is_error,
            }));
        }
    });
}
```

5. Add a `check_auto_approve` helper that consults the config auto-approve rules:
```rust
fn check_auto_approve(&self, tool_name: &str) -> bool {
    // Check if tool is in an auto-approved category based on config
    // read_file, list_files, search_files → auto_approve.read_files
    // write_to_file, apply_patch → auto_approve.edit_files
    // execute_command → auto_approve.execute_safe_commands or execute_all_commands
    false // Default: require approval
}
```

### 41.2 Replace unbounded channels with bounded + backpressure (HIGH)

**Problem**: All MPSC channels use `unbounded_channel()`. During fast streaming, the TUI can't keep up with the controller, causing unbounded queue growth.

**Fix**: Replace the two primary high-throughput channels with bounded channels:

```rust
// In Controller::new():
let (ctrl_tx, rx) = mpsc::channel(4096);     // was unbounded
let (ui_tx, ui_rx) = mpsc::channel(4096);    // was unbounded

// Agent channel can stay unbounded — it's low-throughput (tool results only)
let (agent_tx, agent_rx) = mpsc::unbounded_channel();
```

**Callers must handle `SendError`**: Change `let _ = self.ui_tx.send(...)` to handle the bounded send. Since `send()` on a bounded channel is async, and some callers are sync, use `try_send()` with overflow handling:

```rust
// For non-critical UI updates (stream content):
if self.ui_tx.try_send(update).is_err() {
    tracing::warn!("UI channel full, dropping update");
}

// For critical messages (Quit, StreamEnd):
// Use blocking send or ensure channel is large enough
let _ = self.ui_tx.try_send(update);
```

**Practical note**: With 4096 capacity and the UiBatcher already coalescing at 60fps, overflow should never happen in practice. The bounded channel is a safety net, not an active flow control mechanism.

### 41.3 Track mutable working directory per task (MEDIUM)

**Problem**: `ToolContext.cwd` is read from `std::env::current_dir()` on every call. An LLM running `cd /some/dir && command` in a subprocess doesn't change the Rust process cwd, so subsequent tool calls still use the original directory.

**Fix**: Add a `cwd` field to the Controller that can be updated:

```rust
// In Controller struct:
cwd: String,

// In Controller::new():
let cwd_str = cwd.to_string_lossy().to_string();

// In handle_tool_call_request:
let ctx = ToolContext {
    cwd: self.cwd.clone(),
    auto_approved: self.permission_mode == PermissionMode::Yolo,
};
```

The cwd can be updated via a slash command (`/cd <path>`) or when the agent explicitly requests it. For now, setting it once at startup and using it consistently is the correct fix — the key issue is that it shouldn't be re-read from the process environment on every call.

### 41.4 Add `catch_unwind` around TUI draw loop (MEDIUM)

**Problem**: A panic inside `tui.draw()` or event handling crashes the blocking thread. While RAII `Drop` and the panic hook handle terminal restoration, the app terminates ungracefully.

**Fix**: Wrap the TUI event loop in `catch_unwind`:

```rust
fn run_tui(...) -> anyhow::Result<()> {
    let mut tui = tui::Tui::new()?;
    // ... setup ...

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tui_event_loop(&mut tui, &ctrl_tx, &mut ui_rx, ...)
    }));

    // Always restore terminal, even after panic
    tui.restore()?;

    match result {
        Ok(inner) => inner,
        Err(panic_payload) => {
            let msg = panic_payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic_payload.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            Err(anyhow::anyhow!("TUI panicked: {msg}"))
        }
    }
}
```

This ensures:
- Terminal is always restored (even on panic)
- Panic is converted to an `anyhow::Error` instead of crashing
- The error message is preserved for logging

---

## Tests

```rust
#[cfg(test)]
mod permission_tests {
    #[tokio::test]
    async fn yolo_mode_auto_approves() {
        // Create controller in Yolo mode, send ToolCallRequest
        // Verify it executes immediately (no ToolApproval sent to TUI)
    }

    #[tokio::test]
    async fn ask_mode_sends_approval_prompt() {
        // Create controller in Ask mode, send ToolCallRequest
        // Verify ToolApproval sent to TUI
        // Verify tool NOT executed yet (no ToolCallResult to agent)
    }

    #[tokio::test]
    async fn approval_response_executes_tool() {
        // Send ToolCallRequest → verify pending
        // Send ApprovalResponse(approved=true) → verify tool executes
    }

    #[tokio::test]
    async fn denial_response_returns_error() {
        // Send ToolCallRequest → verify pending
        // Send ApprovalResponse(approved=false) → verify error result to agent
    }
}

#[cfg(test)]
mod bounded_channel_tests {
    #[tokio::test]
    async fn controller_handles_full_channel() {
        // Fill the UI channel to capacity
        // Verify controller doesn't deadlock
    }
}

#[cfg(test)]
mod cwd_tests {
    #[tokio::test]
    async fn cwd_consistent_across_tool_calls() {
        // Verify ToolContext.cwd is the same for consecutive calls
        // (not re-read from env each time)
    }
}
```

## Acceptance Criteria
- [ ] YOLO mode executes tools immediately without approval prompt
- [ ] Ask mode sends `ToolApproval` to TUI and waits for `ApprovalResponse`
- [ ] Approved tools execute; denied tools return error to agent
- [ ] Pending tool calls tracked in `HashMap` and cleaned up
- [ ] Primary channels bounded with backpressure (4096 capacity)
- [ ] `try_send` used for non-critical updates; no deadlocks
- [ ] Working directory tracked as `cwd` field on Controller, not re-read from env
- [ ] `catch_unwind` around TUI draw loop; terminal always restored
- [ ] Panic converted to `anyhow::Error`, not process termination
- [ ] All existing tests pass (no regressions)
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes
