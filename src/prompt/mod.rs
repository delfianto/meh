//! System prompt construction — modular assembly of the full system prompt.
//!
//! The system prompt is not a static string. It is assembled dynamically
//! from multiple sources depending on the current mode, available tools,
//! user rules, and environment context.
//!
//! ```text
//!   build_full_system_prompt()
//!         │
//!         ├── base.rs          ──► core identity and behavioral instructions
//!         ├── tools_section.rs ──► tool definitions (filtered by mode)
//!         ├── rules.rs         ──► user rules from .meh/rules and .mehrules
//!         ├── environment.rs   ──► OS, shell, cwd, language detection
//!         └── context.rs       ──► workspace structure, file tree
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

/// Configuration for building the full modular system prompt.
pub struct PromptConfig {
    /// Current working directory.
    pub cwd: String,
    /// Active mode.
    pub mode: Mode,
    /// XML tool definitions for non-native-tool providers.
    pub tool_definitions_xml: Option<String>,
    /// Description of connected MCP server tools.
    pub mcp_tools_description: String,
    /// Formatted user rules.
    pub user_rules: String,
    /// Formatted environment info section.
    pub environment_info: String,
    /// Whether YOLO mode is active.
    pub yolo_mode: bool,
}

/// Build the complete system prompt from all components.
pub fn build_full_system_prompt(config: &PromptConfig) -> String {
    let mut sections = Vec::new();

    sections.push(base::agent_role());

    if !config.environment_info.is_empty() {
        sections.push(config.environment_info.clone());
    }

    sections.push(base::capabilities(config.mode));

    sections.push(match config.mode {
        Mode::Plan => base::plan_mode_instructions(),
        Mode::Act => base::act_mode_instructions(),
    });

    if let Some(ref xml) = config.tool_definitions_xml {
        sections.push(format!("# Available Tools\n\n{xml}"));
    }

    if !config.mcp_tools_description.is_empty() {
        sections.push(format!(
            "# MCP Server Tools\n\n{}",
            config.mcp_tools_description
        ));
    }

    sections.push(base::behavioral_rules(config.yolo_mode));

    if !config.user_rules.is_empty() {
        sections.push(config.user_rules.clone());
    }

    sections.push(base::file_editing_guidelines());

    let prompt = sections.join("\n\n");
    clean_prompt(&prompt)
}

/// Simple system prompt builder (backward compatible with earlier steps).
pub fn build_system_prompt(cwd: &str, mode: Mode) -> String {
    build_full_system_prompt(&PromptConfig {
        cwd: cwd.to_string(),
        mode,
        tool_definitions_xml: None,
        mcp_tools_description: String::new(),
        user_rules: String::new(),
        environment_info: String::new(),
        yolo_mode: false,
    })
}

/// Remove excessive blank lines (more than 2 consecutive).
fn clean_prompt(prompt: &str) -> String {
    let mut result = String::with_capacity(prompt.len());
    let mut blank_count = 0;
    for line in prompt.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                result.push('\n');
            }
        } else {
            blank_count = 0;
            result.push_str(line);
            result.push('\n');
        }
    }
    result
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

    fn make_config(mode: Mode) -> PromptConfig {
        PromptConfig {
            cwd: "/tmp/project".to_string(),
            mode,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: String::new(),
            environment_info: String::new(),
            yolo_mode: false,
        }
    }

    #[test]
    fn full_prompt_plan_mode() {
        let mut config = make_config(Mode::Plan);
        config.environment_info = "# Environment\n- OS: macOS\n".to_string();
        let prompt = build_full_system_prompt(&config);
        assert!(prompt.contains("expert AI coding assistant"));
        assert!(prompt.contains("PLAN MODE"));
        assert!(prompt.contains("macOS"));
        assert!(!prompt.contains("ACT MODE"));
        assert!(!prompt.contains("YOLO"));
    }

    #[test]
    fn full_prompt_act_mode() {
        let prompt = build_full_system_prompt(&make_config(Mode::Act));
        assert!(prompt.contains("ACT MODE"));
        assert!(!prompt.contains("PLAN MODE"));
    }

    #[test]
    fn full_prompt_with_yolo() {
        let mut config = make_config(Mode::Act);
        config.yolo_mode = true;
        let prompt = build_full_system_prompt(&config);
        assert!(prompt.contains("YOLO mode is enabled"));
    }

    #[test]
    fn full_prompt_with_user_rules() {
        let mut config = make_config(Mode::Act);
        config.user_rules = "# User Rules\n\nAlways use snake_case.\n".to_string();
        let prompt = build_full_system_prompt(&config);
        assert!(prompt.contains("Always use snake_case"));
    }

    #[test]
    fn full_prompt_with_xml_tools() {
        let mut config = make_config(Mode::Act);
        config.tool_definitions_xml =
            Some("<tools><tool name=\"read_file\"></tool></tools>".to_string());
        let prompt = build_full_system_prompt(&config);
        assert!(prompt.contains("Available Tools"));
        assert!(prompt.contains("read_file"));
    }

    #[test]
    fn full_prompt_with_mcp() {
        let mut config = make_config(Mode::Act);
        config.mcp_tools_description = "- github: create_issue, list_prs".to_string();
        let prompt = build_full_system_prompt(&config);
        assert!(prompt.contains("MCP Server Tools"));
        assert!(prompt.contains("github"));
    }

    #[test]
    #[allow(clippy::invisible_characters)]
    fn clean_prompt_removes_excess_blanks() {
        let input = "line1\n\n\n\n\nline2\n";
        let cleaned = clean_prompt(input);
        assert!(!cleaned.contains("\n\n\n\n"));
    }

    #[test]
    fn clean_prompt_preserves_double_blank() {
        let input = "line1\n\nline2\n";
        let cleaned = clean_prompt(input);
        assert!(cleaned.contains("line1\n\nline2"));
    }

    #[test]
    fn sections_order() {
        let mut config = make_config(Mode::Act);
        config.user_rules = "# User Rules\nCustom rule.".to_string();
        config.environment_info = "# Environment\nTest env.".to_string();
        let prompt = build_full_system_prompt(&config);

        let role_pos = prompt.find("expert AI coding assistant").unwrap();
        let env_pos = prompt.find("Test env").unwrap();
        let rules_pos = prompt.find("Custom rule").unwrap();
        let editing_pos = prompt.find("File Editing").unwrap();

        assert!(role_pos < env_pos);
        assert!(env_pos < rules_pos);
        assert!(rules_pos < editing_pos);
    }

    #[test]
    fn backward_compatible_build_system_prompt() {
        let prompt = build_system_prompt("/home/user", Mode::Plan);
        assert!(prompt.contains("PLAN MODE"));
        assert!(prompt.contains("expert AI coding assistant"));
    }

    #[test]
    fn build_system_prompt_act() {
        let prompt = build_system_prompt("/home/user", Mode::Act);
        assert!(prompt.contains("ACT MODE"));
    }

    #[test]
    fn build_system_prompt_not_empty() {
        let prompt = build_system_prompt("/tmp", Mode::Act);
        assert!(!prompt.is_empty());
    }

    #[test]
    fn resolve_default_mode_act() {
        assert_eq!(resolve_default_mode("act"), Mode::Act);
    }

    #[test]
    fn resolve_default_mode_plan() {
        assert_eq!(resolve_default_mode("plan"), Mode::Plan);
    }

    #[test]
    fn resolve_default_mode_plan_then_act() {
        assert_eq!(resolve_default_mode("plan_then_act"), Mode::Plan);
    }

    #[test]
    fn resolve_default_mode_unknown_defaults_to_act() {
        assert_eq!(resolve_default_mode("unknown"), Mode::Act);
        assert_eq!(resolve_default_mode(""), Mode::Act);
    }

    #[test]
    fn excluded_categories_plan() {
        let excluded = excluded_categories_for_mode(Mode::Plan);
        assert_eq!(excluded.len(), 2);
        assert!(excluded.contains(&crate::tool::ToolCategory::FileWrite));
        assert!(excluded.contains(&crate::tool::ToolCategory::Command));
    }

    #[test]
    fn excluded_categories_act() {
        let excluded = excluded_categories_for_mode(Mode::Act);
        assert!(excluded.is_empty());
    }

    #[test]
    fn plan_mode_excludes_write_tools() {
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
    fn act_mode_includes_all_tools() {
        let registry = crate::tool::ToolRegistry::with_defaults();
        let excluded = excluded_categories_for_mode(Mode::Act);
        let act_tools = registry.tool_definitions_filtered(&excluded);
        assert!(act_tools.iter().any(|t| t.name == "write_to_file"));
        assert!(act_tools.iter().any(|t| t.name == "execute_command"));
        assert!(act_tools.iter().any(|t| t.name == "read_file"));
    }
}
