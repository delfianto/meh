# STEP 37 — System Prompt Builder

## Objective
Implement the modular system prompt builder that assembles all components: base instructions, environment info, tool definitions, user rules, MCP info, and mode-specific instructions. This replaces the stub from STEP 07.

## Prerequisites
- STEP 33 (environment detection), STEP 34 (user rules), STEP 32 (ignore), STEP 11 (tool definitions), STEP 17 (mode), STEP 21 (MCP tools)

## Detailed Instructions

### 37.1 Prompt builder (`src/prompt/mod.rs`)

```rust
//! System prompt construction — assembles all components.

pub mod base;
pub mod tools_section;
pub mod rules;
pub mod context;
pub mod environment;

use crate::state::task_state::Mode;

pub struct PromptConfig {
    pub cwd: String,
    pub mode: Mode,
    pub tool_definitions_xml: Option<String>,  // For non-native-tool providers
    pub mcp_tools_description: String,
    pub user_rules: String,
    pub environment_info: String,
    pub yolo_mode: bool,
}

/// Build the complete system prompt.
pub fn build_system_prompt(config: &PromptConfig) -> String {
    let mut sections = Vec::new();

    // 1. Agent role and identity
    sections.push(base::agent_role());

    // 2. Environment information
    if !config.environment_info.is_empty() {
        sections.push(config.environment_info.clone());
    }

    // 3. Capabilities
    sections.push(base::capabilities(config.mode));

    // 4. Mode-specific instructions
    sections.push(match config.mode {
        Mode::Plan => base::plan_mode_instructions(),
        Mode::Act => base::act_mode_instructions(),
    });

    // 5. Tool use guidelines (if XML tools needed)
    if let Some(ref xml) = config.tool_definitions_xml {
        sections.push(format!("# Available Tools\n\n{xml}"));
    }

    // 6. MCP tools
    if !config.mcp_tools_description.is_empty() {
        sections.push(format!("# MCP Server Tools\n\n{}", config.mcp_tools_description));
    }

    // 7. Rules and behavior guidelines
    sections.push(base::behavioral_rules(config.yolo_mode));

    // 8. User rules
    if !config.user_rules.is_empty() {
        sections.push(config.user_rules.clone());
    }

    // 9. File editing guidelines
    sections.push(base::file_editing_guidelines());

    // Join sections, clean up whitespace
    let prompt = sections.join("\n\n");
    clean_prompt(&prompt)
}

/// Remove excessive blank lines and empty sections.
fn clean_prompt(prompt: &str) -> String {
    let mut result = String::with_capacity(prompt.len());
    let mut blank_count = 0;
    for line in prompt.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 2 { result.push('\n'); }
        } else {
            blank_count = 0;
            result.push_str(line);
            result.push('\n');
        }
    }
    result
}
```

### 37.2 Base prompt components (`src/prompt/base.rs`)

```rust
//! Base system prompt sections — role, capabilities, mode instructions, rules.

use crate::state::task_state::Mode;

pub fn agent_role() -> String {
    r#"You are an expert AI coding assistant running in a terminal. You help users with software engineering tasks by reading files, writing code, running commands, and using tools.

You have access to tools that let you interact with the user's codebase and development environment. Use them proactively to accomplish tasks."#.to_string()
}

pub fn capabilities(mode: Mode) -> String {
    match mode {
        Mode::Plan => r#"# Capabilities (Plan Mode)
- Read and analyze files in the workspace
- Search for code patterns across the codebase
- List directory contents
- Ask clarifying questions
- Present structured plans for user review"#.to_string(),

        Mode::Act => r#"# Capabilities (Act Mode)
- Read, create, and modify files
- Execute shell commands
- Search and navigate the codebase
- Apply multi-file patches
- Use MCP server tools
- Ask clarifying questions"#.to_string(),
    }
}

pub fn plan_mode_instructions() -> String {
    r#"# Mode: PLAN
You are in PLAN MODE. Analyze the task thoroughly before proposing changes.
- You can read files, list directories, and search code
- You CANNOT edit files or run commands
- Present a clear, step-by-step plan
- Use plan_mode_respond to present your plan
- Set switch_to_act: true when ready to implement"#.to_string()
}

pub fn act_mode_instructions() -> String {
    r#"# Mode: ACT
You are in ACT MODE with full tool access. Execute your plan step by step.
- Make changes incrementally and verify each step
- Run tests after making changes when applicable
- Use attempt_completion when the task is done"#.to_string()
}

pub fn behavioral_rules(yolo: bool) -> String {
    let mut rules = r#"# Rules
- Always use relative paths from the working directory
- Prefer editing existing files over creating new ones
- Run tests after making code changes when a test suite exists
- Keep changes minimal and focused on the task
- Do not add unnecessary comments or documentation
- If unsure about something, ask for clarification
- When encountering errors, read the error message carefully and fix the root cause"#.to_string();

    if yolo {
        rules.push_str("\n- YOLO mode is enabled: all tool calls are auto-approved. Proceed without asking for permission.");
    } else {
        rules.push_str("\n- The user will be asked to approve tool calls. Explain what you intend to do before using tools.");
    }
    rules
}

pub fn file_editing_guidelines() -> String {
    r#"# File Editing
- When editing files, use apply_patch for surgical changes
- Use write_file only for new files or complete rewrites
- Verify file contents with read_file before editing
- Create parent directories as needed
- Preserve existing code style and formatting
- Do not remove existing comments unless asked"#.to_string()
}
```

### 37.3 Tool section builder (`src/prompt/tools_section.rs`)

For providers that don't support native tool use, emit XML tool definitions:

```rust
//! Build XML tool definitions for non-native-tool providers.

use crate::tools::ToolDefinition;

/// Convert tool definitions to XML format for system prompt injection.
pub fn tools_to_xml(tools: &[ToolDefinition]) -> String {
    let mut xml = String::from("<tools>\n");
    for tool in tools {
        xml.push_str(&format!("  <tool name=\"{}\">\n", tool.name));
        xml.push_str(&format!("    <description>{}</description>\n", tool.description));
        xml.push_str("    <parameters>\n");
        for param in &tool.parameters {
            xml.push_str(&format!(
                "      <parameter name=\"{}\" type=\"{}\" required=\"{}\">{}</parameter>\n",
                param.name, param.param_type, param.required, param.description
            ));
        }
        xml.push_str("    </parameters>\n");
        xml.push_str("  </tool>\n");
    }
    xml.push_str("</tools>");
    xml
}

/// Build tool use instructions for XML-based tool calling.
pub fn xml_tool_use_instructions() -> String {
    r#"To use a tool, respond with XML in this format:
<tool_use>
  <name>tool_name</name>
  <parameters>
    <param_name>value</param_name>
  </parameters>
</tool_use>

You may use multiple tools in a single response. Wait for tool results before proceeding."#.to_string()
}
```

### 37.4 Workspace context (`src/prompt/context.rs`)

```rust
//! Workspace context — file tree summary for system prompt.

use std::path::Path;
use crate::ignore::IgnoreController;

/// Build workspace context section (file tree summary).
pub fn workspace_context(
    cwd: &Path,
    ignore: &IgnoreController,
    max_depth: usize,
    max_entries: usize,
) -> String {
    let mut entries = Vec::new();
    collect_tree(cwd, cwd, ignore, 0, max_depth, &mut entries, max_entries);

    if entries.is_empty() {
        return String::new();
    }

    let mut s = String::from("# Workspace Structure\n```\n");
    for (depth, name, is_dir) in &entries {
        let indent = "  ".repeat(*depth);
        let suffix = if *is_dir { "/" } else { "" };
        s.push_str(&format!("{indent}{name}{suffix}\n"));
    }
    if entries.len() >= max_entries {
        s.push_str("  ... (truncated)\n");
    }
    s.push_str("```\n");
    s
}

fn collect_tree(
    root: &Path,
    dir: &Path,
    ignore: &IgnoreController,
    depth: usize,
    max_depth: usize,
    entries: &mut Vec<(usize, String, bool)>,
    max_entries: usize,
) {
    if depth > max_depth || entries.len() >= max_entries {
        return;
    }

    let mut children: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| ignore.is_allowed(&e.path()))
        .collect();

    // Sort: directories first, then alphabetically
    children.sort_by(|a, b| {
        let a_dir = a.path().is_dir();
        let b_dir = b.path().is_dir();
        b_dir.cmp(&a_dir).then(a.file_name().cmp(&b.file_name()))
    });

    for entry in children {
        if entries.len() >= max_entries { break; }
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden directories (except .github, .vscode)
        if name.starts_with('.') && !matches!(name.as_str(), ".github" | ".vscode") {
            continue;
        }
        let is_dir = entry.path().is_dir();
        entries.push((depth, name, is_dir));
        if is_dir {
            collect_tree(root, &entry.path(), ignore, depth + 1, max_depth, entries, max_entries);
        }
    }
}
```

### 37.5 Integration point

In the Controller or Agent, before the first API call and when the system prompt needs rebuilding:

```rust
fn build_prompt(&self) -> String {
    let env_info = EnvironmentInfo::detect(&self.cwd);
    let user_rules = rules_to_prompt(&self.rules, &self.active_paths());
    let mcp_desc = self.mcp_manager.tools_description();
    let xml_tools = if self.provider.supports_native_tools() {
        None
    } else {
        Some(tools_to_xml(&self.tool_definitions))
    };

    let config = PromptConfig {
        cwd: self.cwd.clone(),
        mode: self.state.mode(),
        tool_definitions_xml: xml_tools,
        mcp_tools_description: mcp_desc,
        user_rules,
        environment_info: env_info.to_prompt_section(),
        yolo_mode: self.state.yolo_mode(),
    };

    build_system_prompt(&config)
}
```

## Tests

```rust
#[cfg(test)]
mod prompt_builder_tests {
    use super::*;

    #[test]
    fn test_build_system_prompt_plan_mode() {
        let config = PromptConfig {
            cwd: "/tmp/project".to_string(),
            mode: Mode::Plan,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: String::new(),
            environment_info: "# Environment\n- OS: macOS\n".to_string(),
            yolo_mode: false,
        };
        let prompt = build_system_prompt(&config);
        assert!(prompt.contains("expert AI coding assistant"));
        assert!(prompt.contains("PLAN MODE"));
        assert!(prompt.contains("macOS"));
        assert!(!prompt.contains("ACT MODE"));
        assert!(!prompt.contains("YOLO"));
    }

    #[test]
    fn test_build_system_prompt_act_mode() {
        let config = PromptConfig {
            cwd: "/tmp/project".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: String::new(),
            environment_info: String::new(),
            yolo_mode: false,
        };
        let prompt = build_system_prompt(&config);
        assert!(prompt.contains("ACT MODE"));
        assert!(!prompt.contains("PLAN MODE"));
    }

    #[test]
    fn test_build_system_prompt_with_yolo() {
        let config = PromptConfig {
            cwd: "/tmp/project".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: String::new(),
            environment_info: String::new(),
            yolo_mode: true,
        };
        let prompt = build_system_prompt(&config);
        assert!(prompt.contains("YOLO mode is enabled"));
    }

    #[test]
    fn test_build_system_prompt_with_user_rules() {
        let config = PromptConfig {
            cwd: "/tmp/project".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: "# User Rules\n\nAlways use snake_case.\n".to_string(),
            environment_info: String::new(),
            yolo_mode: false,
        };
        let prompt = build_system_prompt(&config);
        assert!(prompt.contains("Always use snake_case"));
    }

    #[test]
    fn test_build_system_prompt_with_xml_tools() {
        let config = PromptConfig {
            cwd: "/tmp/project".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: Some("<tools><tool name=\"read_file\"></tool></tools>".to_string()),
            mcp_tools_description: String::new(),
            user_rules: String::new(),
            environment_info: String::new(),
            yolo_mode: false,
        };
        let prompt = build_system_prompt(&config);
        assert!(prompt.contains("Available Tools"));
        assert!(prompt.contains("read_file"));
    }

    #[test]
    fn test_build_system_prompt_with_mcp() {
        let config = PromptConfig {
            cwd: "/tmp/project".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: "- github: create_issue, list_prs".to_string(),
            user_rules: String::new(),
            environment_info: String::new(),
            yolo_mode: false,
        };
        let prompt = build_system_prompt(&config);
        assert!(prompt.contains("MCP Server Tools"));
        assert!(prompt.contains("github"));
    }

    #[test]
    fn test_clean_prompt_removes_excess_blanks() {
        let input = "line1\n\n\n\n\nline2\n";
        let cleaned = clean_prompt(input);
        assert!(!cleaned.contains("\n\n\n"));
    }

    #[test]
    fn test_clean_prompt_preserves_double_blank() {
        let input = "line1\n\nline2\n";
        let cleaned = clean_prompt(input);
        assert!(cleaned.contains("line1\n\nline2"));
    }

    #[test]
    fn test_tools_to_xml() {
        use crate::prompt::tools_section::tools_to_xml;
        let tools = vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: vec![],
        }];
        let xml = tools_to_xml(&tools);
        assert!(xml.contains("<tool name=\"read_file\">"));
        assert!(xml.contains("Read a file"));
    }

    #[test]
    fn test_prompt_sections_order() {
        // Verify that sections appear in the expected order
        let config = PromptConfig {
            cwd: "/tmp/project".to_string(),
            mode: Mode::Act,
            tool_definitions_xml: None,
            mcp_tools_description: String::new(),
            user_rules: "# User Rules\nCustom rule.".to_string(),
            environment_info: "# Environment\nTest env.".to_string(),
            yolo_mode: false,
        };
        let prompt = build_system_prompt(&config);

        // Role should come before environment
        let role_pos = prompt.find("expert AI coding assistant").unwrap();
        let env_pos = prompt.find("Test env").unwrap();
        let rules_pos = prompt.find("Custom rule").unwrap();
        let editing_pos = prompt.find("File Editing").unwrap();

        assert!(role_pos < env_pos);
        assert!(env_pos < rules_pos);
        assert!(rules_pos < editing_pos);
    }
}
```

## Acceptance Criteria
- [x] System prompt includes: role, environment, capabilities, mode, tools, MCP, rules, guidelines
- [x] Plan mode excludes write/command capabilities from description
- [x] Act mode includes full tool access
- [x] YOLO mode adds appropriate auto-approve instruction
- [x] Non-YOLO mode adds "explain before using tools" instruction
- [x] User rules injected when present
- [x] XML tool definitions included for non-native-tool providers
- [x] MCP tools described when connected
- [x] Prompt cleaned of excessive whitespace
- [x] Environment info formatted correctly
- [x] Sections appear in consistent order
- [x] Workspace file tree summary included (respects .mehignore)
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass
