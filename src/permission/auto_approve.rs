//! Configurable auto-approval rules per tool category.

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
    /// Default rules: only read-only tools are auto-approved.
    fn default() -> Self {
        Self {
            read_files: true,
            edit_files: false,
            execute_safe_commands: false,
            execute_all_commands: false,
            mcp_tools: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_rules() {
        let rules = AutoApproveRules::default();
        assert!(rules.read_files);
        assert!(!rules.edit_files);
        assert!(!rules.execute_safe_commands);
        assert!(!rules.execute_all_commands);
        assert!(!rules.mcp_tools);
    }

    #[test]
    fn test_custom_rules() {
        let rules = AutoApproveRules {
            read_files: true,
            edit_files: true,
            execute_safe_commands: true,
            execute_all_commands: false,
            mcp_tools: false,
        };
        assert!(rules.edit_files);
        assert!(rules.execute_safe_commands);
        assert!(!rules.execute_all_commands);
    }
}
