//! Sub-agent — nested agent for task delegation.
//!
//! A sub-agent is a lightweight [`TaskAgent`] spawned by a parent agent
//! for a specific sub-task. It gets its own conversation context (messages)
//! but shares permissions and tool definitions with the parent.
//!
//! ```text
//!   Parent TaskAgent
//!       │ delegate_task("write tests")
//!       ▼
//!   SubAgent::new(parent_id, prompt, provider, ...)
//!       │
//!       ▼
//!   SubAgent::run()  →  TaskAgent::run()  →  completion message
//! ```
//!
//! Concurrency is managed by [`SubAgentTracker`], which enforces a
//! per-parent limit of [`MAX_CONCURRENT_SUB_AGENTS`].

use crate::agent::AgentMessage;
use crate::agent::task_agent::TaskAgent;
use crate::controller::messages::ControllerMessage;
use crate::provider::{self, ModelConfig, ToolDefinition};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::mpsc;

/// Maximum number of concurrent sub-agents per parent.
pub const MAX_CONCURRENT_SUB_AGENTS: u32 = 3;

/// Sub-agent — a task-scoped agent spawned by a parent for delegation.
pub struct SubAgent {
    /// ID of the parent task that spawned this sub-agent.
    pub parent_task_id: String,
    /// Unique ID for this sub-task.
    pub sub_task_id: String,
    /// The underlying task agent that drives the conversation.
    agent: TaskAgent,
}

impl SubAgent {
    /// Create a new sub-agent for a delegated sub-task.
    ///
    /// Returns the sub-agent and a sender for delivering `AgentMessage`s
    /// (tool results, cancellation) to it.
    pub fn new(
        parent_task_id: String,
        prompt: String,
        provider: Box<dyn crate::provider::Provider>,
        system_prompt: String,
        config: ModelConfig,
        tools: Vec<ToolDefinition>,
        ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
    ) -> (Self, mpsc::UnboundedSender<AgentMessage>) {
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

        agent.add_user_message(prompt);

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
        Ok("Sub-task completed".to_string())
    }
}

/// Build a system prompt for a sub-agent that scopes its work.
pub fn build_sub_agent_system_prompt(parent_system_prompt: &str, task: &str) -> String {
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

/// Tracks active sub-agents and enforces concurrency limits.
pub struct SubAgentTracker {
    /// Atomic counter of currently active sub-agents.
    active: Arc<AtomicU32>,
    /// IDs of all sub-agents spawned by this parent (for auditing).
    pub spawned_ids: Vec<String>,
}

impl SubAgentTracker {
    /// Create a new tracker with no active sub-agents.
    pub fn new() -> Self {
        Self {
            active: Arc::new(AtomicU32::new(0)),
            spawned_ids: Vec::new(),
        }
    }

    /// Whether another sub-agent can be spawned (under the limit).
    pub fn can_spawn(&self) -> bool {
        self.active.load(Ordering::Relaxed) < MAX_CONCURRENT_SUB_AGENTS
    }

    /// Record a sub-agent spawn and return the shared counter for decrement on completion.
    pub fn track_spawn(&mut self, sub_task_id: String) -> Arc<AtomicU32> {
        self.active.fetch_add(1, Ordering::Relaxed);
        self.spawned_ids.push(sub_task_id);
        self.active.clone()
    }

    /// Current number of active sub-agents.
    pub fn active_count(&self) -> u32 {
        self.active.load(Ordering::Relaxed)
    }
}

impl Default for SubAgentTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Factory for creating provider instances for sub-agents.
///
/// Each sub-agent needs its own provider instance with independent
/// conversation state. The factory stores the configuration needed
/// to create fresh instances on demand.
pub struct ProviderFactory {
    /// Provider type name (e.g., "anthropic", "openai").
    pub provider_type: String,
    /// API key for authentication.
    pub api_key: String,
    /// Optional custom base URL.
    pub base_url: Option<String>,
}

impl ProviderFactory {
    /// Create a fresh provider instance.
    pub fn create(&self) -> anyhow::Result<Box<dyn crate::provider::Provider>> {
        provider::create_provider(&self.provider_type, &self.api_key, self.base_url.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        counter.fetch_sub(1, Ordering::Relaxed);
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

    #[test]
    fn test_tracker_default() {
        let tracker = SubAgentTracker::default();
        assert!(tracker.can_spawn());
        assert_eq!(tracker.active_count(), 0);
    }

    #[test]
    fn test_factory_unknown_provider() {
        let factory = ProviderFactory {
            provider_type: "unknown".to_string(),
            api_key: "key".to_string(),
            base_url: None,
        };
        assert!(factory.create().is_err());
    }

    #[test]
    fn test_delegate_task_input_parsing() {
        let input = serde_json::json!({
            "task": "Write unit tests for parser.rs",
            "context": "The parser module handles JSON parsing."
        });
        let task = input["task"].as_str().unwrap_or("");
        let context = input.get("context").and_then(|v| v.as_str()).unwrap_or("");
        assert_eq!(task, "Write unit tests for parser.rs");
        assert!(!context.is_empty());
    }

    #[test]
    fn test_delegate_task_without_context() {
        let input = serde_json::json!({
            "task": "Fix the bug"
        });
        let task = input["task"].as_str().unwrap_or("");
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
