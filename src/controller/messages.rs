//! All messages exchanged between components via channels.

use crate::provider::StreamChunk;
use crate::state::task_state::Mode;
use std::path::PathBuf;

/// Messages sent TO the Controller from any component.
#[derive(Debug, Clone)]
pub enum ControllerMessage {
    /// User submitted text input.
    UserSubmit { text: String, images: Vec<PathBuf> },
    /// User responded to a tool approval prompt.
    ApprovalResponse {
        tool_use_id: String,
        approved: bool,
        always_allow: bool,
    },
    /// User requested task cancellation.
    CancelTask,
    /// User switched mode via UI.
    SwitchMode(Mode),
    /// User toggled thinking visibility.
    ToggleThinking,
    /// User toggled YOLO mode.
    ToggleYolo,
    /// User requested quit.
    Quit,

    /// Streaming chunk from the agent (forwarded from provider).
    StreamChunk(StreamChunk),
    /// Agent requests a tool to be executed (needs approval check).
    ToolCallRequest(ToolCallRequest),
    /// Tool execution completed.
    ToolCallResult(ToolCallResult),
    /// Agent finished its task loop.
    TaskComplete(TaskResult),
    /// Agent encountered an error.
    TaskError(String),

    /// Config file changed on disk — reload it.
    ConfigReload,
    /// MCP settings file changed on disk — reload servers.
    McpReload,

    /// User entered a slash command.
    SlashCommand(crate::commands::SlashCommand, String),
}

/// A tool call that needs approval and execution.
#[derive(Debug, Clone)]
pub struct ToolCallRequest {
    pub tool_use_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    /// Human-readable description for the approval prompt.
    pub description: String,
}

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

/// Final result when a task completes.
#[derive(Debug, Clone)]
pub struct TaskResult {
    pub task_id: String,
    pub completion_message: Option<String>,
    pub total_tokens: u64,
    pub total_cost: f64,
}

/// Messages sent FROM the Controller TO the TUI for rendering.
#[derive(Debug, Clone)]
pub enum UiUpdate {
    /// Append a new chat message.
    AppendMessage {
        role: crate::tui::chat_view::ChatRole,
        content: String,
    },
    /// Update the content of the last assistant message (streaming).
    StreamContent { delta: String },
    /// Finalize the current streaming message.
    StreamEnd,
    /// Show a tool approval prompt.
    ToolApproval {
        tool_use_id: String,
        tool_name: String,
        description: String,
    },
    /// Update status bar fields (only `Some` fields are updated).
    StatusUpdate {
        mode: Option<String>,
        tokens: Option<u64>,
        cost: Option<f64>,
        is_streaming: Option<bool>,
        is_yolo: Option<bool>,
        context_tokens: Option<u64>,
        context_window: Option<u32>,
    },
    /// Show thinking/reasoning content.
    ThinkingContent { delta: String },
    /// Update from a sub-agent.
    SubAgentUpdate {
        parent_task_id: String,
        sub_task_id: String,
        content: String,
    },
    /// Signal that the app should quit.
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::chat_view::ChatRole;

    #[test]
    fn tool_call_request_creation() {
        let req = ToolCallRequest {
            tool_use_id: "tc-123".to_string(),
            tool_name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/src/main.rs"}),
            description: "Read /src/main.rs".to_string(),
        };
        assert_eq!(req.tool_name, "read_file");
        assert_eq!(req.arguments["path"], "/src/main.rs");
    }

    #[test]
    fn tool_call_result_creation() {
        let result = ToolCallResult {
            tool_use_id: "tc-123".to_string(),
            content: "file contents here".to_string(),
            is_error: false,
        };
        assert!(!result.is_error);
    }

    #[test]
    fn task_result_creation() {
        let result = TaskResult {
            task_id: "task-1".to_string(),
            completion_message: Some("All done".to_string()),
            total_tokens: 1000,
            total_cost: 0.05,
        };
        assert_eq!(result.total_tokens, 1000);
    }

    #[test]
    fn ui_update_variants() {
        let updates = vec![
            UiUpdate::AppendMessage {
                role: ChatRole::User,
                content: "hello".to_string(),
            },
            UiUpdate::StreamContent {
                delta: "partial".to_string(),
            },
            UiUpdate::StreamEnd,
            UiUpdate::Quit,
        ];
        assert_eq!(updates.len(), 4);
    }
}
