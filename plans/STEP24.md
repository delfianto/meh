# STEP 24 — Task History (Save/Resume)

## Objective
Implement task persistence so conversations can be saved and resumed later. After this step, users can close the app and resume a previous task.

## Prerequisites
- STEP 02 complete (state management)
- STEP 07 complete (agent system)

## Detailed Instructions

### 24.1 Task serialization (`src/state/history.rs`)

Define the full task state that gets persisted:

```rust
//! Task persistence — save and resume conversations.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Full persisted task state.
#[derive(Debug, Serialize, Deserialize)]
pub struct PersistedTask {
    pub task_id: String,
    pub title: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub messages: Vec<PersistedMessage>,
    pub mode: String,
    pub provider: String,
    pub model: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost: f64,
    pub completed: bool,
}

/// Serializable message format.
#[derive(Debug, Serialize, Deserialize)]
pub struct PersistedMessage {
    pub role: String,
    pub content: Vec<PersistedContent>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PersistedContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}
```

### 24.2 Conversion functions

```rust
use crate::provider::{Message, ContentBlock, MessageRole};

impl From<&Message> for PersistedMessage {
    fn from(msg: &Message) -> Self {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
        };

        let content = msg.content.iter().map(|block| match block {
            ContentBlock::Text(text) => PersistedContent::Text { text: text.clone() },
            ContentBlock::Thinking { text, signature } => PersistedContent::Thinking {
                text: text.clone(),
                signature: signature.clone(),
            },
            ContentBlock::ToolUse { id, name, input } => PersistedContent::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            },
            ContentBlock::ToolResult { tool_use_id, content, is_error } => PersistedContent::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: content.clone(),
                is_error: *is_error,
            },
        }).collect();

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
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            _ => MessageRole::User, // fallback
        };

        let content = msg.content.iter().map(|block| match block {
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
            PersistedContent::ToolResult { tool_use_id, content, is_error } => ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: content.clone(),
                is_error: *is_error,
            },
        }).collect();

        Self { role, content }
    }
}
```

### 24.3 TaskHistory manager

```rust
/// Manages task persistence on disk.
pub struct TaskHistory {
    history_dir: PathBuf,
}

impl TaskHistory {
    pub fn new(history_dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&history_dir)?;
        Ok(Self { history_dir })
    }

    /// Default history directory: ~/.meh/history/
    pub fn default_dir() -> anyhow::Result<PathBuf> {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
        Ok(home.join(".meh").join("history"))
    }

    /// Save a task to disk.
    pub fn save_task(&self, task: &PersistedTask) -> anyhow::Result<()> {
        let path = self.history_dir.join(format!("{}.json", task.task_id));
        let json = serde_json::to_string_pretty(task)?;
        // Write atomically: write to temp file then rename
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

    /// List all saved tasks, sorted by updated_at descending (newest first).
    pub fn list_tasks(&self) -> anyhow::Result<Vec<TaskSummary>> {
        let mut summaries = Vec::new();

        for entry in std::fs::read_dir(&self.history_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // Read only the metadata we need (not full messages)
            match std::fs::read_to_string(&path) {
                Ok(json) => {
                    match serde_json::from_str::<PersistedTask>(&json) {
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
                    }
                }
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

    /// Prune old tasks, keeping only the most recent N.
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

/// Summary of a task (without full message history).
#[derive(Debug, Clone)]
pub struct TaskSummary {
    pub task_id: String,
    pub title: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub provider: String,
    pub model: String,
    pub message_count: usize,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost: f64,
    pub completed: bool,
}
```

### 24.4 Auto-save with debouncing

Save after each API call completes, but debounce to avoid excessive disk writes:

```rust
use tokio::sync::mpsc;
use std::time::Duration;

pub struct AutoSaver {
    tx: mpsc::UnboundedSender<PersistedTask>,
}

impl AutoSaver {
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
                            None => break, // Channel closed
                        }
                    }
                    _ = tokio::time::sleep(debounce), if pending.is_some() => {
                        if let Some(task) = pending.take() {
                            if let Err(e) = history.save_task(&task) {
                                tracing::error!(error = %e, "Failed to auto-save task");
                            }
                        }
                    }
                }
            }

            // Save any pending task on shutdown
            if let Some(task) = pending {
                let _ = history.save_task(&task);
            }
        });

        Self { tx }
    }

    pub fn queue_save(&self, task: PersistedTask) {
        let _ = self.tx.send(task);
    }
}
```

### 24.5 Resume task flow

```rust
impl Controller {
    /// Resume a previously saved task.
    pub async fn resume_task(&mut self, task_id: &str) -> anyhow::Result<()> {
        let persisted = self.history.load_task(task_id)?;

        // Convert messages back to internal format
        let messages: Vec<Message> = persisted.messages.iter()
            .map(Message::from)
            .collect();

        // Restore token counters
        self.token_tracker.set(
            persisted.total_input_tokens,
            persisted.total_output_tokens,
        );

        // Create agent with existing messages
        let agent = TaskAgent::with_history(
            persisted.task_id.clone(),
            messages,
            self.create_provider(&persisted.provider, &persisted.model)?,
            self.system_prompt.clone(),
            self.model_config.clone(),
            self.tool_definitions.clone(),
            self.ctrl_tx.clone(),
        );

        // Render existing messages in TUI
        for msg in &persisted.messages {
            self.ui_tx.send(UiUpdate::AppendMessage {
                role: msg.role.clone(),
                content: msg.content.iter().filter_map(|c| match c {
                    PersistedContent::Text { text } => Some(text.clone()),
                    _ => None,
                }).collect::<Vec<_>>().join("\n"),
            })?;
        }

        // Show resume indicator
        self.ui_tx.send(UiUpdate::StatusUpdate {
            message: format!("Resumed task: {}", persisted.title),
        })?;

        // Start agent loop — it will wait for user input
        self.spawn_agent(agent);

        Ok(())
    }
}
```

### 24.6 CLI --resume flag

```rust
/// CLI argument for resuming a task.
#[derive(clap::Parser)]
pub struct Cli {
    /// Initial prompt
    pub prompt: Option<String>,

    /// Resume a previous task by ID
    #[arg(long)]
    pub resume: Option<String>,

    /// List recent tasks
    #[arg(long)]
    pub history: bool,

    // ... other args
}

// In main:
if cli.history {
    // List tasks and exit
    let history = TaskHistory::new(TaskHistory::default_dir()?)?;
    let tasks = history.list_tasks()?;
    for task in tasks.iter().take(20) {
        println!(
            "{} | {} | {} | {} msgs | {:.4}$",
            task.task_id,
            task.updated_at.format("%Y-%m-%d %H:%M"),
            task.title,
            task.message_count,
            task.total_cost,
        );
    }
    return Ok(());
}

if let Some(task_id) = cli.resume {
    controller.resume_task(&task_id).await?;
} else if let Some(prompt) = cli.prompt {
    controller.start_task(prompt).await?;
} else {
    // Interactive mode — wait for user input
}
```

### 24.7 Task title generation

Auto-generate a title from the first user message:

```rust
fn generate_title(first_message: &str) -> String {
    let title = first_message
        .lines()
        .next()
        .unwrap_or(first_message)
        .trim();

    if title.len() <= 80 {
        title.to_string()
    } else {
        format!("{}...", &title[..77])
    }
}
```

### 24.8 Task history TUI view

Add a task history view accessible via Ctrl+H or `/history` command:

```rust
pub struct HistoryView {
    tasks: Vec<TaskSummary>,
    selected: usize,
    scroll_offset: usize,
}

impl HistoryView {
    pub fn new(tasks: Vec<TaskSummary>) -> Self {
        Self { tasks, selected: 0, scroll_offset: 0 }
    }

    pub fn selected_task_id(&self) -> Option<&str> {
        self.tasks.get(self.selected).map(|t| t.task_id.as_str())
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll_offset {
                self.scroll_offset = self.selected;
            }
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.tasks.len() {
            self.selected += 1;
        }
    }
}
```

Render each task as a row:
```
  2026-03-20 14:30  Fix authentication bug          anthropic/claude-sonnet-4  12 msgs  $0.0234
> 2026-03-20 13:15  Refactor database module        anthropic/claude-sonnet-4   8 msgs  $0.0156
  2026-03-19 10:00  Add unit tests for parser       openai/gpt-4o              24 msgs  $0.0089
```

Key bindings in history view:
- `Up/Down`: Navigate
- `Enter`: Resume selected task
- `d`: Delete selected task (with confirmation)
- `Esc`: Return to chat view

## Tests

```rust
#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn test_persisted_task_roundtrip() {
        let task = PersistedTask {
            task_id: "test-1".to_string(),
            title: "Fix bug".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            messages: vec![
                PersistedMessage {
                    role: "user".to_string(),
                    content: vec![PersistedContent::Text { text: "Fix the bug".to_string() }],
                    timestamp: chrono::Utc::now(),
                },
                PersistedMessage {
                    role: "assistant".to_string(),
                    content: vec![
                        PersistedContent::Thinking { text: "Let me think...".to_string(), signature: None },
                        PersistedContent::Text { text: "Fixed it.".to_string() },
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
    fn test_persisted_content_text_roundtrip() {
        let content = PersistedContent::Text { text: "hello world".to_string() };
        let json = serde_json::to_string(&content).unwrap();
        let loaded: PersistedContent = serde_json::from_str(&json).unwrap();
        match loaded {
            PersistedContent::Text { text } => assert_eq!(text, "hello world"),
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn test_persisted_content_thinking_with_signature() {
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
    fn test_persisted_content_thinking_without_signature() {
        let content = PersistedContent::Thinking { text: "hmm".to_string(), signature: None };
        let json = serde_json::to_string(&content).unwrap();
        assert!(!json.contains("signature")); // skip_serializing_if None
    }

    #[test]
    fn test_persisted_content_tool_use() {
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
    fn test_persisted_content_tool_result() {
        let content = PersistedContent::ToolResult {
            tool_use_id: "t1".to_string(),
            content: "file contents".to_string(),
            is_error: false,
        };
        let json = serde_json::to_string(&content).unwrap();
        let loaded: PersistedContent = serde_json::from_str(&json).unwrap();
        match loaded {
            PersistedContent::ToolResult { tool_use_id, content, is_error } => {
                assert_eq!(tool_use_id, "t1");
                assert_eq!(content, "file contents");
                assert!(!is_error);
            }
            _ => panic!("Expected ToolResult variant"),
        }
    }

    #[test]
    fn test_message_conversion_user() {
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
    fn test_message_conversion_assistant_with_thinking() {
        let msg = Message {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Thinking { text: "hmm".to_string(), signature: Some("sig".to_string()) },
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
    fn test_save_and_load_task() {
        let dir = tempfile::TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let task = PersistedTask {
            task_id: "t-save".to_string(),
            title: "Test".to_string(),
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
        };
        history.save_task(&task).unwrap();
        let loaded = history.load_task("t-save").unwrap();
        assert_eq!(loaded.title, "Test");
        assert_eq!(loaded.provider, "anthropic");
    }

    #[test]
    fn test_save_overwrites() {
        let dir = tempfile::TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let mut task = PersistedTask {
            task_id: "t-overwrite".to_string(),
            title: "Version 1".to_string(),
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
        };
        history.save_task(&task).unwrap();
        task.title = "Version 2".to_string();
        history.save_task(&task).unwrap();
        let loaded = history.load_task("t-overwrite").unwrap();
        assert_eq!(loaded.title, "Version 2");
    }

    #[test]
    fn test_load_nonexistent_task() {
        let dir = tempfile::TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        assert!(history.load_task("nonexistent").is_err());
    }

    #[test]
    fn test_list_tasks_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let tasks = history.list_tasks().unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn test_list_tasks_sorted_by_date() {
        let dir = tempfile::TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();

        let now = chrono::Utc::now();
        for i in 0..3 {
            let task = PersistedTask {
                task_id: format!("t-{i}"),
                title: format!("Task {i}"),
                created_at: now,
                updated_at: now + chrono::Duration::seconds(i as i64),
                messages: vec![],
                mode: "act".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4".to_string(),
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cost: 0.0,
                completed: false,
            };
            history.save_task(&task).unwrap();
        }

        let tasks = history.list_tasks().unwrap();
        assert_eq!(tasks.len(), 3);
        // Newest first
        assert_eq!(tasks[0].task_id, "t-2");
        assert_eq!(tasks[1].task_id, "t-1");
        assert_eq!(tasks[2].task_id, "t-0");
    }

    #[test]
    fn test_delete_task() {
        let dir = tempfile::TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let task = PersistedTask {
            task_id: "t-delete".to_string(),
            title: "Delete me".to_string(),
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
        };
        history.save_task(&task).unwrap();
        history.delete_task("t-delete").unwrap();
        assert!(history.load_task("t-delete").is_err());
    }

    #[test]
    fn test_delete_nonexistent_task() {
        let dir = tempfile::TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        assert!(history.delete_task("nonexistent").is_err());
    }

    #[test]
    fn test_prune_keeps_recent() {
        let dir = tempfile::TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();

        let now = chrono::Utc::now();
        for i in 0..5 {
            let task = PersistedTask {
                task_id: format!("t-{i}"),
                title: format!("Task {i}"),
                created_at: now,
                updated_at: now + chrono::Duration::seconds(i as i64),
                messages: vec![],
                mode: "act".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4".to_string(),
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cost: 0.0,
                completed: false,
            };
            history.save_task(&task).unwrap();
        }

        let deleted = history.prune(3).unwrap();
        assert_eq!(deleted, 2);
        let remaining = history.list_tasks().unwrap();
        assert_eq!(remaining.len(), 3);
        // Newest 3 kept
        assert_eq!(remaining[0].task_id, "t-4");
        assert_eq!(remaining[1].task_id, "t-3");
        assert_eq!(remaining[2].task_id, "t-2");
    }

    #[test]
    fn test_generate_title_short() {
        assert_eq!(generate_title("Fix the bug"), "Fix the bug");
    }

    #[test]
    fn test_generate_title_multiline() {
        assert_eq!(
            generate_title("Fix the bug\nMore details here"),
            "Fix the bug"
        );
    }

    #[test]
    fn test_generate_title_long() {
        let long_msg = "a".repeat(100);
        let title = generate_title(&long_msg);
        assert_eq!(title.len(), 80); // 77 chars + "..."
        assert!(title.ends_with("..."));
    }

    #[test]
    fn test_history_view_navigation() {
        let tasks = vec![
            TaskSummary {
                task_id: "t-1".to_string(),
                title: "Task 1".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4".to_string(),
                message_count: 5,
                total_input_tokens: 100,
                total_output_tokens: 50,
                total_cost: 0.001,
                completed: false,
            },
            TaskSummary {
                task_id: "t-2".to_string(),
                title: "Task 2".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                message_count: 10,
                total_input_tokens: 200,
                total_output_tokens: 100,
                total_cost: 0.002,
                completed: true,
            },
        ];
        let mut view = HistoryView::new(tasks);
        assert_eq!(view.selected_task_id(), Some("t-1"));
        view.move_down();
        assert_eq!(view.selected_task_id(), Some("t-2"));
        view.move_down(); // Should not go past last
        assert_eq!(view.selected_task_id(), Some("t-2"));
        view.move_up();
        assert_eq!(view.selected_task_id(), Some("t-1"));
        view.move_up(); // Should not go before first
        assert_eq!(view.selected_task_id(), Some("t-1"));
    }

    #[test]
    fn test_history_view_empty() {
        let view = HistoryView::new(vec![]);
        assert_eq!(view.selected_task_id(), None);
    }
}
```

## Acceptance Criteria
- [ ] Task state serializes to JSON with all messages and metadata
- [ ] Messages round-trip through PersistedMessage without data loss
- [ ] Thinking blocks with signatures preserved through serialization
- [ ] Tool use/result blocks preserved through serialization
- [ ] Tasks saved to ~/.meh/history/{task_id}.json
- [ ] Atomic writes (temp file + rename) prevent corruption
- [ ] Auto-save debounced to every 2 seconds
- [ ] --resume flag loads task and continues conversation
- [ ] --history flag lists recent tasks with metadata
- [ ] Task history listable, sorted newest-first
- [ ] Task deletion and pruning work correctly
- [ ] Ctrl+H opens history view in TUI
- [ ] History view supports navigation and task selection
- [ ] Task title auto-generated from first user message
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass
