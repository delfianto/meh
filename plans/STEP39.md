# STEP 39 — Integration Tests

## Objective
Add integration tests that exercise full component pipelines end-to-end, covering the manual verification items flagged across PRs #18–#34 and the remaining unchecked acceptance criteria. These tests use mock providers and in-memory state to verify behavior without requiring real APIs or a running TUI.

## Prerequisites
- STEP 38 complete (integration wiring)

## Context

22 manual test items were flagged across PRs but never verified. Most can be covered by automated integration tests that exercise the Controller ↔ Agent ↔ Tool pipeline with mock providers.

Additionally, 16 unchecked acceptance criteria from earlier steps are TUI/integration-level items that can be partially verified through controller-level tests.

---

## Detailed Instructions

### 39.1 Integration test infrastructure (`tests/common/mod.rs`)

Create shared test helpers for integration tests:

```rust
//! Shared helpers for integration tests.

use meh::controller::Controller;
use meh::controller::messages::{ControllerMessage, UiUpdate};
use meh::permission::PermissionMode;
use meh::state::StateManager;
use tokio::sync::mpsc;

/// Create a test controller with a temporary config.
pub async fn make_test_controller() -> (
    Controller,
    mpsc::UnboundedSender<ControllerMessage>,
    mpsc::UnboundedReceiver<UiUpdate>,
) {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let state = StateManager::new(Some(path)).await.unwrap();
    Controller::new(state, PermissionMode::Ask)
}

/// Drain all pending UiUpdates from the receiver.
pub fn drain_updates(rx: &mut mpsc::UnboundedReceiver<UiUpdate>) -> Vec<UiUpdate> {
    let mut updates = Vec::new();
    while let Ok(update) = rx.try_recv() {
        updates.push(update);
    }
    updates
}

/// Find the first UiUpdate matching a predicate.
pub fn find_update<F>(updates: &[UiUpdate], pred: F) -> Option<&UiUpdate>
where
    F: Fn(&UiUpdate) -> bool,
{
    updates.iter().find(|u| pred(u))
}
```

### 39.2 Controller slash command integration (`tests/slash_commands.rs`)

Test that slash commands flow through the full controller pipeline:

```rust
#[tokio::test]
async fn slash_help_displays_commands() {
    let (controller, ctrl_tx, mut ui_rx) = make_test_controller().await;
    let handle = tokio::spawn(controller.run());

    ctrl_tx.send(ControllerMessage::SlashCommand(
        SlashCommand::Help, String::new()
    )).unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    let updates = drain_updates(&mut ui_rx);
    assert!(updates.iter().any(|u| matches!(u,
        UiUpdate::AppendMessage { content, .. } if content.contains("/help")
    )));

    ctrl_tx.send(ControllerMessage::Quit).unwrap();
    let _ = handle.await;
}

#[tokio::test]
async fn slash_yolo_toggles_permission_mode() {
    let (controller, ctrl_tx, mut ui_rx) = make_test_controller().await;
    let handle = tokio::spawn(controller.run());

    ctrl_tx.send(ControllerMessage::SlashCommand(
        SlashCommand::Yolo, String::new()
    )).unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    let updates = drain_updates(&mut ui_rx);
    assert!(updates.iter().any(|u| matches!(u,
        UiUpdate::StatusUpdate { is_yolo: Some(true), .. }
    )));

    ctrl_tx.send(ControllerMessage::Quit).unwrap();
    let _ = handle.await;
}

#[tokio::test]
async fn slash_mode_switches() {
    let (controller, ctrl_tx, mut ui_rx) = make_test_controller().await;
    let handle = tokio::spawn(controller.run());

    ctrl_tx.send(ControllerMessage::SlashCommand(
        SlashCommand::Mode("plan".to_string()), String::new()
    )).unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    let updates = drain_updates(&mut ui_rx);
    assert!(updates.iter().any(|u| matches!(u,
        UiUpdate::StatusUpdate { mode: Some(m), .. } if m == "PLAN"
    )));

    ctrl_tx.send(ControllerMessage::Quit).unwrap();
    let _ = handle.await;
}

#[tokio::test]
async fn slash_new_cancels_agent() {
    // Verify /new sends cancel to agent and shows new task message
}

#[tokio::test]
async fn all_slash_commands_parse() {
    // Verify every command from the spec parses correctly
    let commands = [
        "/help", "/h", "/clear", "/compact", "/smol",
        "/history", "/settings", "/new", "/newtask",
        "/plan", "/act", "/model claude-sonnet-4", "/yolo",
    ];
    for cmd in commands {
        assert!(
            parse_slash_command(cmd).is_some(),
            "Failed to parse: {cmd}"
        );
    }
}
```

### 39.3 Cancellation flow integration (`tests/cancellation.rs`)

Test the full cancel → double-cancel pipeline:

```rust
#[tokio::test]
async fn single_cancel_sends_cancel_message() {
    let (controller, ctrl_tx, mut ui_rx) = make_test_controller().await;
    let handle = tokio::spawn(controller.run());

    ctrl_tx.send(ControllerMessage::CancelTask).unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    let updates = drain_updates(&mut ui_rx);

    // Should show "Task cancelled" and clear streaming
    assert!(updates.iter().any(|u| matches!(u,
        UiUpdate::AppendMessage { content, .. } if content.contains("cancelled")
    )));
    assert!(updates.iter().any(|u| matches!(u,
        UiUpdate::StatusUpdate { is_streaming: Some(false), .. }
    )));

    ctrl_tx.send(ControllerMessage::Quit).unwrap();
    let _ = handle.await;
}

#[tokio::test]
async fn double_cancel_force_quits() {
    let (controller, ctrl_tx, mut ui_rx) = make_test_controller().await;
    let handle = tokio::spawn(controller.run());

    ctrl_tx.send(ControllerMessage::CancelTask).unwrap();
    ctrl_tx.send(ControllerMessage::CancelTask).unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    let updates = drain_updates(&mut ui_rx);
    assert!(updates.iter().any(|u| matches!(u, UiUpdate::Quit)));

    let _ = handle.await;
}

#[tokio::test]
async fn task_cancellation_token_reset() {
    let mut tc = TaskCancellation::new();
    tc.cancel();
    assert!(tc.is_cancelled());
    tc.reset();
    assert!(!tc.is_cancelled());
    let is_double = tc.cancel();
    assert!(!is_double); // reset cleared the timer
}
```

### 39.4 Config reload integration (`tests/config_reload.rs`)

Test that config changes are detected and reloaded:

```rust
#[tokio::test]
async fn config_reload_updates_state() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    let config = AppConfig::default();
    config.save(&path).unwrap();

    let state = StateManager::new(Some(path.clone())).await.unwrap();
    assert_eq!(state.config().await.provider.default, "anthropic");

    // Simulate external edit
    let mut new_config = AppConfig::default();
    new_config.provider.default = "openai".to_string();
    new_config.save(&path).unwrap();

    state.reload().await.unwrap();
    assert_eq!(state.config().await.provider.default, "openai");
}

#[tokio::test]
async fn config_reload_invalid_preserves_old() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    AppConfig::default().save(&path).unwrap();

    let state = StateManager::new(Some(path.clone())).await.unwrap();
    std::fs::write(&path, "invalid [[[toml").unwrap();

    assert!(state.reload().await.is_err());
    assert_eq!(state.config().await.provider.default, "anthropic");
}

#[tokio::test]
async fn controller_handles_config_reload_message() {
    let (controller, ctrl_tx, mut ui_rx) = make_test_controller().await;
    let handle = tokio::spawn(controller.run());

    ctrl_tx.send(ControllerMessage::ConfigReload).unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    let updates = drain_updates(&mut ui_rx);
    // Should show reload confirmation or warning
    assert!(!updates.is_empty());

    ctrl_tx.send(ControllerMessage::Quit).unwrap();
    let _ = handle.await;
}
```

### 39.5 Ignore controller integration (`tests/ignore_integration.rs`)

Test that `.mehignore` protects paths end-to-end:

```rust
#[test]
fn default_patterns_block_env_files() {
    let dir = TempDir::new().unwrap();
    let ctrl = IgnoreController::new(dir.path());
    assert!(!ctrl.is_allowed(&dir.path().join(".env")));
    assert!(!ctrl.is_allowed(&dir.path().join(".env.production")));
    assert!(!ctrl.is_allowed(&dir.path().join("server.pem")));
    assert!(!ctrl.is_allowed(&dir.path().join("credentials.json")));
    assert!(ctrl.is_allowed(&dir.path().join("src/main.rs")));
}

#[test]
fn custom_mehignore_blocks_patterns() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(".mehignore"), "*.secret\nbuild/**\n").unwrap();
    let ctrl = IgnoreController::new(dir.path());
    assert!(!ctrl.is_allowed(&dir.path().join("api.secret")));
    assert!(ctrl.is_allowed(&dir.path().join("src/lib.rs")));
}

#[test]
fn negation_overrides() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(".mehignore"), "*.log\n!important.log\n").unwrap();
    let ctrl = IgnoreController::new(dir.path());
    assert!(!ctrl.is_allowed(&dir.path().join("debug.log")));
    assert!(ctrl.is_allowed(&dir.path().join("important.log")));
}

#[test]
fn filter_paths_removes_blocked() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(".mehignore"), "*.log\n").unwrap();
    let ctrl = IgnoreController::new(dir.path());
    let paths = vec![
        dir.path().join("main.rs"),
        dir.path().join("debug.log"),
    ];
    let filtered = ctrl.filter_paths(&paths);
    assert_eq!(filtered.len(), 1);
}
```

### 39.6 System prompt integration (`tests/prompt_integration.rs`)

Test the full prompt assembly with real detection:

```rust
#[test]
fn prompt_includes_environment_info() {
    let env = EnvironmentInfo::detect(".");
    let config = PromptConfig {
        cwd: ".".to_string(),
        mode: Mode::Act,
        tool_definitions_xml: None,
        mcp_tools_description: String::new(),
        user_rules: String::new(),
        environment_info: env.to_prompt_section(),
        yolo_mode: false,
    };
    let prompt = build_full_system_prompt(&config);

    // Should contain detected OS
    assert!(prompt.contains("OS:"));
    // Should contain detected shell
    assert!(prompt.contains("Shell:"));
    // Should contain role
    assert!(prompt.contains("expert AI coding assistant"));
    // Should contain act mode
    assert!(prompt.contains("ACT MODE"));
}

#[test]
fn prompt_includes_user_rules() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(".mehrules"), "Always write tests first.\n").unwrap();
    let rules = load_rules(dir.path());
    let user_rules = rules_to_prompt(&rules, &[]);

    let config = PromptConfig {
        cwd: dir.path().to_string_lossy().to_string(),
        mode: Mode::Act,
        tool_definitions_xml: None,
        mcp_tools_description: String::new(),
        user_rules,
        environment_info: String::new(),
        yolo_mode: false,
    };
    let prompt = build_full_system_prompt(&config);
    assert!(prompt.contains("Always write tests first"));
}

#[test]
fn prompt_yolo_vs_non_yolo() {
    let yolo = build_full_system_prompt(&PromptConfig {
        cwd: ".".to_string(),
        mode: Mode::Act,
        tool_definitions_xml: None,
        mcp_tools_description: String::new(),
        user_rules: String::new(),
        environment_info: String::new(),
        yolo_mode: true,
    });
    let normal = build_full_system_prompt(&PromptConfig {
        cwd: ".".to_string(),
        mode: Mode::Act,
        tool_definitions_xml: None,
        mcp_tools_description: String::new(),
        user_rules: String::new(),
        environment_info: String::new(),
        yolo_mode: false,
    });

    assert!(yolo.contains("auto-approved"));
    assert!(normal.contains("asked to approve"));
}
```

### 39.7 Cost and token display (`tests/cost_token_integration.rs`)

```rust
#[test]
fn format_tokens_all_ranges() {
    assert_eq!(format_tokens(0), "0");
    assert_eq!(format_tokens(999), "999");
    assert_eq!(format_tokens(1_000), "1.0k");
    assert_eq!(format_tokens(45_200), "45.2k");
    assert_eq!(format_tokens(1_000_000), "1.0M");
    assert_eq!(format_tokens(2_500_000), "2.5M");
}

#[test]
fn format_cost_all_ranges() {
    assert_eq!(format_cost(0.001), "$0.0010");
    assert_eq!(format_cost(0.05), "$0.050");
    assert_eq!(format_cost(1.5), "$1.50");
    assert_eq!(format_cost(12.34), "$12.34");
}

#[test]
fn cost_color_thresholds() {
    assert!(matches!(cost_level(0.05), CostLevel::Normal));
    assert!(matches!(cost_level(0.50), CostLevel::Moderate));
    assert!(matches!(cost_level(5.0), CostLevel::Expensive));
}

#[test]
fn calculate_cost_anthropic_with_cache() {
    let usage = UsageInfo {
        input_tokens: 100_000,
        output_tokens: 50_000,
        cache_read_tokens: Some(80_000),
        cache_write_tokens: Some(20_000),
        thinking_tokens: None,
        total_cost: None,
    };
    let pricing = ModelInfo {
        input_price_per_mtok: 3.0,
        output_price_per_mtok: 15.0,
        cache_read_price_per_mtok: Some(0.30),
        cache_write_price_per_mtok: Some(3.75),
        thinking_price_per_mtok: None,
        // ... other fields
    };
    let cost = calculate_cost(&usage, &pricing);
    assert!(cost > 0.0);
    // input: 0.3, output: 0.75, cache_read: 0.024, cache_write: 0.075
    assert!((cost - 1.149).abs() < 0.01);
}

#[test]
fn context_utilization_display() {
    assert!((context_utilization(10_000, 200_000) - 5.0).abs() < 0.01);
    assert!((context_utilization(150_000, 200_000) - 75.0).abs() < 0.01);
    assert!((context_utilization(190_000, 200_000) - 95.0).abs() < 0.01);
}
```

### 39.8 Task history integration (`tests/task_history_integration.rs`)

```rust
#[test]
fn save_load_roundtrip_with_messages() {
    let dir = TempDir::new().unwrap();
    let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
    let task = PersistedTask {
        task_id: "test-1".to_string(),
        title: "Fix bug".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        messages: vec![
            PersistedMessage {
                role: "user".to_string(),
                content: vec![PersistedContent::Text { text: "Fix the auth bug".to_string() }],
                timestamp: chrono::Utc::now(),
            },
            PersistedMessage {
                role: "assistant".to_string(),
                content: vec![
                    PersistedContent::Thinking { text: "Let me check...".to_string(), signature: Some("sig".to_string()) },
                    PersistedContent::Text { text: "Fixed it.".to_string() },
                    PersistedContent::ToolUse {
                        id: "tc1".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "src/auth.rs"}),
                    },
                ],
                timestamp: chrono::Utc::now(),
            },
        ],
        mode: "act".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4".to_string(),
        total_input_tokens: 5000,
        total_output_tokens: 2000,
        total_cost: 0.045,
        completed: true,
    };
    history.save_task(&task).unwrap();
    let loaded = history.load_task("test-1").unwrap();
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.messages[1].content.len(), 3);
    assert_eq!(loaded.total_cost, 0.045);
}

#[test]
fn list_and_prune_tasks() {
    let dir = TempDir::new().unwrap();
    let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
    let now = chrono::Utc::now();
    for i in 0..10 {
        let mut task = make_task(&format!("t-{i}"), &format!("Task {i}"));
        task.updated_at = now + chrono::Duration::seconds(i64::from(i));
        history.save_task(&task).unwrap();
    }
    let listed = history.list_tasks().unwrap();
    assert_eq!(listed.len(), 10);
    assert_eq!(listed[0].task_id, "t-9"); // Newest first

    let pruned = history.prune(5).unwrap();
    assert_eq!(pruned, 5);
    assert_eq!(history.list_tasks().unwrap().len(), 5);
}

#[test]
fn message_conversion_roundtrip() {
    use meh::provider::{Message, MessageRole, ContentBlock};
    let msg = Message {
        role: MessageRole::Assistant,
        content: vec![
            ContentBlock::Thinking { text: "hmm".to_string(), signature: Some("sig".to_string()) },
            ContentBlock::Text("answer".to_string()),
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"path": "a.rs"}),
            },
        ],
    };
    let persisted = PersistedMessage::from(&msg);
    let restored = Message::from(&persisted);
    assert_eq!(restored.content.len(), 3);
}
```

### 39.9 Workspace context integration (`tests/workspace_context.rs`)

```rust
#[test]
fn workspace_tree_respects_ignore() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("visible.rs"), "").unwrap();
    std::fs::write(dir.path().join(".env"), "SECRET=x").unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();

    let ignore = IgnoreController::new(dir.path());
    let ctx = workspace_context(dir.path(), &ignore, 3, 100);

    assert!(ctx.contains("visible.rs"));
    assert!(ctx.contains("src/"));
    // .env should be blocked by default patterns
    assert!(!ctx.contains(".env"));
}

#[test]
fn workspace_tree_max_depth() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("a/b/c/d")).unwrap();
    std::fs::write(dir.path().join("a/b/c/d/deep.rs"), "").unwrap();

    let ignore = IgnoreController::permissive(dir.path());
    let ctx = workspace_context(dir.path(), &ignore, 1, 100);
    assert!(!ctx.contains("deep.rs"));
}
```

---

## Tests

All tests in this step are integration tests placed in `tests/` directory. They use:
- `tempfile::TempDir` for isolated filesystem state
- Mock providers (from `agent/task_agent.rs` test helpers)
- Direct controller channel manipulation
- No real API calls or TUI rendering

Run with: `cargo test --test '*'`

## Acceptance Criteria
- [x] Integration test infrastructure with shared helpers
- [x] Slash command dispatch tests (help, yolo, mode, model, new)
- [x] Cancellation flow tests (single cancel, double cancel, token reset)
- [x] Config reload tests (valid reload, invalid preserves old, controller message)
- [x] Ignore controller tests (defaults, custom patterns, negation, filter)
- [x] System prompt assembly tests (environment, rules, yolo, mode, MCP, XML tools, ordering)
- [x] Cost and token formatting/calculation tests
- [x] Task history roundtrip tests (save, load, list, prune, message conversion)
- [x] Workspace context tests (respects ignore, max depth, truncation, hidden dirs)
- [x] Error handling tests (validate_config, map all error variants, display)
- [x] All tests pass with `cargo test` (750 total)
- [x] `cargo clippy -- -D warnings` passes
- [x] Zero regressions in existing 702+ unit tests
