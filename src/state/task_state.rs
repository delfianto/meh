//! Per-task mutable state.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Represents the mode of operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Planning mode — read-only tools, analysis.
    Plan,
    /// Acting mode — full tool access.
    Act,
}

/// Current state of a running task.
#[derive(Debug, Clone)]
pub struct TaskState {
    /// Unique task identifier.
    pub task_id: String,
    /// Current operating mode.
    pub mode: Mode,
    /// When the task was started.
    pub started_at: DateTime<Utc>,
    /// Cumulative input tokens across all API calls.
    pub total_input_tokens: u64,
    /// Cumulative output tokens across all API calls.
    pub total_output_tokens: u64,
    /// Cumulative cost in USD.
    pub total_cost: f64,
    /// Number of API calls made.
    pub api_calls: u32,
    /// Number of tool executions.
    pub tools_executed: u32,
    /// Whether the task is currently running.
    pub is_running: bool,
}

impl TaskState {
    /// Create a new task state with zero counters.
    pub fn new(task_id: String, mode: Mode) -> Self {
        Self {
            task_id,
            mode,
            started_at: Utc::now(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            api_calls: 0,
            tools_executed: 0,
            is_running: true,
        }
    }

    /// Accumulate token usage and cost from one API call.
    pub fn record_usage(&mut self, input: u64, output: u64, cost: f64) {
        self.total_input_tokens += input;
        self.total_output_tokens += output;
        self.total_cost += cost;
        self.api_calls += 1;
    }

    /// Record that a tool was executed.
    pub const fn record_tool_execution(&mut self) {
        self.tools_executed += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_state_new() {
        let ts = TaskState::new("test-1".to_string(), Mode::Act);
        assert_eq!(ts.task_id, "test-1");
        assert_eq!(ts.mode, Mode::Act);
        assert!(ts.is_running);
        assert_eq!(ts.total_cost, 0.0);
        assert_eq!(ts.api_calls, 0);
        assert_eq!(ts.tools_executed, 0);
    }

    #[test]
    fn task_state_record_usage() {
        let mut ts = TaskState::new("test-1".to_string(), Mode::Plan);
        ts.record_usage(100, 50, 0.003);
        ts.record_usage(200, 100, 0.005);
        assert_eq!(ts.total_input_tokens, 300);
        assert_eq!(ts.total_output_tokens, 150);
        assert!((ts.total_cost - 0.008).abs() < f64::EPSILON);
        assert_eq!(ts.api_calls, 2);
    }

    #[test]
    fn task_state_record_tool_execution() {
        let mut ts = TaskState::new("test-1".to_string(), Mode::Act);
        ts.record_tool_execution();
        ts.record_tool_execution();
        assert_eq!(ts.tools_executed, 2);
    }

    #[test]
    fn mode_serialization() {
        let plan_json = serde_json::to_string(&Mode::Plan).unwrap();
        assert_eq!(plan_json, "\"plan\"");
        let act_json = serde_json::to_string(&Mode::Act).unwrap();
        assert_eq!(act_json, "\"act\"");
    }

    #[test]
    fn mode_deserialization() {
        let plan: Mode = serde_json::from_str("\"plan\"").unwrap();
        assert_eq!(plan, Mode::Plan);
        let act: Mode = serde_json::from_str("\"act\"").unwrap();
        assert_eq!(act, Mode::Act);
    }
}
