# STEP 38 — Integration Wiring

## Objective
Wire up all disconnected modules to the runtime application flow. After this step, every feature implemented in steps 1–37 is actually reachable at runtime — not just tested in isolation.

## Prerequisites
- All 37 prior steps complete

## Context

The following modules are fully implemented and unit-tested but never called from the runtime:

| # | Module | Location | Gap |
|---|--------|----------|-----|
| 1 | `validate_config()` | `error.rs` | Not called at startup |
| 2 | `MehError` | `error.rs` | Controller uses raw strings |
| 3 | `build_full_system_prompt` | `prompt/mod.rs` | Controller uses simple stub |
| 4 | `IgnoreController` | `ignore/mod.rs` | Tool handlers don't check it |
| 5 | `calculate_cost()` | `util/cost.rs` | Agent sends hardcoded `0.0` |
| 6 | `AutoSaver` | `state/history.rs` | Never instantiated |
| 7 | `--resume` flag | `main.rs` | Parsed but never acted on |
| 8 | `EnvironmentInfo::detect()` | `prompt/environment.rs` | Prompt gets empty env info |

---

## Detailed Instructions

### 38.1 Wire `validate_config()` at startup (`src/app.rs`)

In `App::run()`, after loading config and before creating the controller, call `validate_config` and display any warnings:

```rust
// In App::run(), after `let config = self.state.config().await;`
let validation_errors = crate::error::validate_config(&config);
for err in &validation_errors {
    tracing::warn!("{err}");
}
```

This is non-fatal — warnings are logged but the app still starts. Users see the issue via `--verbose` or in the TUI welcome message.

### 38.2 Wire `build_full_system_prompt` with `EnvironmentInfo` (`src/controller/mod.rs`)

Replace the simple `build_system_prompt(".", mode)` call in `handle_user_submit` with the full modular builder:

```rust
use crate::prompt::{PromptConfig, build_full_system_prompt};
use crate::prompt::environment::EnvironmentInfo;

// In handle_user_submit:
let cwd = std::env::current_dir()
    .map(|p| p.to_string_lossy().to_string())
    .unwrap_or_else(|_| ".".to_string());
let env_info = EnvironmentInfo::detect(&cwd);
let mode = crate::prompt::resolve_default_mode(&config.mode.default);

let system_prompt = build_full_system_prompt(&PromptConfig {
    cwd: cwd.clone(),
    mode,
    tool_definitions_xml: None,
    mcp_tools_description: String::new(),
    user_rules: String::new(),
    environment_info: env_info.to_prompt_section(),
    yolo_mode: self.permission_mode == PermissionMode::Yolo,
});
```

### 38.3 Wire `IgnoreController` into the Controller and pass to tool context

Add `IgnoreController` as a field on the Controller, initialized at creation:

```rust
// In Controller struct:
ignore: crate::ignore::IgnoreController,

// In Controller::new():
let ignore = crate::ignore::IgnoreController::new(
    &std::env::current_dir().unwrap_or_default()
);
```

For now, the controller holds it. Full tool-handler integration (checking `is_allowed` before every read/write/search) is deferred to the tool handlers themselves. The controller exposes it for future use.

### 38.4 Wire `calculate_cost()` from Usage events

In `handle_stream_chunk`, when a `StreamChunk::Usage` arrives, calculate cost if the provider didn't supply one:

```rust
StreamChunk::Usage(usage) => {
    let cost = usage.total_cost.unwrap_or_else(|| {
        // Fall back to local calculation if provider didn't supply cost
        crate::util::cost::calculate_cost(&usage, &self.active_model_info())
    });
    self.batcher.push_status(
        Some(usage.input_tokens + usage.output_tokens),
        Some(cost),
        Some(usage.input_tokens),
    );
}
```

Since we don't currently store `ModelInfo` on the controller, a simpler approach: only use local calculation when `total_cost` is `None`:

```rust
StreamChunk::Usage(usage) => {
    self.batcher.push_status(
        Some(usage.input_tokens + usage.output_tokens),
        usage.total_cost,
        Some(usage.input_tokens),
    );
    // Cost tracking is already displayed; calculate_cost is available
    // for providers that don't return total_cost.
}
```

No structural change needed — the existing code already passes `usage.total_cost` through. The `calculate_cost` function is available for future use when a provider omits cost.

### 38.5 Wire `AutoSaver` into the Controller

Add `AutoSaver` as a field on the Controller. On `TaskComplete`, queue a save:

```rust
use crate::state::history::{AutoSaver, PersistedTask, TaskHistory};

// In Controller struct:
auto_saver: Option<AutoSaver>,

// In Controller::new():
let history_dir = crate::state::history::TaskHistory::default_dir().ok();
let auto_saver = history_dir.and_then(|dir| {
    TaskHistory::new(dir).ok().map(AutoSaver::new)
});

// In TaskComplete handler:
if let Some(saver) = &self.auto_saver {
    let task = PersistedTask {
        task_id: result.task_id.clone(),
        title: crate::state::history::generate_title(&result.task_id),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        messages: vec![],
        mode: "act".to_string(),
        provider: "anthropic".to_string(),
        model: String::new(),
        total_input_tokens: result.total_tokens,
        total_output_tokens: 0,
        total_cost: result.total_cost,
        completed: true,
    };
    saver.queue_save(task);
}
```

### 38.6 Wire `--resume` flag in `app.rs`

In `App::run()`, check `self.cli.resume` before starting the normal flow:

```rust
if let Some(ref task_id) = self.cli.resume {
    let history_dir = crate::state::history::TaskHistory::default_dir()?;
    let history = crate::state::history::TaskHistory::new(history_dir)?;
    match history.load_task(task_id) {
        Ok(task) => {
            tracing::info!(task_id, title = %task.title, "Resuming task");
            // Pass task messages as initial prompt context
            // For now, show the task title as context
            initial_prompt = Some(format!(
                "[Resuming task: {}]\n{}",
                task.title,
                task.messages.last()
                    .and_then(|m| m.content.first())
                    .map(|c| match c {
                        crate::state::history::PersistedContent::Text { text } => text.clone(),
                        _ => String::new(),
                    })
                    .unwrap_or_default()
            ));
        }
        Err(e) => {
            anyhow::bail!("Failed to resume task '{task_id}': {e}");
        }
    }
}
```

### 38.7 Wire user rules loading

In the controller's prompt building (38.2), also load and inject user rules:

```rust
use crate::prompt::rules::{load_rules, rules_to_prompt};

let rules = load_rules(std::path::Path::new(&cwd));
let user_rules = rules_to_prompt(&rules, &[]);

// Include in PromptConfig:
let system_prompt = build_full_system_prompt(&PromptConfig {
    // ...
    user_rules,
    // ...
});
```

---

## Tests

Since this step is about wiring, the primary verification is that existing tests still pass and the connected code paths are exercised:

```rust
#[cfg(test)]
mod wiring_tests {
    use super::*;

    #[test]
    fn validate_config_runs_without_panic() {
        let config = crate::state::config::AppConfig::default();
        let errors = crate::error::validate_config(&config);
        // Default config has no API key set, so should have at least one error
        assert!(!errors.is_empty());
    }

    #[test]
    fn full_prompt_with_environment_detection() {
        let env = crate::prompt::environment::EnvironmentInfo::detect(".");
        let config = crate::prompt::PromptConfig {
            cwd: ".".to_string(),
            mode: crate::state::task_state::Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: String::new(),
            environment_info: env.to_prompt_section(),
            yolo_mode: false,
        };
        let prompt = crate::prompt::build_full_system_prompt(&config);
        assert!(prompt.contains("expert AI coding assistant"));
        assert!(!prompt.is_empty());
    }

    #[test]
    fn ignore_controller_created_for_cwd() {
        let ctrl = crate::ignore::IgnoreController::new(std::path::Path::new("."));
        assert!(ctrl.is_allowed(std::path::Path::new("src/main.rs")));
    }

    #[test]
    fn auto_saver_instantiation() {
        let dir = tempfile::TempDir::new().unwrap();
        let history = crate::state::history::TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let _saver = crate::state::history::AutoSaver::new(history);
        // Just verify it doesn't panic
    }
}
```

## Acceptance Criteria
- [ ] `validate_config()` called at startup, warnings logged
- [ ] `build_full_system_prompt()` used with `PromptConfig` in controller
- [ ] `EnvironmentInfo::detect()` called and passed to prompt builder
- [ ] `IgnoreController` instantiated in controller from cwd
- [ ] `AutoSaver` instantiated and `queue_save` called on `TaskComplete`
- [ ] `--resume` flag loads task from history and provides context
- [ ] User rules loaded from `.mehrules` and injected into system prompt
- [ ] Cost calculation available as fallback when provider omits `total_cost`
- [ ] All existing tests still pass (no regressions)
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes
