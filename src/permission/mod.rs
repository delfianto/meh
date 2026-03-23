//! Permission system — controls which tool calls can execute without user approval.
//!
//! Every side-effecting tool call passes through the permission system
//! before execution. Three tiers control the approval behavior:
//!
//! ```text
//!   ToolCallRequest
//!         │
//!         ▼
//!   PermissionController::check()
//!         │
//!         ├── YOLO mode?  ──► auto-approve all
//!         │
//!         ├── Auto-approve rules match?
//!         │     ├── ReadOnly tools  ──► configurable (default: auto)
//!         │     ├── FileWrite tools ──► configurable (default: ask)
//!         │     ├── Command tools   ──► check command_perms patterns
//!         │     └── MCP tools       ──► configurable (default: ask)
//!         │
//!         ├── In always_allowed set? ──► auto-approve
//!         │
//!         └── Otherwise ──► prompt user via TUI
//! ```
//!
//! - `auto_approve` — per-category rules (read=auto, write=ask, etc.)
//! - `command_perms` — glob-based allow/deny patterns for shell commands,
//!   with operator splitting (`&&`, `||`, `|`, `;`) and subshell recursion
//! - `yolo` — bypasses all checks, approves everything

pub mod auto_approve;
pub mod command_perms;
pub mod yolo;

use crate::tool::ToolCategory;
use auto_approve::AutoApproveRules;
use command_perms::CommandPermissions;
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
    auto_approve: AutoApproveRules,
    command_perms: CommandPermissions,
    /// Tools the user has marked "always allow" during this session.
    always_allowed_tools: HashSet<String>,
    /// Specific tool categories the user has marked "always allow".
    always_allowed_categories: HashSet<ToolCategory>,
}

impl PermissionController {
    /// Create a new permission controller with the given mode and rules.
    pub fn new(
        mode: PermissionMode,
        auto_approve: AutoApproveRules,
        command_perms: CommandPermissions,
    ) -> Self {
        Self {
            mode,
            auto_approve,
            command_perms,
            always_allowed_tools: HashSet::new(),
            always_allowed_categories: HashSet::new(),
        }
    }

    /// Check if a tool call is permitted.
    pub fn check_tool(
        &self,
        tool_name: &str,
        category: ToolCategory,
        description: &str,
    ) -> PermissionResult {
        if self.mode == PermissionMode::Yolo {
            return PermissionResult::Approved;
        }

        if category == ToolCategory::Informational {
            return PermissionResult::Approved;
        }

        if self.always_allowed_tools.contains(tool_name) {
            return PermissionResult::Approved;
        }

        if self.always_allowed_categories.contains(&category) {
            return PermissionResult::Approved;
        }

        if category == ToolCategory::ReadOnly {
            return PermissionResult::Approved;
        }

        if self.mode == PermissionMode::Auto && self.auto_approve_category(category) {
            return PermissionResult::Approved;
        }

        PermissionResult::NeedsApproval {
            tool_name: tool_name.to_string(),
            description: description.to_string(),
        }
    }

    /// Check if a specific command is permitted (for `execute_command`).
    pub fn check_command(&self, command: &str) -> PermissionResult {
        if self.mode == PermissionMode::Yolo {
            return PermissionResult::Approved;
        }

        if self
            .always_allowed_categories
            .contains(&ToolCategory::Command)
        {
            return PermissionResult::Approved;
        }

        if self.command_perms.is_denied(command) {
            return PermissionResult::Denied {
                reason: format!("Command matches deny pattern: {command}"),
            };
        }

        if self.mode == PermissionMode::Auto {
            if self.auto_approve.execute_all_commands {
                return PermissionResult::Approved;
            }

            if self.auto_approve.execute_safe_commands && self.command_perms.is_allowed(command) {
                return PermissionResult::Approved;
            }
        }

        PermissionResult::NeedsApproval {
            tool_name: "execute_command".to_string(),
            description: command.to_string(),
        }
    }

    /// Mark a tool as always allowed for this session.
    pub fn always_allow_tool(&mut self, tool_name: &str) {
        self.always_allowed_tools.insert(tool_name.to_string());
    }

    /// Mark a category as always allowed for this session.
    pub fn always_allow_category(&mut self, category: ToolCategory) {
        self.always_allowed_categories.insert(category);
    }

    /// Get the current mode.
    pub const fn mode(&self) -> PermissionMode {
        self.mode
    }

    /// Switch permission mode.
    pub const fn set_mode(&mut self, mode: PermissionMode) {
        self.mode = mode;
    }

    /// Check if a category is auto-approved based on the rules.
    fn auto_approve_category(&self, category: ToolCategory) -> bool {
        self.auto_approve.should_approve(category, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask_controller() -> PermissionController {
        PermissionController::new(
            PermissionMode::Ask,
            AutoApproveRules::default(),
            CommandPermissions::default(),
        )
    }

    #[test]
    fn test_ask_mode_readonly_approved() {
        let pc = ask_controller();
        let result = pc.check_tool("read_file", ToolCategory::ReadOnly, "Read test.rs");
        assert_eq!(result, PermissionResult::Approved);
    }

    #[test]
    fn test_ask_mode_filewrite_needs_approval() {
        let pc = ask_controller();
        let result = pc.check_tool("write_file", ToolCategory::FileWrite, "Write test.rs");
        assert!(matches!(result, PermissionResult::NeedsApproval { .. }));
    }

    #[test]
    fn test_ask_mode_command_needs_approval() {
        let pc = ask_controller();
        let result = pc.check_tool("execute_command", ToolCategory::Command, "Run cargo test");
        assert!(matches!(result, PermissionResult::NeedsApproval { .. }));
    }

    #[test]
    fn test_ask_mode_mcp_needs_approval() {
        let pc = ask_controller();
        let result = pc.check_tool("mcp_search", ToolCategory::Mcp, "Search");
        assert!(matches!(result, PermissionResult::NeedsApproval { .. }));
    }

    #[test]
    fn test_yolo_mode_everything_approved() {
        let pc = PermissionController::new(
            PermissionMode::Yolo,
            AutoApproveRules::default(),
            CommandPermissions::default(),
        );
        assert_eq!(
            pc.check_tool("write_file", ToolCategory::FileWrite, "Write"),
            PermissionResult::Approved
        );
        assert_eq!(
            pc.check_tool("execute_command", ToolCategory::Command, "Run"),
            PermissionResult::Approved
        );
        assert_eq!(
            pc.check_tool("mcp_tool", ToolCategory::Mcp, "MCP"),
            PermissionResult::Approved
        );
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
        let pc =
            PermissionController::new(PermissionMode::Auto, rules, CommandPermissions::default());
        assert_eq!(
            pc.check_tool("read_file", ToolCategory::ReadOnly, "Read"),
            PermissionResult::Approved
        );
        assert_eq!(
            pc.check_tool("write_file", ToolCategory::FileWrite, "Write"),
            PermissionResult::Approved
        );
        assert!(matches!(
            pc.check_tool("execute_command", ToolCategory::Command, "Run"),
            PermissionResult::NeedsApproval { .. }
        ));
    }

    #[test]
    fn test_auto_mode_all_commands() {
        let rules = AutoApproveRules {
            read_files: true,
            edit_files: false,
            execute_safe_commands: false,
            execute_all_commands: true,
            mcp_tools: false,
        };
        let pc =
            PermissionController::new(PermissionMode::Auto, rules, CommandPermissions::default());
        assert_eq!(
            pc.check_tool("execute_command", ToolCategory::Command, "Run"),
            PermissionResult::Approved
        );
    }

    #[test]
    fn test_informational_always_approved() {
        let pc = ask_controller();
        assert_eq!(
            pc.check_tool("ask_followup_question", ToolCategory::Informational, "Ask"),
            PermissionResult::Approved
        );
        assert_eq!(
            pc.check_tool(
                "attempt_completion",
                ToolCategory::Informational,
                "Complete"
            ),
            PermissionResult::Approved
        );
    }

    #[test]
    fn test_always_allow_tool() {
        let mut pc = ask_controller();
        assert!(matches!(
            pc.check_tool("write_file", ToolCategory::FileWrite, "Write"),
            PermissionResult::NeedsApproval { .. }
        ));
        pc.always_allow_tool("write_file");
        assert_eq!(
            pc.check_tool("write_file", ToolCategory::FileWrite, "Write"),
            PermissionResult::Approved
        );
    }

    #[test]
    fn test_always_allow_category() {
        let mut pc = ask_controller();
        pc.always_allow_category(ToolCategory::FileWrite);
        assert_eq!(
            pc.check_tool("write_file", ToolCategory::FileWrite, "Write"),
            PermissionResult::Approved
        );
        assert_eq!(
            pc.check_tool("apply_patch", ToolCategory::FileWrite, "Patch"),
            PermissionResult::Approved
        );
        assert!(matches!(
            pc.check_tool("execute_command", ToolCategory::Command, "Run"),
            PermissionResult::NeedsApproval { .. }
        ));
    }

    #[test]
    fn test_set_mode() {
        let mut pc = ask_controller();
        assert_eq!(pc.mode(), PermissionMode::Ask);
        pc.set_mode(PermissionMode::Yolo);
        assert_eq!(pc.mode(), PermissionMode::Yolo);
    }

    #[test]
    fn test_check_command_yolo() {
        let pc = PermissionController::new(
            PermissionMode::Yolo,
            AutoApproveRules::default(),
            CommandPermissions::default(),
        );
        assert_eq!(pc.check_command("rm -rf /"), PermissionResult::Approved);
    }

    #[test]
    fn test_check_command_ask_always_needs_approval() {
        let perms = CommandPermissions::from_patterns(&["cargo *"], &[]);
        let pc = PermissionController::new(PermissionMode::Ask, AutoApproveRules::default(), perms);
        assert!(matches!(
            pc.check_command("cargo test"),
            PermissionResult::NeedsApproval { .. }
        ));
    }

    #[test]
    fn test_check_command_auto_safe() {
        let perms = CommandPermissions::from_patterns(&["cargo *", "git *"], &[]);
        let rules = AutoApproveRules {
            read_files: true,
            edit_files: false,
            execute_safe_commands: true,
            execute_all_commands: false,
            mcp_tools: false,
        };
        let pc = PermissionController::new(PermissionMode::Auto, rules, perms);
        assert_eq!(pc.check_command("cargo test"), PermissionResult::Approved);
        assert_eq!(pc.check_command("git status"), PermissionResult::Approved);
        assert!(matches!(
            pc.check_command("rm -rf /"),
            PermissionResult::NeedsApproval { .. }
        ));
    }

    #[test]
    fn test_check_command_deny_overrides() {
        let perms = CommandPermissions::from_patterns(&["cargo *"], &["cargo publish*"]);
        let rules = AutoApproveRules {
            read_files: true,
            edit_files: false,
            execute_safe_commands: true,
            execute_all_commands: false,
            mcp_tools: false,
        };
        let pc = PermissionController::new(PermissionMode::Auto, rules, perms);
        assert_eq!(pc.check_command("cargo test"), PermissionResult::Approved);
        assert_eq!(
            pc.check_command("cargo publish"),
            PermissionResult::Denied {
                reason: "Command matches deny pattern: cargo publish".to_string()
            }
        );
    }

    #[test]
    fn test_check_command_category_always_allowed() {
        let mut pc = ask_controller();
        pc.always_allow_category(ToolCategory::Command);
        assert_eq!(pc.check_command("rm -rf /"), PermissionResult::Approved);
    }
}
