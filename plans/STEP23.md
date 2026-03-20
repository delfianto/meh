# STEP 23 — Sub-Agent Support

## Objective
Implement sub-agent spawning so a TaskAgent can delegate work to a nested agent. Sub-agents have their own conversation context but share permissions.

## Prerequisites
- STEP 07 complete (agent system)

## Detailed Instructions

### 23.1 SubAgent (`src/agent/sub_agent.rs`)

A sub-agent is a lightweight TaskAgent spawned by a parent agent for a specific sub-task:

```rust
//! Sub-agent — nested agent for task delegation.

use crate::agent::task_agent::TaskAgent;
use crate::controller::messages::{ControllerMessage, ToolCallResult};
use crate::provider::{Provider, ModelConfig, ToolDefinition};
use tokio::sync::mpsc;

pub struct SubAgent {
    parent_task_id: String,
    sub_task_id: String,
    agent: TaskAgent,
}

impl SubAgent {
    pub fn new(
        parent_task_id: String,
        prompt: String,
        provider: Box<dyn Provider>,
        system_prompt: String,
        config: ModelConfig,
        tools: Vec<ToolDefinition>,
        ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
    ) -> (Self, mpsc::UnboundedSender<crate::agent::AgentMessage>) {
        let sub_task_id = uuid::Uuid::new_v4().to_string();
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();

        let mut agent = TaskAgent::new(
            sub_task_id.clone(),
            provider,
            system_prompt,
            config,
            tools,
            ctrl_tx,
            agent_rx,
        );

        // Add the delegation prompt as the initial user message
        agent.add_initial_message(prompt);

        (
            Self {
                parent_task_id,
                sub_task_id,
                agent,
            },
            agent_tx,
        )
    }

    /// Run the sub-agent to completion.
    pub async fn run(self) -> anyhow::Result<String> {
        self.agent.run().await?;
        // Return the completion message
        Ok("Sub-task completed".to_string())
    }
}
```

### 23.2 Sub-agent system prompt

Sub-agents receive a modified system prompt that scopes their work:

```rust
fn build_sub_agent_system_prompt(parent_system_prompt: &str, task: &str) -> String {
    format!(
        "{parent_system_prompt}\n\n\
        ## Sub-task Context\n\
        You are a sub-agent spawned to handle a specific sub-task. \
        Focus exclusively on the task described below. \
        When you have completed the task, use the attempt_completion tool \
        to report your results back to the parent agent.\n\n\
        Your task: {task}"
    )
}
```

### 23.3 Add delegate_task tool

The parent agent can spawn sub-agents using a `delegate_task` tool:

```rust
//! delegate_task tool — spawn a sub-agent for a specific sub-task.

use crate::tool::{ToolHandler, ToolCategory, ToolResult};
use async_trait::async_trait;

pub struct DelegateTaskHandler {
    // Reference to agent spawning infrastructure
}

// Tool definition:
// {
//   "name": "delegate_task",
//   "description": "Delegate a sub-task to a separate agent. The sub-agent has its own
//     conversation context but shares your tools and permissions. Use this for independent
//     sub-tasks that don't need your full conversation history.",
//   "input_schema": {
//     "type": "object",
//     "required": ["task"],
//     "properties": {
//       "task": {
//         "type": "string",
//         "description": "Description of the sub-task to delegate"
//       },
//       "context": {
//         "type": "string",
//         "description": "Optional context information the sub-agent needs"
//       }
//     }
//   }
// }
```

### 23.4 Wire into agent loop

In `TaskAgent`, when `delegate_task` tool is called:

```rust
"delegate_task" => {
    let task = arguments["task"].as_str().unwrap_or("");
    let context = arguments.get("context").and_then(|v| v.as_str()).unwrap_or("");

    let prompt = if context.is_empty() {
        task.to_string()
    } else {
        format!("{context}\n\nTask: {task}")
    };

    // Check concurrency limit
    let active_count = self.active_sub_agents.load(Ordering::Relaxed);
    if active_count >= MAX_CONCURRENT_SUB_AGENTS {
        tool_results.push(ToolCallResult {
            tool_use_id: id,
            content: format!("Cannot spawn sub-agent: limit of {MAX_CONCURRENT_SUB_AGENTS} concurrent sub-agents reached. Wait for an existing sub-agent to complete."),
            is_error: true,
        });
        continue;
    }

    // Create sub-agent with same provider + tools
    let sub_system_prompt = build_sub_agent_system_prompt(&self.system_prompt, &prompt);
    let (sub_agent, _sub_tx) = SubAgent::new(
        self.task_id.clone(),
        prompt,
        self.create_sub_provider()?,
        sub_system_prompt,
        self.config.clone(),
        self.tools.clone(),
        self.ctrl_tx.clone(),
    );

    // Track active sub-agent
    self.active_sub_agents.fetch_add(1, Ordering::Relaxed);
    let active_counter = self.active_sub_agents.clone();

    // Run sub-agent and get result
    let result = match sub_agent.run().await {
        Ok(completion) => completion,
        Err(e) => format!("Sub-agent failed: {e}"),
    };

    active_counter.fetch_sub(1, Ordering::Relaxed);

    tool_results.push(ToolCallResult {
        tool_use_id: id,
        content: result,
        is_error: false,
    });
}
```

### 23.5 Concurrency limits and resource management

```rust
/// Maximum number of concurrent sub-agents per parent.
const MAX_CONCURRENT_SUB_AGENTS: u32 = 3;

/// Sub-agent resource tracking.
pub struct SubAgentTracker {
    active: Arc<AtomicU32>,
    /// All spawned sub-agent IDs for this parent.
    spawned_ids: Vec<String>,
}

impl SubAgentTracker {
    pub fn new() -> Self {
        Self {
            active: Arc::new(AtomicU32::new(0)),
            spawned_ids: Vec::new(),
        }
    }

    pub fn can_spawn(&self) -> bool {
        self.active.load(Ordering::Relaxed) < MAX_CONCURRENT_SUB_AGENTS
    }

    pub fn track_spawn(&mut self, sub_task_id: String) -> Arc<AtomicU32> {
        self.active.fetch_add(1, Ordering::Relaxed);
        self.spawned_ids.push(sub_task_id);
        self.active.clone()
    }

    pub fn active_count(&self) -> u32 {
        self.active.load(Ordering::Relaxed)
    }
}
```

### 23.6 Sub-agent permission inheritance

Sub-agents inherit the parent's permission state:

```rust
// When creating a sub-agent, pass the same PermissionController reference
// This means:
// - If parent is in YOLO mode, sub-agent is too
// - If parent has "always allow" for a tool, sub-agent does too
// - Permission prompts from sub-agents appear in the same TUI

// The PermissionController is wrapped in Arc<RwLock<>> and shared:
let shared_permissions = self.permission_controller.clone(); // Arc<RwLock<PermissionController>>
```

### 23.7 Sub-agent UI integration

Sub-agent activity is shown in the TUI with indented, prefixed output:

```
 Assistant: I'll delegate the test writing to a sub-agent.

 [Tool: delegate_task]
 Task: Write unit tests for the parser module

   ┌─ Sub-agent (abc123) ──────────────────┐
   │ I'll write tests for the parser...    │
   │ [Tool: read_file] src/parser.rs       │
   │ [Tool: write_file] src/parser_test.rs │
   │ ✓ Completed: Wrote 8 tests            │
   └────────────────────────────────────────┘

 The sub-agent has written 8 unit tests. Let me verify they pass...
```

Messages from sub-agents are sent through the controller with a `sub_task_id` field so the TUI can render them appropriately:

```rust
/// UI update from a sub-agent.
UiUpdate::SubAgentUpdate {
    parent_task_id: String,
    sub_task_id: String,
    content: String,
}
```

### 23.8 Provider cloning for sub-agents

Sub-agents need their own provider instance (separate conversation state):

```rust
impl TaskAgent {
    /// Create a new provider instance for a sub-agent.
    fn create_sub_provider(&self) -> anyhow::Result<Box<dyn Provider>> {
        // The provider factory creates a fresh instance with the same config
        // but independent conversation state
        self.provider_factory.create()
    }
}

/// Factory for creating provider instances.
pub struct ProviderFactory {
    provider_type: String,
    api_key: String,
    model: String,
    config: ModelConfig,
}

impl ProviderFactory {
    pub fn create(&self) -> anyhow::Result<Box<dyn Provider>> {
        match self.provider_type.as_str() {
            "anthropic" => Ok(Box::new(AnthropicProvider::new(
                &self.api_key, &self.model, &self.config,
            )?)),
            "openai" => Ok(Box::new(OpenAiProvider::new(
                &self.api_key, &self.model, &self.config,
            )?)),
            "gemini" => Ok(Box::new(GeminiProvider::new(
                &self.api_key, &self.model, &self.config,
            )?)),
            "openrouter" => Ok(Box::new(OpenRouterProvider::new(
                &self.api_key, &self.model, &self.config,
            )?)),
            other => anyhow::bail!("Unknown provider: {other}"),
        }
    }
}
```

## Tests

```rust
#[cfg(test)]
mod sub_agent_tests {
    use super::*;
    use crate::provider::mock::MockProvider;
    use crate::provider::StreamChunk;

    #[tokio::test]
    async fn test_sub_agent_creation() {
        let (ctrl_tx, _ctrl_rx) = mpsc::unbounded_channel();
        let provider = MockProvider::new(vec![
            StreamChunk::Text { delta: "Done.".to_string() },
            StreamChunk::Done,
        ]);
        let (sub_agent, _tx) = SubAgent::new(
            "parent-1".to_string(),
            "Fix the bug".to_string(),
            Box::new(provider),
            "system prompt".to_string(),
            ModelConfig::default(),
            vec![],
            ctrl_tx,
        );
        assert_eq!(sub_agent.parent_task_id, "parent-1");
        assert!(!sub_agent.sub_task_id.is_empty());
        assert_ne!(sub_agent.sub_task_id, "parent-1");
    }

    #[tokio::test]
    async fn test_sub_agent_unique_ids() {
        let (ctrl_tx, _ctrl_rx) = mpsc::unbounded_channel();
        let provider1 = MockProvider::new(vec![StreamChunk::Done]);
        let provider2 = MockProvider::new(vec![StreamChunk::Done]);
        let (sa1, _) = SubAgent::new(
            "parent".to_string(), "task 1".to_string(),
            Box::new(provider1), "sys".to_string(),
            ModelConfig::default(), vec![], ctrl_tx.clone(),
        );
        let (sa2, _) = SubAgent::new(
            "parent".to_string(), "task 2".to_string(),
            Box::new(provider2), "sys".to_string(),
            ModelConfig::default(), vec![], ctrl_tx,
        );
        assert_ne!(sa1.sub_task_id, sa2.sub_task_id);
    }

    #[test]
    fn test_sub_agent_system_prompt() {
        let parent_prompt = "You are a helpful coding assistant.";
        let task = "Write tests for parser.rs";
        let result = build_sub_agent_system_prompt(parent_prompt, task);
        assert!(result.contains(parent_prompt));
        assert!(result.contains(task));
        assert!(result.contains("Sub-task Context"));
        assert!(result.contains("attempt_completion"));
    }

    #[test]
    fn test_tracker_can_spawn() {
        let tracker = SubAgentTracker::new();
        assert!(tracker.can_spawn());
        assert_eq!(tracker.active_count(), 0);
    }

    #[test]
    fn test_tracker_limit() {
        let mut tracker = SubAgentTracker::new();
        for i in 0..MAX_CONCURRENT_SUB_AGENTS {
            assert!(tracker.can_spawn());
            tracker.track_spawn(format!("sub-{i}"));
        }
        assert!(!tracker.can_spawn());
        assert_eq!(tracker.active_count(), MAX_CONCURRENT_SUB_AGENTS);
    }

    #[test]
    fn test_tracker_decrement() {
        let mut tracker = SubAgentTracker::new();
        let counter = tracker.track_spawn("sub-1".to_string());
        assert_eq!(tracker.active_count(), 1);
        counter.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(tracker.active_count(), 0);
        assert!(tracker.can_spawn());
    }

    #[test]
    fn test_tracker_records_ids() {
        let mut tracker = SubAgentTracker::new();
        tracker.track_spawn("sub-1".to_string());
        tracker.track_spawn("sub-2".to_string());
        assert_eq!(tracker.spawned_ids.len(), 2);
        assert!(tracker.spawned_ids.contains(&"sub-1".to_string()));
        assert!(tracker.spawned_ids.contains(&"sub-2".to_string()));
    }
}

#[cfg(test)]
mod provider_factory_tests {
    use super::*;

    #[test]
    fn test_factory_unknown_provider() {
        let factory = ProviderFactory {
            provider_type: "unknown".to_string(),
            api_key: "key".to_string(),
            model: "model".to_string(),
            config: ModelConfig::default(),
        };
        assert!(factory.create().is_err());
    }
}

#[cfg(test)]
mod delegate_tool_tests {
    use super::*;

    #[test]
    fn test_delegate_task_input_parsing() {
        let input = serde_json::json!({
            "task": "Write unit tests for parser.rs",
            "context": "The parser module handles JSON parsing with streaming support."
        });
        let task = input["task"].as_str().unwrap();
        let context = input.get("context").and_then(|v| v.as_str()).unwrap_or("");
        assert_eq!(task, "Write unit tests for parser.rs");
        assert!(!context.is_empty());
    }

    #[test]
    fn test_delegate_task_without_context() {
        let input = serde_json::json!({
            "task": "Fix the bug"
        });
        let task = input["task"].as_str().unwrap();
        let context = input.get("context").and_then(|v| v.as_str()).unwrap_or("");
        assert_eq!(task, "Fix the bug");
        assert!(context.is_empty());
    }

    #[test]
    fn test_prompt_construction_with_context() {
        let task = "Write tests";
        let context = "Module handles streaming";
        let prompt = format!("{context}\n\nTask: {task}");
        assert_eq!(prompt, "Module handles streaming\n\nTask: Write tests");
    }

    #[test]
    fn test_prompt_construction_without_context() {
        let task = "Write tests";
        let context = "";
        let prompt = if context.is_empty() {
            task.to_string()
        } else {
            format!("{context}\n\nTask: {task}")
        };
        assert_eq!(prompt, "Write tests");
    }
}
```

## Acceptance Criteria
- [ ] Sub-agents can be spawned from parent agent via delegate_task tool
- [ ] Sub-agents have isolated conversation context
- [ ] Sub-agents share permission state with parent
- [ ] Sub-agent completion message returned as tool result
- [ ] Max 3 concurrent sub-agents enforced
- [ ] Sub-agent errors don't crash parent (returned as error tool result)
- [ ] Sub-agent activity visible in TUI with visual nesting
- [ ] Provider factory creates independent instances for sub-agents
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass
