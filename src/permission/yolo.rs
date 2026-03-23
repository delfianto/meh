//! YOLO mode — approve everything without asking.
//!
//! When YOLO mode is active, all tool calls are auto-approved regardless
//! of their category or any other permission rules. This is intended for
//! trusted environments where maximum automation speed is desired.
//!
//! YOLO can be activated via the `--yolo` CLI flag or by setting
//! `permissions.mode = "yolo"` in `config.toml`. At runtime, the user
//! can toggle YOLO on/off with Ctrl+Y.

use crate::state::config::PermissionsConfig;

/// Check if YOLO mode should be active based on config and CLI flag.
pub fn is_yolo_mode(config: &PermissionsConfig, cli_yolo: bool) -> bool {
    cli_yolo || config.mode == "yolo"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yolo_from_cli() {
        assert!(is_yolo_mode(&PermissionsConfig::default(), true));
    }

    #[test]
    fn test_yolo_from_config() {
        let config = PermissionsConfig {
            mode: "yolo".to_string(),
            ..Default::default()
        };
        assert!(is_yolo_mode(&config, false));
    }

    #[test]
    fn test_not_yolo() {
        assert!(!is_yolo_mode(&PermissionsConfig::default(), false));
    }

    #[test]
    fn test_yolo_both_cli_and_config() {
        let config = PermissionsConfig {
            mode: "yolo".to_string(),
            ..Default::default()
        };
        assert!(is_yolo_mode(&config, true));
    }

    #[test]
    fn test_auto_mode_not_yolo() {
        let config = PermissionsConfig {
            mode: "auto".to_string(),
            ..Default::default()
        };
        assert!(!is_yolo_mode(&config, false));
    }
}
