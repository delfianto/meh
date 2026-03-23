//! `delegate_task` tool — spawn a sub-agent for a specific sub-task.
//!
//! The delegate task handler creates a sub-agent with its own conversation
//! context, runs it to completion, and returns the result. The sub-agent
//! shares tools and permissions with the parent but has an independent
//! message history.

use crate::agent::sub_agent::{
    ProviderFactory, SubAgent, SubAgentTracker, build_sub_agent_system_prompt,
};
use crate::controller::messages::ControllerMessage;
use crate::provider::{ModelConfig, ToolDefinition};
use crate::tool::{ToolCategory, ToolContext, ToolHandler, ToolResponse};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::{RwLock, mpsc};

/// Handler for delegating sub-tasks to a sub-agent.
pub struct DelegateTaskHandler {
    /// Factory for creating provider instances for sub-agents.
    provider_factory: Arc<ProviderFactory>,
    /// Parent's system prompt (base for sub-agent prompt).
    system_prompt: String,
    /// Model configuration for sub-agents.
    config: ModelConfig,
    /// Tool definitions available to sub-agents.
    tools: Vec<ToolDefinition>,
    /// Channel for sub-agent → controller communication.
    ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
    /// Shared tracker for concurrency limits.
    tracker: Arc<RwLock<SubAgentTracker>>,
    /// Parent task ID.
    parent_task_id: String,
}

impl DelegateTaskHandler {
    /// Create a new delegate task handler with the context needed to spawn sub-agents.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        provider_factory: Arc<ProviderFactory>,
        system_prompt: String,
        config: ModelConfig,
        tools: Vec<ToolDefinition>,
        ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
        tracker: Arc<RwLock<SubAgentTracker>>,
        parent_task_id: String,
    ) -> Self {
        Self {
            provider_factory,
            system_prompt,
            config,
            tools,
            ctrl_tx,
            tracker,
            parent_task_id,
        }
    }
}

#[async_trait]
impl ToolHandler for DelegateTaskHandler {
    fn name(&self) -> &str {
        "delegate_task"
    }

    fn description(&self) -> &str {
        "Delegate a sub-task to a separate agent. The sub-agent has its own \
         conversation context but shares your tools and permissions. Use this \
         for independent sub-tasks that don't need your full conversation history."
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Informational
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["task"],
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Description of the sub-task to delegate"
                },
                "context": {
                    "type": "string",
                    "description": "Optional context information the sub-agent needs"
                }
            }
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResponse> {
        let task = params
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: task"))?;

        let context = params.get("context").and_then(|v| v.as_str()).unwrap_or("");

        let prompt = if context.is_empty() {
            task.to_string()
        } else {
            format!("{context}\n\nTask: {task}")
        };

        {
            let tracker = self.tracker.read().await;
            if !tracker.can_spawn() {
                return Ok(ToolResponse::error(format!(
                    "Cannot spawn sub-agent: limit of {} concurrent sub-agents reached. \
                     Wait for an existing sub-agent to complete.",
                    crate::agent::sub_agent::MAX_CONCURRENT_SUB_AGENTS
                )));
            }
        }

        let provider = match self.provider_factory.create() {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResponse::error(format!(
                    "Failed to create provider: {e}"
                )));
            }
        };

        let sub_system_prompt = build_sub_agent_system_prompt(&self.system_prompt, &prompt);

        let (sub_agent, _sub_tx) = SubAgent::new(
            self.parent_task_id.clone(),
            prompt,
            provider,
            sub_system_prompt,
            self.config.clone(),
            self.tools.clone(),
            self.ctrl_tx.clone(),
        );

        let active_counter = {
            let mut tracker = self.tracker.write().await;
            tracker.track_spawn(sub_agent.sub_task_id.clone())
        };

        let result = match sub_agent.run().await {
            Ok(completion) => completion,
            Err(e) => format!("Sub-agent failed: {e}"),
        };

        active_counter.fetch_sub(1, Ordering::Relaxed);

        Ok(ToolResponse::success(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delegate_task_metadata() {
        let (ctrl_tx, _rx) = mpsc::unbounded_channel();
        let handler = DelegateTaskHandler::new(
            Arc::new(ProviderFactory {
                provider_type: "anthropic".to_string(),
                api_key: "key".to_string(),
                base_url: None,
            }),
            "system".to_string(),
            ModelConfig {
                model_id: "test".to_string(),
                max_tokens: 1024,
                temperature: None,
                thinking_budget: None,
            },
            vec![],
            ctrl_tx,
            Arc::new(RwLock::new(SubAgentTracker::new())),
            "parent-1".to_string(),
        );
        assert_eq!(handler.name(), "delegate_task");
        assert!(!handler.requires_approval());
        assert_eq!(handler.category(), ToolCategory::Informational);
    }

    #[test]
    fn test_delegate_task_schema() {
        let (ctrl_tx, _rx) = mpsc::unbounded_channel();
        let handler = DelegateTaskHandler::new(
            Arc::new(ProviderFactory {
                provider_type: "anthropic".to_string(),
                api_key: "key".to_string(),
                base_url: None,
            }),
            "system".to_string(),
            ModelConfig {
                model_id: "test".to_string(),
                max_tokens: 1024,
                temperature: None,
                thinking_budget: None,
            },
            vec![],
            ctrl_tx,
            Arc::new(RwLock::new(SubAgentTracker::new())),
            "parent-1".to_string(),
        );
        let schema = handler.input_schema();
        assert!(schema["properties"]["task"].is_object());
        assert!(schema["properties"]["context"].is_object());
    }
}
