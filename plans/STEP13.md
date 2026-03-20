# STEP 13 — Permission System (Ask Mode)

## Objective
Implement the core permission system that gates tool execution. After this step, side-effecting tools require user approval via the TUI before executing. This is the default "ask" mode.

## Prerequisites
- STEP 11-12 complete (tool handlers exist)
- STEP 04 complete (controller message routing)

## Detailed Instructions

### 13.1 PermissionController (`src/permission/mod.rs`)

```rust
//! Permission system — controls which tools can execute without approval.

pub mod command_perms;
pub mod auto_approve;
pub mod yolo;

use crate::tool::ToolCategory;
use std::collections::HashSet;

/// Result of a permission check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionResult {
    /// Tool is approved to execute immediately.
    Approved,
    /// Tool execution was denied.
    Denied { reason: String },
    /// Tool needs user approval before execution.
    NeedsApproval {
        tool_name: String,
        description: String,
    },
}

/// Permission mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    /// Every side-effecting tool requires approval.
    Ask,
    /// Granular auto-approval per category.
    Auto,
    /// Everything auto-approved.
    Yolo,
}

/// Central permission controller.
pub struct PermissionController {
    mode: PermissionMode,
    auto_approve: auto_approve::AutoApproveRules,
    command_perms: command_perms::CommandPermissions,
    /// Tools the user has marked "always allow" during this session.
    always_allowed_tools: HashSet<String>,
    /// Specific tool categories the user has marked "always allow".
    always_allowed_categories: HashSet<ToolCategory>,
}
```

Implement these methods:

```rust
impl PermissionController {
    pub fn new(mode: PermissionMode, auto_approve: AutoApproveRules, command_perms: CommandPermissions) -> Self;

    /// Check if a tool call is permitted.
    pub fn check_tool(&self, tool_name: &str, category: ToolCategory, description: &str) -> PermissionResult {
        // 1. YOLO mode → always Approved
        // 2. Check always_allowed_tools and always_allowed_categories
        // 3. Informational tools (ask_followup, attempt_completion, plan_mode_respond) → always Approved
        // 4. Auto mode → check auto_approve rules by category
        // 5. Ask mode → NeedsApproval for side-effecting tools, Approved for ReadOnly
    }

    /// Check if a specific command is permitted (for execute_command).
    pub fn check_command(&self, command: &str) -> PermissionResult {
        // Delegates to command_perms for pattern matching
        // In Ask mode, always NeedsApproval regardless of pattern match
        // In Auto mode, check if command matches allow patterns
    }

    /// Mark a tool as always allowed for this session.
    pub fn always_allow_tool(&mut self, tool_name: &str);

    /// Mark a category as always allowed for this session.
    pub fn always_allow_category(&mut self, category: ToolCategory);

    /// Get the current mode.
    pub fn mode(&self) -> PermissionMode;

    /// Switch permission mode.
    pub fn set_mode(&mut self, mode: PermissionMode);
}
```

### 13.2 Wire into Controller and ToolExecutor

Update the Controller's `handle_message` for `ControllerMessage::ToolCallRequest`:

```rust
ControllerMessage::ToolCallRequest(req) => {
    // 1. Look up the tool handler in the registry
    // 2. Check permissions via PermissionController
    // 3. If Approved → execute tool, send result to agent
    // 4. If NeedsApproval → send UiUpdate::ToolApproval to TUI, wait for response
    // 5. If Denied → send error ToolCallResult to agent
}
```

Update for `ControllerMessage::ApprovalResponse`:
```rust
ControllerMessage::ApprovalResponse { tool_use_id, approved, always_allow } => {
    // 1. If always_allow → mark in PermissionController
    // 2. If approved → execute tool, send result to agent
    // 3. If denied → send denied ToolCallResult to agent
}
```

### 13.3 Update TUI for approval prompts

When `UiUpdate::ToolApproval` is received:
- Render an inline approval prompt in the chat view
- Show the tool name and description
- Key bindings: `y` = approve, `n` = deny, `a` = always allow this tool type
- Send `ControllerMessage::ApprovalResponse` back

### 13.4 Tool Executor (`src/tool/executor.rs`)

```rust
//! Routes tool calls to handlers and manages execution.

use crate::tool::{ToolHandler, ToolRegistry, ToolContext, ToolResponse};

pub struct ToolExecutor {
    registry: std::sync::Arc<ToolRegistry>,
}

impl ToolExecutor {
    pub fn new(registry: std::sync::Arc<ToolRegistry>) -> Self;

    /// Execute a tool by name with the given arguments.
    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        let handler = self.registry.get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {tool_name}"))?;
        handler.execute(arguments, ctx).await
    }
}
```

## Tests

Cover:
- PermissionController in Ask mode: ReadOnly tools auto-approved, FileWrite/Command need approval
- PermissionController in Yolo mode: everything approved
- PermissionController in Auto mode: respects AutoApproveRules per category
- always_allow_tool works for subsequent checks
- always_allow_category works for all tools in that category
- Informational tools always approved regardless of mode
- ToolExecutor routes to correct handler
- ToolExecutor returns error for unknown tool
- Integration: Controller receives ToolCallRequest → sends ToolApproval → receives ApprovalResponse → sends ToolCallResult

```rust
#[cfg(test)]
mod permission_tests {
    use super::*;

    #[test]
    fn test_ask_mode_readonly_approved() {
        let pc = PermissionController::new(PermissionMode::Ask, AutoApproveRules::default(), CommandPermissions::default());
        let result = pc.check_tool("read_file", ToolCategory::ReadOnly, "Read test.rs");
        assert_eq!(result, PermissionResult::Approved);
    }

    #[test]
    fn test_ask_mode_filewrite_needs_approval() {
        let pc = PermissionController::new(PermissionMode::Ask, AutoApproveRules::default(), CommandPermissions::default());
        let result = pc.check_tool("write_file", ToolCategory::FileWrite, "Write test.rs");
        assert!(matches!(result, PermissionResult::NeedsApproval { .. }));
    }

    #[test]
    fn test_ask_mode_command_needs_approval() {
        let pc = PermissionController::new(PermissionMode::Ask, AutoApproveRules::default(), CommandPermissions::default());
        let result = pc.check_tool("execute_command", ToolCategory::Command, "Run cargo test");
        assert!(matches!(result, PermissionResult::NeedsApproval { .. }));
    }

    #[test]
    fn test_yolo_mode_everything_approved() {
        let pc = PermissionController::new(PermissionMode::Yolo, AutoApproveRules::default(), CommandPermissions::default());
        assert_eq!(pc.check_tool("write_file", ToolCategory::FileWrite, "Write"), PermissionResult::Approved);
        assert_eq!(pc.check_tool("execute_command", ToolCategory::Command, "Run"), PermissionResult::Approved);
    }

    #[test]
    fn test_auto_mode_respects_rules() {
        let rules = AutoApproveRules {
            read_files: true,
            edit_files: true,
            execute_safe_commands: false,
            execute_all_commands: false,
            mcp_tools: false,
        };
        let pc = PermissionController::new(PermissionMode::Auto, rules, CommandPermissions::default());
        assert_eq!(pc.check_tool("read_file", ToolCategory::ReadOnly, "Read"), PermissionResult::Approved);
        assert_eq!(pc.check_tool("write_file", ToolCategory::FileWrite, "Write"), PermissionResult::Approved);
        assert!(matches!(pc.check_tool("execute_command", ToolCategory::Command, "Run"), PermissionResult::NeedsApproval { .. }));
    }

    #[test]
    fn test_informational_always_approved() {
        let pc = PermissionController::new(PermissionMode::Ask, AutoApproveRules::default(), CommandPermissions::default());
        assert_eq!(pc.check_tool("ask_followup_question", ToolCategory::Informational, "Ask"), PermissionResult::Approved);
        assert_eq!(pc.check_tool("attempt_completion", ToolCategory::Informational, "Complete"), PermissionResult::Approved);
    }

    #[test]
    fn test_always_allow_tool() {
        let mut pc = PermissionController::new(PermissionMode::Ask, AutoApproveRules::default(), CommandPermissions::default());
        assert!(matches!(pc.check_tool("write_file", ToolCategory::FileWrite, "Write"), PermissionResult::NeedsApproval { .. }));
        pc.always_allow_tool("write_file");
        assert_eq!(pc.check_tool("write_file", ToolCategory::FileWrite, "Write"), PermissionResult::Approved);
    }

    #[test]
    fn test_always_allow_category() {
        let mut pc = PermissionController::new(PermissionMode::Ask, AutoApproveRules::default(), CommandPermissions::default());
        pc.always_allow_category(ToolCategory::FileWrite);
        assert_eq!(pc.check_tool("write_file", ToolCategory::FileWrite, "Write"), PermissionResult::Approved);
        assert_eq!(pc.check_tool("apply_patch", ToolCategory::FileWrite, "Patch"), PermissionResult::Approved);
    }

    #[test]
    fn test_set_mode() {
        let mut pc = PermissionController::new(PermissionMode::Ask, AutoApproveRules::default(), CommandPermissions::default());
        assert_eq!(pc.mode(), PermissionMode::Ask);
        pc.set_mode(PermissionMode::Yolo);
        assert_eq!(pc.mode(), PermissionMode::Yolo);
    }
}

#[cfg(test)]
mod executor_tests {
    use super::*;

    #[tokio::test]
    async fn test_executor_routes_to_handler() {
        let registry = ToolRegistry::with_defaults();
        let executor = ToolExecutor::new(std::sync::Arc::new(registry));
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: false };
        // read_file with a nonexistent file should return error response (not panic)
        let result = executor.execute("read_file", serde_json::json!({"path": "/nonexistent"}), &ctx).await.unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_executor_unknown_tool() {
        let registry = ToolRegistry::with_defaults();
        let executor = ToolExecutor::new(std::sync::Arc::new(registry));
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: false };
        let result = executor.execute("nonexistent_tool", serde_json::json!({}), &ctx).await;
        assert!(result.is_err());
    }
}
```

## Acceptance Criteria
- [x] Ask mode: ReadOnly+Informational auto-approved, FileWrite/Command/Mcp need approval
- [x] Auto mode: respects AutoApproveRules per category
- [x] Yolo mode: everything approved
- [x] always_allow persists for session (tool-level and category-level)
- [ ] TUI renders approval prompt with y/n/a keybindings (deferred to TUI integration step)
- [ ] Approval flow: request → TUI prompt → response → execute/deny (deferred to TUI integration step)
- [x] ToolExecutor routes to correct handler
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass (26 test cases)

**Completed**: PR #10
