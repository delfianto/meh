//! Task persistence — save and resume conversations.
//!
//! Stores complete task state (messages, metadata, token counts) as JSON
//! files in `~/.meh/history/`. Supports atomic writes (temp + rename),
//! debounced auto-save, listing, pruning, and round-trip conversion
//! between internal `Message` types and the serializable `Persisted*` types.

use crate::provider::{ContentBlock, Message, MessageRole};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

/// Full persisted task state.
#[derive(Debug, Serialize, Deserialize)]
pub struct PersistedTask {
    /// Unique task identifier.
    pub task_id: String,
    /// Human-readable title (auto-generated from first user message).
    pub title: String,
    /// When the task was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the task was last updated.
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Full conversation history.
    pub messages: Vec<PersistedMessage>,
    /// Active mode (plan/act).
    pub mode: String,
    /// Provider name.
    pub provider: String,
    /// Model ID.
    pub model: String,
    /// Cumulative input tokens.
    pub total_input_tokens: u64,
    /// Cumulative output tokens.
    pub total_output_tokens: u64,
    /// Cumulative cost in USD.
    pub total_cost: f64,
    /// Whether the task has completed.
    pub completed: bool,
}

/// Serializable message format.
#[derive(Debug, Serialize, Deserialize)]
pub struct PersistedMessage {
    /// Message role (user/assistant).
    pub role: String,
    /// Content blocks within the message.
    pub content: Vec<PersistedContent>,
    /// When this message was recorded.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Serializable content block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PersistedContent {
    /// Plain text.
    #[serde(rename = "text")]
    Text { text: String },
    /// Thinking/reasoning block.
    #[serde(rename = "thinking")]
    Thinking {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Tool use request.
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool execution result.
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

impl From<&Message> for PersistedMessage {
    fn from(msg: &Message) -> Self {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };

        let content = msg
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(PersistedContent::Text { text: text.clone() }),
                ContentBlock::Thinking { text, signature } => Some(PersistedContent::Thinking {
                    text: text.clone(),
                    signature: signature.clone(),
                }),
                ContentBlock::ToolUse { id, name, input } => Some(PersistedContent::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }),
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => Some(PersistedContent::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                }),
                ContentBlock::Image { .. } => None,
            })
            .collect();

        Self {
            role: role.to_string(),
            content,
            timestamp: chrono::Utc::now(),
        }
    }
}

impl From<&PersistedMessage> for Message {
    fn from(msg: &PersistedMessage) -> Self {
        let role = match msg.role.as_str() {
            "assistant" => MessageRole::Assistant,
            _ => MessageRole::User,
        };

        let content = msg
            .content
            .iter()
            .map(|block| match block {
                PersistedContent::Text { text } => ContentBlock::Text(text.clone()),
                PersistedContent::Thinking { text, signature } => ContentBlock::Thinking {
                    text: text.clone(),
                    signature: signature.clone(),
                },
                PersistedContent::ToolUse { id, name, input } => ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                },
                PersistedContent::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                },
            })
            .collect();

        Self { role, content }
    }
}

/// Summary of a task (without full message history).
#[derive(Debug, Clone)]
pub struct TaskSummary {
    /// Unique task identifier.
    pub task_id: String,
    /// Human-readable title.
    pub title: String,
    /// When the task was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the task was last updated.
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Provider name.
    pub provider: String,
    /// Model ID.
    pub model: String,
    /// Number of messages.
    pub message_count: usize,
    /// Cumulative input tokens.
    pub total_input_tokens: u64,
    /// Cumulative output tokens.
    pub total_output_tokens: u64,
    /// Cumulative cost in USD.
    pub total_cost: f64,
    /// Whether the task has completed.
    pub completed: bool,
}

/// Manages task persistence on disk.
pub struct TaskHistory {
    history_dir: PathBuf,
}

impl TaskHistory {
    /// Create a new `TaskHistory`. Creates the history directory if needed.
    pub fn new(history_dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&history_dir)?;
        Ok(Self { history_dir })
    }

    /// Default history directory: `~/.meh/history/`.
    pub fn default_dir() -> anyhow::Result<PathBuf> {
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
        Ok(home.join(".meh").join("history"))
    }

    /// Save a task to disk atomically (temp file + rename).
    pub fn save_task(&self, task: &PersistedTask) -> anyhow::Result<()> {
        let path = self.history_dir.join(format!("{}.json", task.task_id));
        let json = serde_json::to_string_pretty(task)?;
        let temp_path = self.history_dir.join(format!("{}.json.tmp", task.task_id));
        std::fs::write(&temp_path, &json)?;
        std::fs::rename(&temp_path, &path)?;
        Ok(())
    }

    /// Load a task from disk.
    pub fn load_task(&self, task_id: &str) -> anyhow::Result<PersistedTask> {
        let path = self.history_dir.join(format!("{task_id}.json"));
        let json = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Failed to load task {task_id}: {e}"))?;
        let task: PersistedTask = serde_json::from_str(&json)?;
        Ok(task)
    }

    /// List all saved tasks, sorted by `updated_at` descending (newest first).
    pub fn list_tasks(&self) -> anyhow::Result<Vec<TaskSummary>> {
        let mut summaries = Vec::new();

        for entry in std::fs::read_dir(&self.history_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(json) => match serde_json::from_str::<PersistedTask>(&json) {
                    Ok(task) => {
                        summaries.push(TaskSummary {
                            task_id: task.task_id,
                            title: task.title,
                            created_at: task.created_at,
                            updated_at: task.updated_at,
                            provider: task.provider,
                            model: task.model,
                            message_count: task.messages.len(),
                            total_input_tokens: task.total_input_tokens,
                            total_output_tokens: task.total_output_tokens,
                            total_cost: task.total_cost,
                            completed: task.completed,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "Failed to parse task file");
                    }
                },
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to read task file");
                }
            }
        }

        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(summaries)
    }

    /// Delete a task from disk.
    pub fn delete_task(&self, task_id: &str) -> anyhow::Result<()> {
        let path = self.history_dir.join(format!("{task_id}.json"));
        std::fs::remove_file(&path)
            .map_err(|e| anyhow::anyhow!("Failed to delete task {task_id}: {e}"))?;
        Ok(())
    }

    /// Prune old tasks, keeping only the most recent `keep`.
    pub fn prune(&self, keep: usize) -> anyhow::Result<usize> {
        let tasks = self.list_tasks()?;
        let mut deleted = 0;
        for task in tasks.into_iter().skip(keep) {
            if let Err(e) = self.delete_task(&task.task_id) {
                tracing::warn!(task_id = %task.task_id, error = %e, "Failed to prune task");
            } else {
                deleted += 1;
            }
        }
        Ok(deleted)
    }
}

/// Auto-generates a title from the first user message.
pub fn generate_title(first_message: &str) -> String {
    let title = first_message.lines().next().unwrap_or(first_message).trim();

    if title.len() <= 80 {
        title.to_string()
    } else {
        format!("{}...", &title[..77])
    }
}

/// Debounced auto-saver that runs as a background tokio task.
pub struct AutoSaver {
    tx: mpsc::UnboundedSender<PersistedTask>,
}

impl AutoSaver {
    /// Spawn the auto-saver background task.
    pub fn new(history: TaskHistory) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<PersistedTask>();

        tokio::spawn(async move {
            let mut pending: Option<PersistedTask> = None;
            let debounce = Duration::from_secs(2);

            loop {
                tokio::select! {
                    task = rx.recv() => {
                        match task {
                            Some(task) => pending = Some(task),
                            None => break,
                        }
                    }
                    () = tokio::time::sleep(debounce), if pending.is_some() => {
                        if let Some(task) = pending.take() {
                            if let Err(e) = history.save_task(&task) {
                                tracing::error!(error = %e, "Failed to auto-save task");
                            }
                        }
                    }
                }
            }

            if let Some(task) = pending {
                let _ = history.save_task(&task);
            }
        });

        Self { tx }
    }

    /// Queue a task snapshot for saving. Non-blocking.
    pub fn queue_save(&self, task: PersistedTask) {
        let _ = self.tx.send(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_task(task_id: &str, title: &str) -> PersistedTask {
        PersistedTask {
            task_id: task_id.to_string(),
            title: title.to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            messages: vec![],
            mode: "act".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            completed: false,
        }
    }

    #[test]
    fn persisted_task_roundtrip() {
        let task = PersistedTask {
            task_id: "test-1".to_string(),
            title: "Fix bug".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            messages: vec![
                PersistedMessage {
                    role: "user".to_string(),
                    content: vec![PersistedContent::Text {
                        text: "Fix the bug".to_string(),
                    }],
                    timestamp: chrono::Utc::now(),
                },
                PersistedMessage {
                    role: "assistant".to_string(),
                    content: vec![
                        PersistedContent::Thinking {
                            text: "Let me think...".to_string(),
                            signature: None,
                        },
                        PersistedContent::Text {
                            text: "Fixed it.".to_string(),
                        },
                    ],
                    timestamp: chrono::Utc::now(),
                },
            ],
            mode: "act".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
            total_input_tokens: 500,
            total_output_tokens: 200,
            total_cost: 0.005,
            completed: true,
        };

        let json = serde_json::to_string_pretty(&task).unwrap();
        let loaded: PersistedTask = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.task_id, "test-1");
        assert_eq!(loaded.title, "Fix bug");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.total_input_tokens, 500);
        assert!(loaded.completed);
    }

    #[test]
    fn persisted_content_text_roundtrip() {
        let content = PersistedContent::Text {
            text: "hello world".to_string(),
        };
        let json = serde_json::to_string(&content).unwrap();
        let loaded: PersistedContent = serde_json::from_str(&json).unwrap();
        match loaded {
            PersistedContent::Text { text } => assert_eq!(text, "hello world"),
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn persisted_content_thinking_with_signature() {
        let content = PersistedContent::Thinking {
            text: "hmm".to_string(),
            signature: Some("sig123".to_string()),
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("\"signature\":\"sig123\""));
        let loaded: PersistedContent = serde_json::from_str(&json).unwrap();
        match loaded {
            PersistedContent::Thinking { text, signature } => {
                assert_eq!(text, "hmm");
                assert_eq!(signature, Some("sig123".to_string()));
            }
            _ => panic!("Expected Thinking variant"),
        }
    }

    #[test]
    fn persisted_content_thinking_without_signature() {
        let content = PersistedContent::Thinking {
            text: "hmm".to_string(),
            signature: None,
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(!json.contains("signature"));
    }

    #[test]
    fn persisted_content_tool_use() {
        let content = PersistedContent::ToolUse {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "test.rs"}),
        };
        let json = serde_json::to_string(&content).unwrap();
        let loaded: PersistedContent = serde_json::from_str(&json).unwrap();
        match loaded {
            PersistedContent::ToolUse { id, name, input } => {
                assert_eq!(id, "t1");
                assert_eq!(name, "read_file");
                assert_eq!(input["path"], "test.rs");
            }
            _ => panic!("Expected ToolUse variant"),
        }
    }

    #[test]
    fn persisted_content_tool_result() {
        let content = PersistedContent::ToolResult {
            tool_use_id: "t1".to_string(),
            content: "file contents".to_string(),
            is_error: false,
        };
        let json = serde_json::to_string(&content).unwrap();
        let loaded: PersistedContent = serde_json::from_str(&json).unwrap();
        match loaded {
            PersistedContent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "t1");
                assert_eq!(content, "file contents");
                assert!(!is_error);
            }
            _ => panic!("Expected ToolResult variant"),
        }
    }

    #[test]
    fn message_conversion_user() {
        let msg = Message {
            role: MessageRole::User,
            content: vec![ContentBlock::Text("hello".to_string())],
        };
        let persisted = PersistedMessage::from(&msg);
        assert_eq!(persisted.role, "user");
        assert_eq!(persisted.content.len(), 1);

        let restored = Message::from(&persisted);
        assert_eq!(restored.role, MessageRole::User);
        assert_eq!(restored.content.len(), 1);
    }

    #[test]
    fn message_conversion_assistant_with_thinking() {
        let msg = Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "hmm".to_string(),
                    signature: Some("sig".to_string()),
                },
                ContentBlock::Text("answer".to_string()),
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "test.rs"}),
                },
            ],
        };
        let persisted = PersistedMessage::from(&msg);
        assert_eq!(persisted.role, "assistant");
        assert_eq!(persisted.content.len(), 3);

        let restored = Message::from(&persisted);
        assert_eq!(restored.role, MessageRole::Assistant);
        assert_eq!(restored.content.len(), 3);
    }

    #[test]
    fn message_conversion_skips_images() {
        let msg = Message {
            role: MessageRole::User,
            content: vec![
                ContentBlock::Text("look at this".to_string()),
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: vec![1, 2, 3],
                },
            ],
        };
        let persisted = PersistedMessage::from(&msg);
        assert_eq!(persisted.content.len(), 1);
    }

    #[test]
    fn save_and_load_task() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let task = make_task("t-save", "Test");
        history.save_task(&task).unwrap();
        let loaded = history.load_task("t-save").unwrap();
        assert_eq!(loaded.title, "Test");
        assert_eq!(loaded.provider, "anthropic");
    }

    #[test]
    fn save_overwrites() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let mut task = make_task("t-overwrite", "Version 1");
        history.save_task(&task).unwrap();
        task.title = "Version 2".to_string();
        history.save_task(&task).unwrap();
        let loaded = history.load_task("t-overwrite").unwrap();
        assert_eq!(loaded.title, "Version 2");
    }

    #[test]
    fn load_nonexistent_task() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        assert!(history.load_task("nonexistent").is_err());
    }

    #[test]
    fn list_tasks_empty() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let tasks = history.list_tasks().unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn list_tasks_sorted_by_date() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();

        let now = chrono::Utc::now();
        for i in 0..3 {
            let mut task = make_task(&format!("t-{i}"), &format!("Task {i}"));
            task.updated_at = now + chrono::Duration::seconds(i64::from(i));
            history.save_task(&task).unwrap();
        }

        let tasks = history.list_tasks().unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].task_id, "t-2");
        assert_eq!(tasks[1].task_id, "t-1");
        assert_eq!(tasks[2].task_id, "t-0");
    }

    #[test]
    fn delete_task() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let task = make_task("t-delete", "Delete me");
        history.save_task(&task).unwrap();
        history.delete_task("t-delete").unwrap();
        assert!(history.load_task("t-delete").is_err());
    }

    #[test]
    fn delete_nonexistent_task() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        assert!(history.delete_task("nonexistent").is_err());
    }

    #[test]
    fn prune_keeps_recent() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();

        let now = chrono::Utc::now();
        for i in 0..5 {
            let mut task = make_task(&format!("t-{i}"), &format!("Task {i}"));
            task.updated_at = now + chrono::Duration::seconds(i64::from(i));
            history.save_task(&task).unwrap();
        }

        let deleted = history.prune(3).unwrap();
        assert_eq!(deleted, 2);
        let remaining = history.list_tasks().unwrap();
        assert_eq!(remaining.len(), 3);
        assert_eq!(remaining[0].task_id, "t-4");
        assert_eq!(remaining[1].task_id, "t-3");
        assert_eq!(remaining[2].task_id, "t-2");
    }

    #[test]
    fn generate_title_short() {
        assert_eq!(generate_title("Fix the bug"), "Fix the bug");
    }

    #[test]
    fn generate_title_multiline() {
        assert_eq!(
            generate_title("Fix the bug\nMore details here"),
            "Fix the bug"
        );
    }

    #[test]
    fn generate_title_long() {
        let long_msg = "a".repeat(100);
        let title = generate_title(&long_msg);
        assert_eq!(title.len(), 80);
        assert!(title.ends_with("..."));
    }

    #[test]
    fn history_creates_directory() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("deep/nested/history");
        let _history = TaskHistory::new(nested.clone()).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn atomic_write_no_temp_files_left() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let task = make_task("t-atomic", "Atomic");
        history.save_task(&task).unwrap();

        let has_tmp = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext == "tmp")
            });
        assert!(!has_tmp);
    }
}
