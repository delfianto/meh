//! File watcher for config hot-reload.
//!
//! Watches the config file (and optionally MCP settings) for changes
//! using the `notify` crate. Changes are debounced (100ms) to avoid
//! rapid reloads from editors that write multiple times.

use crate::controller::messages::ControllerMessage;
use notify::{EventKind, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

/// Spawns a background task that watches config files and sends reload messages.
///
/// Watches the directory containing `config_path`. When the config file
/// is modified, sends `ControllerMessage::ConfigReload`. Changes are
/// debounced to avoid rapid reloads from editor save cycles.
pub fn spawn_config_watcher(
    config_path: PathBuf,
    ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
) {
    tokio::spawn(async move {
        if let Err(e) = watch_config_file(config_path, ctrl_tx).await {
            tracing::warn!(error = %e, "Config file watcher stopped");
        }
    });
}

/// Watches a config file for changes and sends reload messages.
async fn watch_config_file(
    config_path: PathBuf,
    ctrl_tx: mpsc::UnboundedSender<ControllerMessage>,
) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();

    let mut watcher =
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                    for path in &event.paths {
                        let _ = tx.send(path.clone());
                    }
                }
            }
        })?;

    let watch_dir = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Config path has no parent directory"))?;

    watcher.watch(watch_dir, RecursiveMode::NonRecursive)?;

    let debounce = Duration::from_millis(100);

    while let Some(changed_path) = rx.recv().await {
        if changed_path != config_path {
            continue;
        }
        tokio::time::sleep(debounce).await;
        while rx.try_recv().is_ok() {}

        tracing::info!("Config file changed, requesting reload");
        if ctrl_tx.send(ControllerMessage::ConfigReload).is_err() {
            tracing::debug!("Controller channel closed, stopping watcher");
            break;
        }
    }

    Ok(())
}

/// Extract the MCP settings path from the config directory.
pub fn mcp_settings_path(config_path: &std::path::Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("mcp_settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;

    #[tokio::test]
    #[ignore = "requires OS file system events, may be flaky in CI"]
    async fn watcher_detects_config_change() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "[provider]\ndefault = \"anthropic\"\n").unwrap();

        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();

        spawn_config_watcher(config_path.clone(), ctrl_tx);

        tokio::time::sleep(Duration::from_millis(200)).await;

        std::fs::write(&config_path, "[provider]\ndefault = \"openai\"\n").unwrap();

        let result = tokio::time::timeout(Duration::from_secs(3), ctrl_rx.recv()).await;
        match result {
            Ok(Some(ControllerMessage::ConfigReload)) => {}
            Ok(Some(other)) => panic!("Expected ConfigReload, got {other:?}"),
            Ok(None) => panic!("Channel closed unexpectedly"),
            Err(_) => panic!("Timed out waiting for ConfigReload"),
        }
    }

    #[tokio::test]
    #[ignore = "requires OS file system events, may be flaky in CI"]
    async fn watcher_ignores_other_files() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").unwrap();

        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();

        spawn_config_watcher(config_path, ctrl_tx);

        tokio::time::sleep(Duration::from_millis(200)).await;

        std::fs::write(dir.path().join("other.txt"), "not config").unwrap();

        let result = tokio::time::timeout(Duration::from_millis(500), ctrl_rx.recv()).await;
        assert!(result.is_err(), "Should not receive any message");
    }

    #[test]
    fn mcp_settings_path_from_config() {
        let config = std::path::Path::new("/home/user/.config/meh/config.toml");
        let mcp = mcp_settings_path(config);
        assert_eq!(
            mcp,
            PathBuf::from("/home/user/.config/meh/mcp_settings.json")
        );
    }
}
