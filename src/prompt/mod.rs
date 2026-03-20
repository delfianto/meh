//! System prompt construction — modular assembly of the full system prompt.
//!
//! The system prompt is not a static string. It is assembled dynamically
//! from multiple sources depending on the current mode, available tools,
//! user rules, and environment context.
//!
//! ```text
//!   build_system_prompt()
//!         │
//!         ├── base.rs          ──► core identity and behavioral instructions
//!         ├── tools_section.rs ──► tool definitions (filtered by mode)
//!         ├── rules.rs         ──► user rules from .meh/rules and .mehrules
//!         ├── environment.rs   ──► OS, shell, cwd, language detection
//!         └── context.rs       ──► workspace structure, file tree, git status
//!         │
//!         ▼
//!   String (complete system prompt)
//! ```
//!
//! In Plan mode, only read-only tool definitions are injected. In Act mode,
//! the full tool set is included. MCP server tools are appended dynamically
//! based on which servers are currently connected.

pub mod base;
pub mod context;
pub mod environment;
pub mod rules;
pub mod tools_section;

use crate::state::task_state::Mode;

/// Plan mode instructions appended to the system prompt.
const PLAN_MODE_INSTRUCTIONS: &str = "\
You are currently in PLAN MODE. In this mode:
- Analyze the task thoroughly before proposing any changes
- You can read files, list directories, and search code to understand the codebase
- You CANNOT edit files or run commands — those tools are not available
- Present a clear, step-by-step plan
- When your plan is ready, use the plan_mode_respond tool to present it
- Set switch_to_act: true when you want to proceed with implementation";

/// Act mode instructions appended to the system prompt.
const ACT_MODE_INSTRUCTIONS: &str = "\
You are in ACT MODE. You have full access to all tools.
Execute your plan step by step, making file changes and running commands as needed.";

/// Builds the system prompt for the task agent.
///
/// Assembles the base prompt with mode-specific instructions.
/// Full modular assembly (tools, rules, environment, context) will be
/// completed in STEP 37.
pub fn build_system_prompt(cwd: &str, mode: Mode) -> String {
    let mode_instructions = match mode {
        Mode::Plan => PLAN_MODE_INSTRUCTIONS,
        Mode::Act => ACT_MODE_INSTRUCTIONS,
    };

    format!(
        "You are a helpful AI coding assistant running in a terminal.\n\
         The user's working directory is: {cwd}\n\
         Respond concisely and helpfully.\n\n\
         {mode_instructions}"
    )
}

/// Resolve the default mode from a config string.
pub fn resolve_default_mode(config_default: &str) -> Mode {
    match config_default {
        "plan" | "plan_then_act" => Mode::Plan,
        _ => Mode::Act,
    }
}

/// Get the tool categories that should be excluded in a given mode.
pub fn excluded_categories_for_mode(mode: Mode) -> Vec<crate::tool::ToolCategory> {
    match mode {
        Mode::Plan => vec![
            crate::tool::ToolCategory::FileWrite,
            crate::tool::ToolCategory::Command,
        ],
        Mode::Act => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt_plan_mode() {
        let prompt = build_system_prompt("/home/user", Mode::Plan);
        assert!(prompt.contains("PLAN MODE"));
        assert!(prompt.contains("CANNOT edit files"));
        assert!(prompt.contains("/home/user"));
    }

    #[test]
    fn test_build_system_prompt_act_mode() {
        let prompt = build_system_prompt("/home/user", Mode::Act);
        assert!(prompt.contains("ACT MODE"));
        assert!(prompt.contains("full access"));
        assert!(prompt.contains("/home/user"));
    }

    #[test]
    fn test_build_system_prompt_not_empty() {
        let prompt = build_system_prompt("/tmp", Mode::Act);
        assert!(!prompt.is_empty());
    }

    #[test]
    fn test_resolve_default_mode_act() {
        assert_eq!(resolve_default_mode("act"), Mode::Act);
    }

    #[test]
    fn test_resolve_default_mode_plan() {
        assert_eq!(resolve_default_mode("plan"), Mode::Plan);
    }

    #[test]
    fn test_resolve_default_mode_plan_then_act() {
        assert_eq!(resolve_default_mode("plan_then_act"), Mode::Plan);
    }

    #[test]
    fn test_resolve_default_mode_unknown_defaults_to_act() {
        assert_eq!(resolve_default_mode("unknown"), Mode::Act);
        assert_eq!(resolve_default_mode(""), Mode::Act);
    }

    #[test]
    fn test_excluded_categories_plan() {
        let excluded = excluded_categories_for_mode(Mode::Plan);
        assert_eq!(excluded.len(), 2);
        assert!(excluded.contains(&crate::tool::ToolCategory::FileWrite));
        assert!(excluded.contains(&crate::tool::ToolCategory::Command));
    }

    #[test]
    fn test_excluded_categories_act() {
        let excluded = excluded_categories_for_mode(Mode::Act);
        assert!(excluded.is_empty());
    }

    #[test]
    fn test_plan_mode_excludes_write_tools() {
        let registry = crate::tool::ToolRegistry::with_defaults();
        let excluded = excluded_categories_for_mode(Mode::Plan);
        let plan_tools = registry.tool_definitions_filtered(&excluded);
        assert!(plan_tools.iter().all(|t| t.name != "write_to_file"));
        assert!(plan_tools.iter().all(|t| t.name != "apply_patch"));
        assert!(plan_tools.iter().all(|t| t.name != "execute_command"));
        assert!(plan_tools.iter().any(|t| t.name == "read_file"));
        assert!(plan_tools.iter().any(|t| t.name == "search_files"));
    }

    #[test]
    fn test_act_mode_includes_all_tools() {
        let registry = crate::tool::ToolRegistry::with_defaults();
        let excluded = excluded_categories_for_mode(Mode::Act);
        let act_tools = registry.tool_definitions_filtered(&excluded);
        assert!(act_tools.iter().any(|t| t.name == "write_to_file"));
        assert!(act_tools.iter().any(|t| t.name == "execute_command"));
        assert!(act_tools.iter().any(|t| t.name == "read_file"));
    }
}
