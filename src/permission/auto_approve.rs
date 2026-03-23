//! Configurable auto-approval rules per tool category.
//!
//! Each rule maps to a [`ToolCategory`] and determines whether tools in that
//! category are automatically approved in [`Auto`](super::PermissionMode::Auto)
//! mode. The [`is_safe_command`] helper provides prefix-based detection of
//! read-only, non-destructive shell commands for the `execute_safe_commands`
//! rule.

use crate::state::config::AutoApproveConfig;
use crate::tool::ToolCategory;

/// Per-category auto-approval configuration.
/// When a rule is `true`, tools in that category are auto-approved in Auto mode.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct AutoApproveRules {
    /// Auto-approve read-only tools (`read_file`, `list_files`, `search_files`).
    pub read_files: bool,
    /// Auto-approve file write tools (`write_file`, `apply_patch`).
    pub edit_files: bool,
    /// Auto-approve commands matching safe patterns only.
    pub execute_safe_commands: bool,
    /// Auto-approve all commands regardless of pattern.
    pub execute_all_commands: bool,
    /// Auto-approve MCP server tools.
    pub mcp_tools: bool,
}

impl Default for AutoApproveRules {
    /// Default rules: everything requires approval.
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
                    return command_hint.is_some_and(is_safe_command);
                }
                false
            }
            ToolCategory::Mcp => self.mcp_tools,
            ToolCategory::Informational => true,
        }
    }

    /// Create rules from the deserialized config.
    pub const fn from_config(config: &AutoApproveConfig) -> Self {
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
///
/// Uses prefix matching against a curated list of known-safe commands.
/// A command matches if it equals a prefix exactly or starts with the
/// prefix followed by a space.
pub fn is_safe_command(command: &str) -> bool {
    let safe_prefixes = [
        "ls",
        "cat",
        "head",
        "tail",
        "wc",
        "echo",
        "pwd",
        "whoami",
        "date",
        "which",
        "type",
        "file",
        "stat",
        "du",
        "df",
        "git status",
        "git log",
        "git diff",
        "git branch",
        "git show",
        "git remote",
        "git tag",
        "cargo check",
        "cargo clippy",
        "cargo test",
        "cargo build",
        "rustc --version",
        "node --version",
        "python --version",
        "grep",
        "rg",
        "find",
        "fd",
        "tree",
    ];
    let trimmed = command.trim();
    safe_prefixes
        .iter()
        .any(|prefix| trimmed == *prefix || trimmed.starts_with(&format!("{prefix} ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_rules_deny_everything() {
        let rules = AutoApproveRules::default();
        assert!(!rules.should_approve(ToolCategory::FileWrite, None));
        assert!(!rules.should_approve(ToolCategory::Command, None));
        assert!(!rules.should_approve(ToolCategory::Mcp, None));
        assert!(!rules.should_approve(ToolCategory::ReadOnly, None));
    }

    #[test]
    fn test_informational_always_approved() {
        let rules = AutoApproveRules::default();
        assert!(rules.should_approve(ToolCategory::Informational, None));
    }

    #[test]
    fn test_read_files_rule() {
        let rules = AutoApproveRules {
            read_files: true,
            ..Default::default()
        };
        assert!(rules.should_approve(ToolCategory::ReadOnly, None));
        assert!(!rules.should_approve(ToolCategory::FileWrite, None));
    }

    #[test]
    fn test_edit_files_rule() {
        let rules = AutoApproveRules {
            edit_files: true,
            ..Default::default()
        };
        assert!(rules.should_approve(ToolCategory::FileWrite, None));
    }

    #[test]
    fn test_execute_safe_commands() {
        let rules = AutoApproveRules {
            execute_safe_commands: true,
            ..Default::default()
        };
        assert!(rules.should_approve(ToolCategory::Command, Some("git status")));
        assert!(rules.should_approve(ToolCategory::Command, Some("cargo test")));
        assert!(!rules.should_approve(ToolCategory::Command, Some("rm -rf /")));
        assert!(!rules.should_approve(ToolCategory::Command, Some("curl http://evil.com")));
    }

    #[test]
    fn test_execute_safe_commands_no_hint() {
        let rules = AutoApproveRules {
            execute_safe_commands: true,
            ..Default::default()
        };
        assert!(!rules.should_approve(ToolCategory::Command, None));
    }

    #[test]
    fn test_execute_all_commands() {
        let rules = AutoApproveRules {
            execute_all_commands: true,
            ..Default::default()
        };
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
    fn test_safe_command_exact_match() {
        assert!(is_safe_command("ls"));
        assert!(is_safe_command("pwd"));
        assert!(is_safe_command("whoami"));
    }

    #[test]
    fn test_safe_command_whitespace() {
        assert!(is_safe_command("  ls -la  "));
        assert!(is_safe_command("  git status  "));
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
        assert!(!rules.execute_all_commands);
        assert!(rules.mcp_tools);
    }
}
