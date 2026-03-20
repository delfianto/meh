# STEP 16 — Informational Tool Handlers (ask_followup, attempt_completion, plan_mode_respond)

## Objective
Implement the three informational tools that manage the conversation flow rather than performing side effects. After this step, the LLM can ask clarifying questions, signal task completion, and respond in plan mode.

## Prerequisites
- STEP 11 complete (tool system)
- STEP 04 complete (controller)

## Detailed Instructions

### 16.1 AskFollowupHandler (`src/tool/handlers/ask_followup.rs`)

```rust
//! ask_followup_question tool — LLM asks the user for clarification.

pub struct AskFollowupHandler;

// Input schema: { "question": string (required) }

// Implementation:
// - Extract question text
// - Return ToolResponse with the question
// - The agent will receive this and the controller will display it to the user
// - The user's response becomes the tool result
// - This tool does NOT require approval (category = Informational)

// Note: The actual user interaction is handled by the controller/agent loop.
// The tool handler just validates the params and returns the question.
// The agent loop detects "ask_followup_question" and handles it specially:
// - Sends the question to the TUI
// - Waits for user input
// - Returns user input as the tool result
```

### 16.2 AttemptCompletionHandler (`src/tool/handlers/attempt_completion.rs`)

```rust
//! attempt_completion tool — signals that the LLM believes the task is done.

pub struct AttemptCompletionHandler;

// Input schema:
// {
//     "result": string (required) — summary of what was accomplished
//     "command": string (optional) — a command for the user to verify the result
// }

// Implementation:
// - Extract result message
// - If command provided, include it in the response
// - Return ToolResponse with the completion message
// - The agent loop detects "attempt_completion" and:
//   - Displays the result to the user
//   - If command provided, suggests running it
//   - Asks user: "Is this complete? (y/n)"
//   - If yes → task ends
//   - If no → user provides feedback, loop continues
```

### 16.3 PlanModeRespondHandler (`src/tool/handlers/plan_mode_respond.rs`)

```rust
//! plan_mode_respond tool — used in plan mode to present the plan and optionally request mode switch.

pub struct PlanModeRespondHandler;

// Input schema:
// {
//     "response": string (required) — the plan or response text
//     "options": {
//         "switch_to_act": boolean (optional) — request to switch to act mode
//     }
// }

// Implementation:
// - Extract response and options
// - Return ToolResponse with the plan text
// - The agent loop detects "plan_mode_respond" with switch_to_act:
//   - Displays plan to user
//   - Asks: "Approve this plan and switch to act mode? (y/n)"
//   - If approved → controller sends ModeSwitch(Act) to agent
//   - If denied → user provides feedback, plan mode continues
```

### 16.4 Special handling in TaskAgent

Update the TaskAgent (`src/agent/task_agent.rs`) to handle these tools specially:

```rust
// After identifying pending_tool_calls, check for special tools:
for (id, name, arguments) in &pending_tool_calls {
    match name.as_str() {
        "ask_followup_question" => {
            // Send question to controller
            // Wait for user response via AgentMessage
            // Use response as tool result
        }
        "attempt_completion" => {
            // Send completion to controller
            // Wait for user confirmation
            // If confirmed → return TaskResult
            // If denied → use feedback as tool result, continue loop
        }
        "plan_mode_respond" => {
            // Send plan to controller
            // If switch_to_act requested, wait for approval
            // If approved → switch mode, rebuild tools/prompt
        }
        _ => {
            // Normal tool execution flow
        }
    }
}
```

## Tests

```rust
// ask_followup tests
#[cfg(test)]
mod ask_followup_tests {
    use super::*;

    #[tokio::test]
    async fn test_ask_followup() {
        let handler = AskFollowupHandler;
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: false };
        let result = handler.execute(
            serde_json::json!({"question": "What file should I edit?"}),
            &ctx,
        ).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("What file should I edit?"));
    }

    #[tokio::test]
    async fn test_ask_followup_missing_question() {
        let handler = AskFollowupHandler;
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: false };
        let result = handler.execute(serde_json::json!({}), &ctx).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_ask_followup_no_approval_required() {
        let handler = AskFollowupHandler;
        assert!(!handler.requires_approval());
        assert_eq!(handler.category(), ToolCategory::Informational);
    }
}

// attempt_completion tests
#[cfg(test)]
mod attempt_completion_tests {
    use super::*;

    #[tokio::test]
    async fn test_attempt_completion() {
        let handler = AttemptCompletionHandler;
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: false };
        let result = handler.execute(
            serde_json::json!({"result": "Fixed the bug in main.rs"}),
            &ctx,
        ).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Fixed the bug"));
    }

    #[tokio::test]
    async fn test_attempt_completion_with_command() {
        let handler = AttemptCompletionHandler;
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: false };
        let result = handler.execute(
            serde_json::json!({"result": "Fixed it", "command": "cargo test"}),
            &ctx,
        ).await.unwrap();
        assert!(result.content.contains("cargo test"));
    }

    #[test]
    fn test_completion_no_approval() {
        let handler = AttemptCompletionHandler;
        assert!(!handler.requires_approval());
    }
}

// plan_mode_respond tests
#[cfg(test)]
mod plan_mode_tests {
    use super::*;

    #[tokio::test]
    async fn test_plan_mode_respond() {
        let handler = PlanModeRespondHandler;
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: false };
        let result = handler.execute(
            serde_json::json!({"response": "Here is my plan:\n1. Read the file\n2. Fix the bug"}),
            &ctx,
        ).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("plan"));
    }

    #[tokio::test]
    async fn test_plan_mode_with_switch() {
        let handler = PlanModeRespondHandler;
        let ctx = ToolContext { cwd: "/tmp".to_string(), auto_approved: false };
        let result = handler.execute(
            serde_json::json!({
                "response": "Plan ready.",
                "options": {"switch_to_act": true}
            }),
            &ctx,
        ).await.unwrap();
        assert!(result.content.contains("Plan ready"));
    }

    #[test]
    fn test_plan_mode_no_approval() {
        let handler = PlanModeRespondHandler;
        assert!(!handler.requires_approval());
    }
}
```

## Acceptance Criteria
- [x] ask_followup_question extracts question, returns it
- [x] attempt_completion extracts result and optional command
- [x] plan_mode_respond extracts response and switch_to_act flag
- [x] All three tools are Informational category (no approval needed)
- [ ] Agent handles these tools specially (user interaction, mode switch) — deferred to agent integration
- [x] Missing required params return error
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass (15 test cases)

**Completed**: PR #13
