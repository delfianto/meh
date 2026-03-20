//! State management — in-memory cache backed by disk persistence.
//!
//! The `StateManager` is the single source of truth for application
//! configuration and runtime state. It is `Clone`-friendly (Arc internals)
//! so it can be shared across tokio tasks without explicit locking at
//! the call site.
//!
//! ```text
//!   StateManager (Clone → Arc<RwLock<StateInner>>)
//!         │
//!         ├── config.rs     ──► AppConfig (provider, model, mode, keys)
//!         │                     loaded from config.toml, defaults on missing
//!         │
//!         ├── history.rs    ──► conversation/task history persistence
//!         │                     JSON files in ~/.meh/history/
//!         │
//!         ├── secrets.rs    ──► API key storage via OS keyring
//!         │
//!         └── task_state.rs ──► per-task mutable counters (tokens, cost, etc.)
//! ```
//!
//! Config changes are accumulated in memory (marked dirty) and flushed
//! to disk on explicit `persist()` calls. This avoids write amplification
//! from frequent updates during streaming.

pub mod config;
pub mod history;
pub mod secrets;
pub mod task_state;

use config::AppConfig;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Central state manager. Clone-friendly (Arc internals).
#[derive(Clone)]
pub struct StateManager {
    inner: Arc<RwLock<StateInner>>,
}

struct StateInner {
    config: AppConfig,
    config_path: PathBuf,
    dirty: bool,
}

impl StateManager {
    /// Create a new `StateManager`.
    ///
    /// If `config_path` is `None`, uses the default path (`~/.meh/config.toml`).
    /// If the config file does not exist, uses defaults.
    #[allow(clippy::unused_async)]
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

    /// Get a clone of the current config.
    pub async fn config(&self) -> AppConfig {
        self.inner.read().await.config.clone()
    }

    /// Update the config via a closure. Marks state as dirty.
    pub async fn update_config<F>(&self, f: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut AppConfig),
    {
        let mut inner = self.inner.write().await;
        f(&mut inner.config);
        inner.dirty = true;
        drop(inner);
        Ok(())
    }

    /// Persist the config to disk if it has been modified.
    pub async fn persist(&self) -> anyhow::Result<()> {
        let needs_save = self.inner.read().await.dirty;
        if needs_save {
            let inner = self.inner.read().await;
            inner.config.save(&inner.config_path)?;
            drop(inner);
            self.inner.write().await.dirty = false;
        }
        Ok(())
    }

    /// Resolve the API key for the given provider name.
    pub async fn resolve_api_key(&self, provider: &str) -> Option<String> {
        self.inner.read().await.config.resolve_api_key(provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn state_manager_new_with_missing_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let sm = StateManager::new(Some(path)).await.unwrap();
        let cfg = sm.config().await;
        assert_eq!(cfg.provider.default, "anthropic");
    }

    #[tokio::test]
    async fn state_manager_update_config() {
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
    async fn state_manager_persist_and_reload() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let sm = StateManager::new(Some(path.clone())).await.unwrap();
        sm.update_config(|c| c.provider.default = "gemini".to_string())
            .await
            .unwrap();
        sm.persist().await.unwrap();

        let sm2 = StateManager::new(Some(path)).await.unwrap();
        let cfg = sm2.config().await;
        assert_eq!(cfg.provider.default, "gemini");
    }

    #[tokio::test]
    async fn state_manager_persist_skips_when_clean() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let sm = StateManager::new(Some(path.clone())).await.unwrap();
        sm.persist().await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn state_manager_resolve_api_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let sm = StateManager::new(Some(path)).await.unwrap();
        sm.update_config(|c| {
            c.provider.anthropic.api_key = Some("sk-test".to_string());
            c.provider.anthropic.api_key_env = None;
        })
        .await
        .unwrap();
        assert_eq!(
            sm.resolve_api_key("anthropic").await,
            Some("sk-test".to_string())
        );
        assert_eq!(sm.resolve_api_key("unknown").await, None);
    }
}
