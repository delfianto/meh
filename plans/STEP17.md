# STEP 17 — Plan/Act Mode Switching

## Objective
Implement the full plan/act mode system. After this step, tasks can start in plan mode, the LLM proposes a plan using only read-only tools, and upon user approval switches to act mode with full tool access.

## Prerequisites
- STEP 16 complete (plan_mode_respond tool)
- STEP 07 complete (agent + controller wiring)
- STEP 11 complete (tool registry with filtering)

## Detailed Instructions

### 17.1 Mode management in Controller

Add to Controller:
```rust
/// Current mode for the active task.
current_mode: Mode,

/// Mode configuration from AppConfig.
mode_config: ModeConfig,
```

When creating a new task:
```rust
fn start_task(&mut self, initial_prompt: String) {
    let mode = match self.mode_config.default.as_str() {
        "plan" | "plan_then_act" => Mode::Plan,
        _ => Mode::Act,
    };
    self.current_mode = mode;

    // Select provider + model based on mode
    let (provider_name, model_id) = self.resolve_model_for_mode(mode);

    // Build tools filtered by mode
    let tools = if mode == Mode::Plan {
        self.tool_registry.tool_definitions_filtered(&[ToolCategory::FileWrite, ToolCategory::Command])
    } else {
        self.tool_registry.tool_definitions()
    };

    // Build system prompt with mode-specific instructions
    let system_prompt = self.build_system_prompt(mode);

    // Create provider and agent...
}
```

### 17.2 Mode-specific system prompt (`src/prompt/mod.rs`)

Add mode-specific instructions:
```rust
pub fn build_system_prompt(cwd: &str, mode: Mode) -> String {
    let base = base_prompt(cwd);
    let mode_instructions = match mode {
        Mode::Plan => PLAN_MODE_INSTRUCTIONS,
        Mode::Act => ACT_MODE_INSTRUCTIONS,
    };
    format!("{base}\n\n{mode_instructions}")
}

const PLAN_MODE_INSTRUCTIONS: &str = r#"
You are currently in PLAN MODE. In this mode:
- Analyze the task thoroughly before proposing any changes
- You can read files, list directories, and search code to understand the codebase
- You CANNOT edit files or run commands — those tools are not available
- Present a clear, step-by-step plan
- When your plan is ready, use the plan_mode_respond tool to present it
- Set switch_to_act: true when you want to proceed with implementation
"#;

const ACT_MODE_INSTRUCTIONS: &str = r#"
You are in ACT MODE. You have full access to all tools.
Execute your plan step by step, making file changes and running commands as needed.
"#;
```

### 17.3 Mode switching in Agent

When the agent receives `AgentMessage::ModeSwitch(new_mode)`:
```rust
AgentMessage::ModeSwitch(new_mode) => {
    self.mode = new_mode;

    // 1. Rebuild tools based on new mode
    self.tools = if new_mode == Mode::Plan {
        self.full_tools.iter()
            .filter(|t| !plan_restricted(t))
            .cloned()
            .collect()
    } else {
        self.full_tools.clone()
    };

    // 2. Optionally switch provider if different model configured
    // (For now, keep same provider — STEP 18 adds per-mode models)

    // 3. Rebuild system prompt
    self.system_prompt = build_system_prompt(&self.cwd, new_mode);

    // 4. Add a system-level message noting the mode switch
    self.messages.push(Message {
        role: MessageRole::User,
        content: vec![ContentBlock::Text(format!(
            "[Mode switched to {}]", if new_mode == Mode::Act { "ACT" } else { "PLAN" }
        ))],
    });
}
```

### 17.4 Handle plan_mode_respond specially in agent

When the agent processes a `plan_mode_respond` tool call:
```rust
"plan_mode_respond" => {
    let response = arguments.get("response").and_then(|v| v.as_str()).unwrap_or("");
    let switch_requested = arguments
        .get("options")
        .and_then(|o| o.get("switch_to_act"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Send plan to controller for display
    let _ = self.ctrl_tx.send(ControllerMessage::StreamChunk(
        StreamChunk::Text { delta: format!("\n Plan:\n{response}\n") }
    ));

    if switch_requested {
        // Ask controller to get user approval
        let _ = self.ctrl_tx.send(ControllerMessage::ToolCallRequest(ToolCallRequest {
            tool_use_id: id.clone(),
            tool_name: "plan_mode_respond".to_string(),
            arguments: arguments.clone(),
            description: "Switch to act mode to implement the plan?".to_string(),
        }));

        // Wait for approval
        let result = wait_for_tool_result(&mut self.rx, &id).await;
        if !result.is_error {
            // Mode switch will come as AgentMessage::ModeSwitch
            // Wait for it
            loop {
                match self.rx.recv().await {
                    Some(AgentMessage::ModeSwitch(mode)) => {
                        // Apply mode switch (rebuild tools, prompt)
                        break;
                    }
                    _ => continue,
                }
            }
            tool_results.push(ToolCallResult {
                tool_use_id: id,
                content: "Switched to act mode. You now have full tool access.".to_string(),
                is_error: false,
            });
        } else {
            tool_results.push(result);
        }
    } else {
        // Just presenting plan, wait for user feedback
        let _ = self.ctrl_tx.send(ControllerMessage::ToolCallRequest(ToolCallRequest {
            tool_use_id: id.clone(),
            tool_name: "plan_mode_respond".to_string(),
            arguments: arguments.clone(),
            description: "Review the plan and provide feedback".to_string(),
        }));
        let result = wait_for_tool_result(&mut self.rx, &id).await;
        tool_results.push(result);
    }
}
```

### 17.5 Update TUI status bar

Show the current mode prominently in the status bar:
- Plan mode: yellow badge `[PLAN]`
- Act mode: green badge `[ACT]`

### 17.6 Strict plan mode

When `strict_plan` is enabled in config:
- Tasks MUST start in plan mode
- User must explicitly approve the plan before any act-mode tools become available
- The `--mode act` CLI flag overrides this

## Tests

```rust
#[cfg(test)]
mod mode_tests {
    use super::*;

    #[test]
    fn test_plan_mode_excludes_write_tools() {
        let registry = ToolRegistry::with_defaults();
        let plan_tools = registry.tool_definitions_filtered(&[ToolCategory::FileWrite, ToolCategory::Command]);
        assert!(plan_tools.iter().all(|t| t.name != "write_file"));
        assert!(plan_tools.iter().all(|t| t.name != "apply_patch"));
        assert!(plan_tools.iter().all(|t| t.name != "execute_command"));
        assert!(plan_tools.iter().any(|t| t.name == "read_file"));
        assert!(plan_tools.iter().any(|t| t.name == "search_files"));
    }

    #[test]
    fn test_act_mode_includes_all_tools() {
        let registry = ToolRegistry::with_defaults();
        let act_tools = registry.tool_definitions();
        assert!(act_tools.iter().any(|t| t.name == "write_file"));
        assert!(act_tools.iter().any(|t| t.name == "execute_command"));
    }

    #[test]
    fn test_system_prompt_plan_mode() {
        let prompt = build_system_prompt("/home/user", Mode::Plan);
        assert!(prompt.contains("PLAN MODE"));
        assert!(prompt.contains("CANNOT edit files"));
    }

    #[test]
    fn test_system_prompt_act_mode() {
        let prompt = build_system_prompt("/home/user", Mode::Act);
        assert!(prompt.contains("ACT MODE"));
        assert!(prompt.contains("full access"));
    }

    #[tokio::test]
    async fn test_mode_switch_via_agent_message() {
        // Create agent in plan mode
        // Send ModeSwitch(Act)
        // Verify tools are updated
    }

    #[test]
    fn test_resolve_model_for_mode() {
        // Test that plan mode uses plan model config
        // Test that act mode uses act model config
        // Test fallback to default when mode-specific not set
    }
}
```

## Acceptance Criteria
- [ ] Tasks can start in Plan or Act mode based on config
- [ ] Plan mode restricts tools to ReadOnly + Informational only
- [ ] plan_mode_respond tool presents plan and optionally requests mode switch
- [ ] Mode switch rebuilds tool set and system prompt
- [ ] Status bar shows current mode
- [ ] Strict plan mode enforced when configured
- [ ] CLI --mode flag works
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass
