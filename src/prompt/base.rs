//! Base system prompt sections — role, capabilities, mode instructions, rules.

use crate::state::task_state::Mode;

/// Agent identity and role description.
pub fn agent_role() -> String {
    "You are an expert AI coding assistant running in a terminal. You help users \
     with software engineering tasks by reading files, writing code, running \
     commands, and using tools.\n\n\
     You have access to tools that let you interact with the user's codebase \
     and development environment. Use them proactively to accomplish tasks."
        .to_string()
}

/// Mode-appropriate capability list.
pub fn capabilities(mode: Mode) -> String {
    match mode {
        Mode::Plan => "# Capabilities (Plan Mode)\n\
            - Read and analyze files in the workspace\n\
            - Search for code patterns across the codebase\n\
            - List directory contents\n\
            - Ask clarifying questions\n\
            - Present structured plans for user review"
            .to_string(),
        Mode::Act => "# Capabilities (Act Mode)\n\
            - Read, create, and modify files\n\
            - Execute shell commands\n\
            - Search and navigate the codebase\n\
            - Apply multi-file patches\n\
            - Use MCP server tools\n\
            - Ask clarifying questions"
            .to_string(),
    }
}

/// Plan mode instructions.
pub fn plan_mode_instructions() -> String {
    "# Mode: PLAN\n\
     You are in PLAN MODE. Analyze the task thoroughly before proposing changes.\n\
     - You can read files, list directories, and search code\n\
     - You CANNOT edit files or run commands\n\
     - Present a clear, step-by-step plan\n\
     - Use plan_mode_respond to present your plan\n\
     - Set switch_to_act: true when ready to implement"
        .to_string()
}

/// Act mode instructions.
pub fn act_mode_instructions() -> String {
    "# Mode: ACT\n\
     You are in ACT MODE with full tool access. Execute your plan step by step.\n\
     - Make changes incrementally and verify each step\n\
     - Run tests after making changes when applicable\n\
     - Use attempt_completion when the task is done"
        .to_string()
}

/// Behavioral rules, adjusted for YOLO mode.
pub fn behavioral_rules(yolo: bool) -> String {
    let mut rules = "# Rules\n\
        - Always use relative paths from the working directory\n\
        - Prefer editing existing files over creating new ones\n\
        - Run tests after making code changes when a test suite exists\n\
        - Keep changes minimal and focused on the task\n\
        - Do not add unnecessary comments or documentation\n\
        - If unsure about something, ask for clarification\n\
        - When encountering errors, read the error message carefully and fix the root cause"
        .to_string();

    if yolo {
        rules.push_str(
            "\n- YOLO mode is enabled: all tool calls are auto-approved. \
             Proceed without asking for permission.",
        );
    } else {
        rules.push_str(
            "\n- The user will be asked to approve tool calls. \
             Explain what you intend to do before using tools.",
        );
    }
    rules
}

/// File editing guidelines.
pub fn file_editing_guidelines() -> String {
    "# File Editing\n\
     - When editing files, use apply_patch for surgical changes\n\
     - Use write_file only for new files or complete rewrites\n\
     - Verify file contents with read_file before editing\n\
     - Create parent directories as needed\n\
     - Preserve existing code style and formatting\n\
     - Do not remove existing comments unless asked"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_role_content() {
        let role = agent_role();
        assert!(role.contains("expert AI coding assistant"));
        assert!(role.contains("tools"));
    }

    #[test]
    fn capabilities_plan_read_only() {
        let caps = capabilities(Mode::Plan);
        assert!(caps.contains("Plan Mode"));
        assert!(caps.contains("Read and analyze"));
        assert!(!caps.contains("Execute shell"));
    }

    #[test]
    fn capabilities_act_full() {
        let caps = capabilities(Mode::Act);
        assert!(caps.contains("Act Mode"));
        assert!(caps.contains("Execute shell"));
        assert!(caps.contains("modify files"));
    }

    #[test]
    fn plan_mode_restrictions() {
        let inst = plan_mode_instructions();
        assert!(inst.contains("PLAN MODE"));
        assert!(inst.contains("CANNOT edit files"));
    }

    #[test]
    fn act_mode_full_access() {
        let inst = act_mode_instructions();
        assert!(inst.contains("ACT MODE"));
        assert!(inst.contains("full tool access"));
    }

    #[test]
    fn behavioral_rules_yolo() {
        let rules = behavioral_rules(true);
        assert!(rules.contains("YOLO mode is enabled"));
        assert!(rules.contains("auto-approved"));
    }

    #[test]
    fn behavioral_rules_non_yolo() {
        let rules = behavioral_rules(false);
        assert!(rules.contains("asked to approve"));
        assert!(!rules.contains("YOLO"));
    }

    #[test]
    fn file_editing_guidelines_content() {
        let guide = file_editing_guidelines();
        assert!(guide.contains("apply_patch"));
        assert!(guide.contains("write_file"));
    }
}
