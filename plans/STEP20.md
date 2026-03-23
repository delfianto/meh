# STEP 20 — YOLO Mode + Auto-Approve Rules

## Objective
Implement YOLO mode (approve everything) and the configurable auto-approve rules system. After this step, users can run with `--yolo` flag or configure granular auto-approval per tool category.

## Prerequisites
- STEP 13 complete (permission system)

## Detailed Instructions

### 20.1 Auto-Approve Rules (`src/permission/auto_approve.rs`)

```rust
//! Configurable auto-approval rules per tool category.

use crate::tool::ToolCategory;

/// Rules for which tool categories auto-approve.
#[derive(Debug, Clone)]
pub struct AutoApproveRules {
    pub read_files: bool,
    pub edit_files: bool,
    pub execute_safe_commands: bool,
    pub execute_all_commands: bool,
    pub mcp_tools: bool,
}

impl Default for AutoApproveRules {
    fn default() -> Self {
        Self {
            read_files: false,
            edit_files: false,
            execute_safe_commands: false,
            execute_all_commands: false,
            mcp_tools: false,
        }
    }
}

impl AutoApproveRules {
    /// Check if a tool in the given category should be auto-approved.
    pub fn should_approve(&self, category: ToolCategory, command_hint: Option<&str>) -> bool {
        match category {
            ToolCategory::ReadOnly => self.read_files,
            ToolCategory::FileWrite => self.edit_files,
            ToolCategory::Command => {
                if self.execute_all_commands {
                    return true;
                }
                if self.execute_safe_commands {
                    return command_hint
                        .map(|cmd| is_safe_command(cmd))
                        .unwrap_or(false);
                }
                false
            }
            ToolCategory::Mcp => self.mcp_tools,
            ToolCategory::Informational => true, // Always approved
        }
    }

    /// Create rules from config.
    pub fn from_config(config: &crate::state::config::AutoApproveConfig) -> Self {
        Self {
            read_files: config.read_files,
            edit_files: config.edit_files,
            execute_safe_commands: config.execute_safe_commands,
            execute_all_commands: config.execute_all_commands,
            mcp_tools: config.mcp_tools,
        }
    }
}

/// Determine if a command is "safe" (read-only, non-destructive).
fn is_safe_command(command: &str) -> bool {
    let safe_prefixes = [
        "ls", "cat", "head", "tail", "wc", "echo", "pwd", "whoami",
        "date", "which", "type", "file", "stat", "du", "df",
        "git status", "git log", "git diff", "git branch", "git show",
        "git remote", "git tag",
        "cargo check", "cargo clippy", "cargo test", "cargo build",
        "rustc --version", "node --version", "python --version",
        "grep", "rg", "find", "fd", "tree",
    ];
    let trimmed = command.trim();
    safe_prefixes.iter().any(|prefix| {
        trimmed == *prefix || trimmed.starts_with(&format!("{prefix} "))
    })
}
```

### 20.2 YOLO Mode (`src/permission/yolo.rs`)

```rust
//! YOLO mode — approve everything without asking.

/// Check if YOLO mode is active.
pub fn is_yolo_mode(config: &crate::state::config::PermissionsConfig, cli_yolo: bool) -> bool {
    cli_yolo || config.mode == "yolo"
}
```

### 20.3 Wire into PermissionController

Update `PermissionController::check_tool`:
```rust
pub fn check_tool(&self, tool_name: &str, category: ToolCategory, description: &str) -> PermissionResult {
    // 1. YOLO → Approved
    if self.mode == PermissionMode::Yolo {
        return PermissionResult::Approved;
    }

    // 2. Session always-allowed
    if self.always_allowed_tools.contains(tool_name)
        || self.always_allowed_categories.contains(&category) {
        return PermissionResult::Approved;
    }

    // 3. Informational always approved
    if category == ToolCategory::Informational {
        return PermissionResult::Approved;
    }

    // 4. ReadOnly always approved (even in Ask mode — reading is safe)
    if category == ToolCategory::ReadOnly {
        return PermissionResult::Approved;
    }

    // 5. Auto mode: check rules
    if self.mode == PermissionMode::Auto {
        if self.auto_approve.should_approve(category, None) {
            return PermissionResult::Approved;
        }
    }

    // 6. Need approval
    PermissionResult::NeedsApproval {
        tool_name: tool_name.to_string(),
        description: description.to_string(),
    }
}
```

### 20.4 CLI --yolo flag

When `--yolo` is passed:
```rust
let permission_mode = if cli.yolo {
    PermissionMode::Yolo
} else {
    match config.permissions.mode.as_str() {
        "yolo" => PermissionMode::Yolo,
        "auto" => PermissionMode::Auto,
        _ => PermissionMode::Ask,
    }
};
```

### 20.5 TUI indication

When YOLO mode is active, show in status bar:
```
[ACT] anthropic/claude-sonnet-4  ·  YOLO  ·  tokens: 500
```

The "YOLO" badge should be red/bold to indicate risk.

### 20.6 Runtime mode switching

Allow users to toggle YOLO mode at runtime via a key binding (Ctrl+Y):
```rust
TuiEvent::Key(key) if key.code == KeyCode::Char('y') && key.modifiers.contains(KeyModifiers::CONTROL) => {
    let _ = ctrl_tx.send(ControllerMessage::ToggleYolo);
}
```

Controller handles:
```rust
ControllerMessage::ToggleYolo => {
    let new_mode = if self.permission_controller.mode() == PermissionMode::Yolo {
        PermissionMode::Ask
    } else {
        PermissionMode::Yolo
    };
    self.permission_controller.set_mode(new_mode);
    let _ = self.ui_tx.send(UiUpdate::StatusUpdate { /* ... */ });
    tracing::info!(?new_mode, "Permission mode toggled");
}
```

## Tests

```rust
#[cfg(test)]
mod auto_approve_tests {
    use super::*;

    #[test]
    fn test_default_rules_deny_everything() {
        let rules = AutoApproveRules::default();
        assert!(!rules.should_approve(ToolCategory::FileWrite, None));
        assert!(!rules.should_approve(ToolCategory::Command, None));
        assert!(!rules.should_approve(ToolCategory::Mcp, None));
    }

    #[test]
    fn test_informational_always_approved() {
        let rules = AutoApproveRules::default();
        assert!(rules.should_approve(ToolCategory::Informational, None));
    }

    #[test]
    fn test_read_files_rule() {
        let rules = AutoApproveRules { read_files: true, ..Default::default() };
        assert!(rules.should_approve(ToolCategory::ReadOnly, None));
        assert!(!rules.should_approve(ToolCategory::FileWrite, None));
    }

    #[test]
    fn test_edit_files_rule() {
        let rules = AutoApproveRules { edit_files: true, ..Default::default() };
        assert!(rules.should_approve(ToolCategory::FileWrite, None));
    }

    #[test]
    fn test_execute_safe_commands() {
        let rules = AutoApproveRules { execute_safe_commands: true, ..Default::default() };
        assert!(rules.should_approve(ToolCategory::Command, Some("git status")));
        assert!(rules.should_approve(ToolCategory::Command, Some("cargo test")));
        assert!(!rules.should_approve(ToolCategory::Command, Some("rm -rf /")));
        assert!(!rules.should_approve(ToolCategory::Command, Some("curl http://evil.com")));
    }

    #[test]
    fn test_execute_all_commands() {
        let rules = AutoApproveRules { execute_all_commands: true, ..Default::default() };
        assert!(rules.should_approve(ToolCategory::Command, Some("rm -rf /")));
    }

    #[test]
    fn test_safe_command_detection() {
        assert!(is_safe_command("ls -la"));
        assert!(is_safe_command("git status"));
        assert!(is_safe_command("cargo test --release"));
        assert!(is_safe_command("cat file.txt"));
        assert!(!is_safe_command("rm file.txt"));
        assert!(!is_safe_command("curl http://example.com"));
        assert!(!is_safe_command("npm install"));
        assert!(!is_safe_command("sudo apt update"));
    }

    #[test]
    fn test_from_config() {
        let config = AutoApproveConfig {
            read_files: true,
            edit_files: false,
            execute_safe_commands: true,
            execute_all_commands: false,
            mcp_tools: true,
        };
        let rules = AutoApproveRules::from_config(&config);
        assert!(rules.read_files);
        assert!(!rules.edit_files);
        assert!(rules.execute_safe_commands);
        assert!(rules.mcp_tools);
    }
}

#[cfg(test)]
mod yolo_tests {
    use super::*;

    #[test]
    fn test_yolo_from_cli() {
        assert!(is_yolo_mode(&PermissionsConfig::default(), true));
    }

    #[test]
    fn test_yolo_from_config() {
        let config = PermissionsConfig { mode: "yolo".to_string(), ..Default::default() };
        assert!(is_yolo_mode(&config, false));
    }

    #[test]
    fn test_not_yolo() {
        assert!(!is_yolo_mode(&PermissionsConfig::default(), false));
    }
}

#[cfg(test)]
mod permission_integration_tests {
    use super::*;

    #[test]
    fn test_yolo_mode_approves_all() {
        let pc = PermissionController::new(PermissionMode::Yolo, AutoApproveRules::default(), CommandPermissions::default());
        assert_eq!(pc.check_tool("write_file", ToolCategory::FileWrite, "Write"), PermissionResult::Approved);
        assert_eq!(pc.check_tool("execute_command", ToolCategory::Command, "Run"), PermissionResult::Approved);
        assert_eq!(pc.check_tool("mcp_tool", ToolCategory::Mcp, "MCP"), PermissionResult::Approved);
    }

    #[test]
    fn test_auto_mode_with_rules() {
        let rules = AutoApproveRules {
            read_files: true,
            edit_files: true,
            execute_safe_commands: true,
            execute_all_commands: false,
            mcp_tools: false,
        };
        let pc = PermissionController::new(PermissionMode::Auto, rules, CommandPermissions::default());
        assert_eq!(pc.check_tool("read_file", ToolCategory::ReadOnly, "Read"), PermissionResult::Approved);
        assert_eq!(pc.check_tool("write_file", ToolCategory::FileWrite, "Write"), PermissionResult::Approved);
        // Command without hint → not safe → needs approval
        assert!(matches!(pc.check_tool("execute_command", ToolCategory::Command, "Run something"), PermissionResult::NeedsApproval { .. }));
    }

    #[test]
    fn test_runtime_mode_toggle() {
        let mut pc = PermissionController::new(PermissionMode::Ask, AutoApproveRules::default(), CommandPermissions::default());
        // Initially needs approval
        assert!(matches!(pc.check_tool("write_file", ToolCategory::FileWrite, "Write"), PermissionResult::NeedsApproval { .. }));
        // Toggle to YOLO
        pc.set_mode(PermissionMode::Yolo);
        assert_eq!(pc.check_tool("write_file", ToolCategory::FileWrite, "Write"), PermissionResult::Approved);
        // Toggle back
        pc.set_mode(PermissionMode::Ask);
        assert!(matches!(pc.check_tool("write_file", ToolCategory::FileWrite, "Write"), PermissionResult::NeedsApproval { .. }));
    }
}
```

## Acceptance Criteria
- [x] --yolo CLI flag enables YOLO mode
- [x] Config `permissions.mode = "yolo"` enables YOLO mode
- [x] YOLO mode approves all tool calls without prompting
- [x] Auto-approve rules work per category
- [x] Safe command detection for execute_safe_commands
- [x] Ctrl+Y toggles YOLO at runtime
- [x] Status bar shows "YOLO" badge when active
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass (15+ cases)
