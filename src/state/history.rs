//! Conversation/task history persistence.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A single task history entry, persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskHistoryEntry {
    /// Unique task identifier.
    pub task_id: String,
    /// First line of user's initial prompt.
    pub title: String,
    /// When the task was started.
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// When the task was completed (if finished).
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Total tokens used.
    pub total_tokens: u64,
    /// Total cost in USD.
    pub total_cost: f64,
    /// Number of messages in the conversation.
    pub message_count: u32,
}

/// Manages task history entries on disk.
pub struct TaskHistory {
    history_dir: PathBuf,
}

impl TaskHistory {
    /// Create a new `TaskHistory`. Creates the history directory if it doesn't exist.
    pub fn new(history_dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&history_dir)?;
        Ok(Self { history_dir })
    }

    /// Save a task history entry as `{task_id}.json`.
    pub fn save_entry(&self, entry: &TaskHistoryEntry) -> anyhow::Result<()> {
        let path = self.history_dir.join(format!("{}.json", entry.task_id));
        let json = serde_json::to_string_pretty(entry)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load a single task history entry by ID.
    pub fn load_entry(&self, task_id: &str) -> anyhow::Result<TaskHistoryEntry> {
        let path = self.history_dir.join(format!("{task_id}.json"));
        let json = std::fs::read_to_string(path)?;
        let entry: TaskHistoryEntry = serde_json::from_str(&json)?;
        Ok(entry)
    }

    /// List all task history entries from the history directory.
    pub fn list_entries(&self) -> anyhow::Result<Vec<TaskHistoryEntry>> {
        let mut entries = Vec::new();
        for dir_entry in std::fs::read_dir(&self.history_dir)? {
            let dir_entry = dir_entry?;
            let path = dir_entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let json = std::fs::read_to_string(&path)?;
                if let Ok(entry) = serde_json::from_str::<TaskHistoryEntry>(&json) {
                    entries.push(entry);
                }
            }
        }
        Ok(entries)
    }

    /// Delete a task history entry by ID.
    pub fn delete_entry(&self, task_id: &str) -> anyhow::Result<()> {
        let path = self.history_dir.join(format!("{task_id}.json"));
        std::fs::remove_file(path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_entry(task_id: &str, title: &str) -> TaskHistoryEntry {
        TaskHistoryEntry {
            task_id: task_id.to_string(),
            title: title.to_string(),
            started_at: chrono::Utc::now(),
            completed_at: None,
            total_tokens: 0,
            total_cost: 0.0,
            message_count: 0,
        }
    }

    #[test]
    fn history_save_and_load() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        let entry = TaskHistoryEntry {
            task_id: "task-abc".to_string(),
            title: "Fix the bug".to_string(),
            started_at: chrono::Utc::now(),
            completed_at: None,
            total_tokens: 500,
            total_cost: 0.01,
            message_count: 4,
        };
        history.save_entry(&entry).unwrap();
        let loaded = history.load_entry("task-abc").unwrap();
        assert_eq!(loaded.title, "Fix the bug");
        assert_eq!(loaded.total_tokens, 500);
    }

    #[test]
    fn history_list_entries() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        for i in 0..3 {
            history
                .save_entry(&make_entry(&format!("task-{i}"), &format!("Task {i}")))
                .unwrap();
        }
        let entries = history.list_entries().unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn history_delete_entry() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        history
            .save_entry(&make_entry("task-del", "Delete me"))
            .unwrap();
        history.delete_entry("task-del").unwrap();
        assert!(history.load_entry("task-del").is_err());
    }

    #[test]
    fn history_load_nonexistent_returns_error() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        assert!(history.load_entry("nonexistent").is_err());
    }

    #[test]
    fn history_creates_directory() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("deep/nested/history");
        let _history = TaskHistory::new(nested.clone()).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn history_entry_roundtrip_json() {
        let entry = TaskHistoryEntry {
            task_id: "rt-1".to_string(),
            title: "Roundtrip".to_string(),
            started_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            total_tokens: 1234,
            total_cost: 0.05,
            message_count: 10,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let restored: TaskHistoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.task_id, "rt-1");
        assert_eq!(restored.total_tokens, 1234);
        assert!(restored.completed_at.is_some());
    }
}
