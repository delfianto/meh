# STEP 02 — State Management (Config, Persistence, Secrets)

## Objective
Implement the full state management layer: config file parsing (TOML), in-memory state cache, disk persistence, and secure API key storage. After this step, the app can load configuration from `~/.meh/config.toml`, store/retrieve API keys, and persist state to disk.

## Prerequisites
- STEP 01 complete (all files exist, project compiles)

## Detailed Instructions

### 2.1 Define core configuration types in `src/state/config.rs`

```rust
//! Application configuration types and loading.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Top-level configuration, maps to config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub provider: ProviderConfig,
    pub mode: ModeConfig,
    pub permissions: PermissionsConfig,
}

/// Provider configuration section
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Default provider name: "anthropic", "openai", "gemini", "openrouter"
    pub default: String,
    pub anthropic: ProviderSettings,
    pub openai: ProviderSettings,
    pub gemini: ProviderSettings,
    pub openrouter: ProviderSettings,
}

/// Settings for a single provider
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderSettings {
    /// Environment variable name containing the API key
    pub api_key_env: Option<String>,
    /// Direct API key (NOT recommended — prefer env var)
    pub api_key: Option<String>,
    /// Custom base URL (for proxies/self-hosted)
    pub base_url: Option<String>,
}

/// Mode configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeConfig {
    /// Default mode: "plan", "act", or "plan_then_act"
    pub default: String,
    /// Require plan approval before acting
    pub strict_plan: bool,
    pub plan: ModeModelConfig,
    pub act: ModeModelConfig,
}

/// Per-mode model settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeModelConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking_budget: Option<u32>,
}

/// Permission configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionsConfig {
    /// "ask", "auto", "yolo"
    pub mode: String,
    pub auto_approve: AutoApproveConfig,
    pub command_rules: CommandRulesConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoApproveConfig {
    pub read_files: bool,
    pub edit_files: bool,
    pub execute_safe_commands: bool,
    pub execute_all_commands: bool,
    pub mcp_tools: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandRulesConfig {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub allow_redirects: bool,
}
```

Implement `Default` for every config type with these values:
- `AppConfig::default()` — provider default `"anthropic"`, mode default `"act"`, permissions default `"ask"`
- `ProviderConfig::default()` — `default = "anthropic"`, each provider has default env var names:
  - anthropic: `api_key_env = Some("ANTHROPIC_API_KEY")`
  - openai: `api_key_env = Some("OPENAI_API_KEY")`
  - gemini: `api_key_env = Some("GEMINI_API_KEY")`
  - openrouter: `api_key_env = Some("OPENROUTER_API_KEY")`
- `ModeConfig::default()` — `default = "act"`, `strict_plan = false`
- `ModeModelConfig::default()` — all `None`
- `PermissionsConfig::default()` — `mode = "ask"`, auto_approve all `false`
- `AutoApproveConfig::default()` — all `false`
- `CommandRulesConfig::default()` — empty vecs, `allow_redirects = false`

Implement these methods on `AppConfig`:
- `AppConfig::load(path: Option<&Path>) -> anyhow::Result<Self>` — Reads TOML file, falls back to default if not found
- `AppConfig::save(&self, path: &Path) -> anyhow::Result<()>` — Writes TOML to disk
- `AppConfig::config_dir() -> PathBuf` — Returns `~/.meh/`, creates it if needed
- `AppConfig::default_config_path() -> PathBuf` — Returns `~/.meh/config.toml`
- `AppConfig::resolve_api_key(&self, provider_name: &str) -> Option<String>` — Checks env var first, then inline key

`resolve_api_key` logic:
1. Look up the `ProviderSettings` for the given `provider_name` (match on `"anthropic"`, `"openai"`, `"gemini"`, `"openrouter"`)
2. If `api_key_env` is `Some(var_name)`, try `std::env::var(var_name)`. If that returns `Ok(val)` and `val` is non-empty, return `Some(val)`.
3. Otherwise, if `api_key` is `Some(key)` and non-empty, return `Some(key.clone())`.
4. Otherwise return `None`.

### 2.2 Implement StateManager in `src/state/mod.rs`

```rust
//! State management — in-memory cache backed by disk persistence.

pub mod config;
pub mod history;
pub mod secrets;
pub mod task_state;

use config::AppConfig;
use tokio::sync::RwLock;
use std::sync::Arc;
use std::path::PathBuf;

/// Central state manager. Clone-friendly (Arc internals).
#[derive(Clone)]
pub struct StateManager {
    inner: Arc<RwLock<StateInner>>,
}

struct StateInner {
    config: AppConfig,
    config_path: PathBuf,
    dirty: bool, // Tracks unsaved changes for debounced persistence
}

impl StateManager {
    /// Create a new StateManager.
    /// If `config_path` is None, uses the default config path (~/.meh/config.toml).
    /// If the config file does not exist, uses defaults (does NOT error).
    pub async fn new(config_path: Option<PathBuf>) -> anyhow::Result<Self> {
        let path = config_path.unwrap_or_else(AppConfig::default_config_path);
        let config = AppConfig::load(Some(&path))?;
        Ok(Self {
            inner: Arc::new(RwLock::new(StateInner {
                config,
                config_path: path,
                dirty: false,
            })),
        })
    }

    /// Get a clone of the current config (read lock).
    pub async fn config(&self) -> AppConfig {
        self.inner.read().await.config.clone()
    }

    /// Update the config with a closure (write lock). Marks state as dirty.
    pub async fn update_config<F>(&self, f: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut AppConfig),
    {
        let mut inner = self.inner.write().await;
        f(&mut inner.config);
        inner.dirty = true;
        Ok(())
    }

    /// Persist the config to disk if it has been modified.
    pub async fn persist(&self) -> anyhow::Result<()> {
        let inner = self.inner.read().await;
        if inner.dirty {
            inner.config.save(&inner.config_path)?;
            drop(inner);
            self.inner.write().await.dirty = false;
        }
        Ok(())
    }

    /// Resolve the API key for the given provider.
    pub async fn resolve_api_key(&self, provider: &str) -> Option<String> {
        self.inner.read().await.config.resolve_api_key(provider)
    }
}
```

### 2.3 Implement secrets management in `src/state/secrets.rs`

```rust
//! Secure API key storage using OS keyring.

/// Store and retrieve API keys securely via the OS credential store.
pub struct SecretStore {
    service_name: String, // "meh" — used as keyring service identifier
}

impl SecretStore {
    pub fn new() -> Self {
        Self {
            service_name: "meh".to_string(),
        }
    }

    /// Store an API key. `key_name` is like "anthropic_api_key".
    pub fn set(&self, key_name: &str, value: &str) -> anyhow::Result<()> {
        let entry = keyring::Entry::new(&self.service_name, key_name)?;
        entry.set_password(value)?;
        Ok(())
    }

    /// Retrieve an API key. Returns None if not found.
    pub fn get(&self, key_name: &str) -> Option<String> {
        let entry = keyring::Entry::new(&self.service_name, key_name).ok()?;
        entry.get_password().ok()
    }

    /// Delete an API key.
    pub fn delete(&self, key_name: &str) -> anyhow::Result<()> {
        let entry = keyring::Entry::new(&self.service_name, key_name)?;
        entry.delete_credential()?;
        Ok(())
    }
}
```

Note on `SecretStore`: The `keyring` crate may not work in all environments (CI, headless Linux without a secrets service). Tests that exercise `SecretStore` should be marked `#[ignore]` so they only run on developer machines. The application code should always fall back to environment variables if the keyring is unavailable.

### 2.4 Implement task state in `src/state/task_state.rs`

```rust
//! Per-task mutable state.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Represents the mode of operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Plan,
    Act,
}

/// Current state of a running task.
#[derive(Debug, Clone)]
pub struct TaskState {
    pub task_id: String,
    pub mode: Mode,
    pub started_at: DateTime<Utc>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost: f64,
    pub api_calls: u32,
    pub tools_executed: u32,
    pub is_running: bool,
}

impl TaskState {
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
    pub fn record_tool_execution(&mut self) {
        self.tools_executed += 1;
    }
}
```

### 2.5 Implement task history in `src/state/history.rs`

```rust
//! Conversation/task history persistence.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskHistoryEntry {
    pub task_id: String,
    pub title: String, // First line of user's initial prompt
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub message_count: u32,
}

pub struct TaskHistory {
    history_dir: PathBuf, // ~/.meh/history/
}

impl TaskHistory {
    /// Create a new TaskHistory. Creates the history directory if it doesn't exist.
    pub fn new(history_dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&history_dir)?;
        Ok(Self { history_dir })
    }

    /// Save a task history entry as a JSON file named `{task_id}.json`.
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

    /// List all task history entries (reads all JSON files in the history dir).
    pub fn list_entries(&self) -> anyhow::Result<Vec<TaskHistoryEntry>> {
        let mut entries = Vec::new();
        for dir_entry in std::fs::read_dir(&self.history_dir)? {
            let dir_entry = dir_entry?;
            let path = dir_entry.path();
            if path.extension().map_or(false, |ext| ext == "json") {
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
```

### 2.6 Wire into app.rs
Update `App::new()` to create a `StateManager` and store it. Log the loaded configuration at debug level to confirm it loads correctly.

```rust
use crate::Cli;
use crate::state::StateManager;

pub struct App {
    cli: Cli,
    state: StateManager,
}

impl App {
    pub async fn new(cli: Cli) -> anyhow::Result<Self> {
        let state = StateManager::new(cli.config.clone()).await?;
        let config = state.config().await;
        tracing::debug!(?config, "Loaded configuration");
        Ok(Self { cli, state })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        tracing::info!("meh starting up");
        Ok(())
    }
}
```

## Tests

### Unit tests for config
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn test_default_config_serializes_to_valid_toml() {
        let config = AppConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.provider.default, "anthropic");
    }

    #[test]
    fn test_config_load_missing_file_returns_default() {
        let result = AppConfig::load(Some(Path::new("/nonexistent/config.toml")));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().provider.default, "anthropic");
    }

    #[test]
    fn test_config_save_and_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = AppConfig::default();
        config.provider.default = "openai".to_string();
        config.save(&path).unwrap();
        let loaded = AppConfig::load(Some(&path)).unwrap();
        assert_eq!(loaded.provider.default, "openai");
    }

    #[test]
    fn test_resolve_api_key_from_env() {
        std::env::set_var("TEST_MEH_KEY", "sk-test-123");
        let config = AppConfig {
            provider: ProviderConfig {
                anthropic: ProviderSettings {
                    api_key_env: Some("TEST_MEH_KEY".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            config.resolve_api_key("anthropic"),
            Some("sk-test-123".to_string())
        );
        std::env::remove_var("TEST_MEH_KEY");
    }

    #[test]
    fn test_resolve_api_key_inline_fallback() {
        let config = AppConfig {
            provider: ProviderConfig {
                anthropic: ProviderSettings {
                    api_key: Some("sk-inline".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            config.resolve_api_key("anthropic"),
            Some("sk-inline".to_string())
        );
    }

    #[test]
    fn test_resolve_api_key_env_takes_precedence() {
        std::env::set_var("TEST_MEH_KEY2", "sk-from-env");
        let config = AppConfig {
            provider: ProviderConfig {
                anthropic: ProviderSettings {
                    api_key_env: Some("TEST_MEH_KEY2".to_string()),
                    api_key: Some("sk-inline".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            config.resolve_api_key("anthropic"),
            Some("sk-from-env".to_string())
        );
        std::env::remove_var("TEST_MEH_KEY2");
    }

    #[test]
    fn test_mode_config_defaults() {
        let config = ModeConfig::default();
        assert_eq!(config.default, "act");
        assert!(!config.strict_plan);
    }

    #[test]
    fn test_permissions_config_defaults() {
        let config = PermissionsConfig::default();
        assert_eq!(config.mode, "ask");
        assert!(!config.auto_approve.read_files);
        assert!(!config.auto_approve.execute_all_commands);
    }

    #[test]
    fn test_partial_toml_fills_defaults() {
        let toml_str = r#"
[provider]
default = "gemini"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider.default, "gemini");
        assert_eq!(config.mode.default, "act"); // default filled in
    }
}
```

### StateManager tests
```rust
#[cfg(test)]
mod state_manager_tests {
    use super::*;

    #[tokio::test]
    async fn test_state_manager_new() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let sm = StateManager::new(Some(path)).await.unwrap();
        let cfg = sm.config().await;
        assert_eq!(cfg.provider.default, "anthropic");
    }

    #[tokio::test]
    async fn test_state_manager_update_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let sm = StateManager::new(Some(path)).await.unwrap();
        sm.update_config(|c| c.provider.default = "openai".to_string())
            .await
            .unwrap();
        let cfg = sm.config().await;
        assert_eq!(cfg.provider.default, "openai");
    }

    #[tokio::test]
    async fn test_state_manager_persist() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let sm = StateManager::new(Some(path.clone())).await.unwrap();
        sm.update_config(|c| c.provider.default = "gemini".to_string())
            .await
            .unwrap();
        sm.persist().await.unwrap();
        // Reload from disk
        let sm2 = StateManager::new(Some(path)).await.unwrap();
        let cfg = sm2.config().await;
        assert_eq!(cfg.provider.default, "gemini");
    }
}
```

### TaskState tests
```rust
#[cfg(test)]
mod task_state_tests {
    use crate::state::task_state::{Mode, TaskState};

    #[test]
    fn test_task_state_new() {
        let ts = TaskState::new("test-1".to_string(), Mode::Act);
        assert_eq!(ts.task_id, "test-1");
        assert_eq!(ts.mode, Mode::Act);
        assert!(ts.is_running);
        assert_eq!(ts.total_cost, 0.0);
    }

    #[test]
    fn test_task_state_record_usage() {
        let mut ts = TaskState::new("test-1".to_string(), Mode::Plan);
        ts.record_usage(100, 50, 0.003);
        ts.record_usage(200, 100, 0.005);
        assert_eq!(ts.total_input_tokens, 300);
        assert_eq!(ts.total_output_tokens, 150);
        assert!((ts.total_cost - 0.008).abs() < f64::EPSILON);
        assert_eq!(ts.api_calls, 2);
    }

    #[test]
    fn test_task_state_record_tool_execution() {
        let mut ts = TaskState::new("test-1".to_string(), Mode::Act);
        ts.record_tool_execution();
        ts.record_tool_execution();
        assert_eq!(ts.tools_executed, 2);
    }
}
```

### TaskHistory tests
```rust
#[cfg(test)]
mod history_tests {
    use crate::state::history::{TaskHistory, TaskHistoryEntry};
    use tempfile::TempDir;

    #[test]
    fn test_history_save_and_load() {
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
    }

    #[test]
    fn test_history_list_entries() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        for i in 0..3 {
            history
                .save_entry(&TaskHistoryEntry {
                    task_id: format!("task-{i}"),
                    title: format!("Task {i}"),
                    started_at: chrono::Utc::now(),
                    completed_at: None,
                    total_tokens: 0,
                    total_cost: 0.0,
                    message_count: 0,
                })
                .unwrap();
        }
        let entries = history.list_entries().unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_history_delete_entry() {
        let dir = TempDir::new().unwrap();
        let history = TaskHistory::new(dir.path().to_path_buf()).unwrap();
        history
            .save_entry(&TaskHistoryEntry {
                task_id: "task-del".to_string(),
                title: "Delete me".to_string(),
                started_at: chrono::Utc::now(),
                completed_at: None,
                total_tokens: 0,
                total_cost: 0.0,
                message_count: 0,
            })
            .unwrap();
        history.delete_entry("task-del").unwrap();
        assert!(history.load_entry("task-del").is_err());
    }
}
```

## Acceptance Criteria
- [x] `AppConfig` deserializes from TOML with defaults for missing fields
- [x] Config round-trips (save -> load) without data loss
- [x] API keys resolved from env vars (priority) or inline config
- [x] `StateManager` provides async read/write access with `RwLock`
- [x] `StateManager::persist()` writes to disk only when dirty
- [x] `TaskState` tracks tokens, cost, API calls, and tool executions
- [x] `TaskHistory` persists to `~/.config/meh/history/` as JSON files
- [x] `TaskHistory::list_entries()` reads all entries from disk
- [x] `cargo test` — all 41 tests pass (1 ignored: keyring)
- [x] `cargo clippy -- -D warnings` — zero warnings
- [x] No `unwrap()` or `expect()` in non-test code

**Completed**: PR #3
