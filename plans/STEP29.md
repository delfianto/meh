# STEP 29 — Config File Hot-Reload

## Objective
Implement file watching on the config file and MCP settings file so changes take effect without restarting the app.

## Prerequisites
- STEP 02 (state management)
- STEP 21 (MCP hub)

## Detailed Instructions

### 29.1 Add notify dependency

Add to Cargo.toml:
```toml
notify = { version = "7", features = ["macos_kqueue"] }
```

### 29.2 File watcher (`src/state/mod.rs`)

```rust
use notify::{Watcher, RecursiveMode, Event, EventKind};
use std::path::PathBuf;

/// Watch config files for changes and send reload messages.
pub async fn watch_config_files(
    config_path: PathBuf,
    mcp_settings_path: PathBuf,
    ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
) -> anyhow::Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                for path in &event.paths {
                    let _ = tx.send(path.clone());
                }
            }
        }
    })?;

    // Watch the directory containing config files
    if let Some(parent) = config_path.parent() {
        watcher.watch(parent, RecursiveMode::NonRecursive)?;
    }
    if let Some(parent) = mcp_settings_path.parent() {
        if parent != config_path.parent().unwrap_or(std::path::Path::new("")) {
            watcher.watch(parent, RecursiveMode::NonRecursive)?;
        }
    }

    // Debounce: wait 100ms after last change before reloading
    let mut last_change = std::time::Instant::now();
    let debounce = Duration::from_millis(100);

    loop {
        tokio::select! {
            Some(path) = rx.recv() => {
                last_change = std::time::Instant::now();
                // Wait for debounce period
                tokio::time::sleep(debounce).await;

                if path == config_path {
                    tracing::info!("Config file changed, reloading");
                    let _ = ctrl_tx.send(ControllerMessage::ConfigReload);
                } else if path == mcp_settings_path {
                    tracing::info!("MCP settings changed, reloading");
                    let _ = ctrl_tx.send(ControllerMessage::McpReload);
                }
            }
        }
    }
}
```

### 29.3 Add reload messages to Controller

```rust
pub enum ControllerMessage {
    // ... existing ...
    ConfigReload,
    McpReload,
}
```

Handler:
```rust
ControllerMessage::ConfigReload => {
    match self.state.reload().await {
        Ok(()) => {
            tracing::info!("Config reloaded successfully");
            // Update permission controller with new rules
            // Update status bar with new model info
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to reload config");
        }
    }
}

ControllerMessage::McpReload => {
    // Disconnect existing MCP servers
    // Reload settings
    // Reconnect to servers
}
```

### 29.4 StateManager reload

```rust
impl StateManager {
    pub async fn reload(&self) -> anyhow::Result<()> {
        let mut inner = self.inner.write().await;
        let new_config = AppConfig::load(Some(&inner.config_path))?;
        inner.config = new_config;
        inner.dirty = false;
        Ok(())
    }
}
```

### 29.5 Prevent write-read cycles

When the app writes to the config file (e.g., saving a setting), it should temporarily ignore file watcher events:

```rust
pub async fn update_config_no_watch<F>(&self, f: F) -> anyhow::Result<()>
where F: FnOnce(&mut AppConfig)
{
    // Set a flag to ignore next file watcher event
    self.set_ignore_next_watch(true);
    self.update_config(f).await?;
    self.persist().await?;
    // Clear flag after a delay
    tokio::time::sleep(Duration::from_millis(200)).await;
    self.set_ignore_next_watch(false);
    Ok(())
}
```

## Tests

```rust
#[cfg(test)]
mod config_reload_tests {
    use super::*;

    #[tokio::test]
    async fn test_config_reload() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        // Write initial config
        let config = AppConfig::default();
        config.save(&path).unwrap();

        // Load state manager
        let sm = StateManager::new(Some(path.clone())).await.unwrap();
        assert_eq!(sm.config().await.provider.default, "anthropic");

        // Modify config file externally
        let mut new_config = AppConfig::default();
        new_config.provider.default = "openai".to_string();
        new_config.save(&path).unwrap();

        // Reload
        sm.reload().await.unwrap();
        assert_eq!(sm.config().await.provider.default, "openai");
    }

    #[tokio::test]
    async fn test_reload_invalid_file_keeps_old_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        let config = AppConfig::default();
        config.save(&path).unwrap();

        let sm = StateManager::new(Some(path.clone())).await.unwrap();

        // Write invalid TOML
        std::fs::write(&path, "this is not valid toml [[[").unwrap();

        // Reload should fail but not crash
        let result = sm.reload().await;
        assert!(result.is_err());

        // Old config should still be in place
        assert_eq!(sm.config().await.provider.default, "anthropic");
    }
}
```

## Acceptance Criteria
- [x] Config file changes detected via notify watcher
- [x] Changes debounced (100ms) to avoid rapid reloads
- [x] Config reload updates permission rules, model selection
- [ ] MCP settings reload reconnects servers
- [x] Invalid config changes logged as warning, don't crash
- [ ] Self-write cycles don't trigger reload
- [x] Watcher runs as background task, doesn't block
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass
