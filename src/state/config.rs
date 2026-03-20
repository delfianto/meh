//! Application configuration types and loading.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Top-level configuration, maps to `config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Provider configuration section.
    pub provider: ProviderConfig,
    /// Mode configuration section.
    pub mode: ModeConfig,
    /// Permission configuration section.
    pub permissions: PermissionsConfig,
}

/// Provider configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Default provider name.
    pub default: String,
    /// Anthropic provider settings.
    pub anthropic: ProviderSettings,
    /// `OpenAI` provider settings.
    pub openai: ProviderSettings,
    /// Gemini provider settings.
    pub gemini: ProviderSettings,
    /// `OpenRouter` provider settings.
    pub openrouter: ProviderSettings,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            default: "anthropic".to_string(),
            anthropic: ProviderSettings {
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                ..ProviderSettings::default()
            },
            openai: ProviderSettings {
                api_key_env: Some("OPENAI_API_KEY".to_string()),
                ..ProviderSettings::default()
            },
            gemini: ProviderSettings {
                api_key_env: Some("GEMINI_API_KEY".to_string()),
                ..ProviderSettings::default()
            },
            openrouter: ProviderSettings {
                api_key_env: Some("OPENROUTER_API_KEY".to_string()),
                ..ProviderSettings::default()
            },
        }
    }
}

/// Settings for a single provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderSettings {
    /// Environment variable name containing the API key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Direct API key (not recommended — prefer env var).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Custom base URL (for proxies/self-hosted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Model ID override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Mode configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeConfig {
    /// Default mode: "plan", "act", or "`plan_then_act`".
    pub default: String,
    /// Require plan approval before acting.
    pub strict_plan: bool,
    /// Plan mode model settings.
    pub plan: ModeModelConfig,
    /// Act mode model settings.
    pub act: ModeModelConfig,
}

impl Default for ModeConfig {
    fn default() -> Self {
        Self {
            default: "act".to_string(),
            strict_plan: false,
            plan: ModeModelConfig::default(),
            act: ModeModelConfig::default(),
        }
    }
}

/// Per-mode model settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModeModelConfig {
    /// Provider override for this mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model ID override for this mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Thinking token budget for this mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
}

/// Permission configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionsConfig {
    /// Permission mode: "ask", "auto", "yolo".
    pub mode: String,
    /// Auto-approval rules per tool category.
    pub auto_approve: AutoApproveConfig,
    /// Shell command allow/deny rules.
    pub command_rules: CommandRulesConfig,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        Self {
            mode: "ask".to_string(),
            auto_approve: AutoApproveConfig::default(),
            command_rules: CommandRulesConfig::default(),
        }
    }
}

/// Auto-approval rules per tool category.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct AutoApproveConfig {
    /// Auto-approve file reads.
    pub read_files: bool,
    /// Auto-approve file edits.
    pub edit_files: bool,
    /// Auto-approve safe commands (git status, ls, etc.).
    pub execute_safe_commands: bool,
    /// Auto-approve all commands.
    pub execute_all_commands: bool,
    /// Auto-approve MCP tool calls.
    pub mcp_tools: bool,
}

/// Shell command allow/deny rules.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommandRulesConfig {
    /// Glob patterns for allowed commands.
    pub allow: Vec<String>,
    /// Glob patterns for denied commands.
    pub deny: Vec<String>,
    /// Whether to allow redirect operators (>, >>, <).
    pub allow_redirects: bool,
}

impl AppConfig {
    /// Load configuration from a TOML file.
    ///
    /// If the file does not exist, returns the default configuration.
    /// If the file exists but is invalid, returns an error.
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };

        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let config: Self = toml::from_str(&contents)?;
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Save configuration to a TOML file.
    ///
    /// Creates parent directories if they do not exist.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml_str = toml::to_string_pretty(self)?;
        std::fs::write(path, toml_str)?;
        Ok(())
    }

    /// Returns the default config directory, creating it if needed.
    ///
    /// Always uses `$HOME/.config/meh/` on all platforms (Linux, macOS, WSL).
    pub fn config_dir() -> PathBuf {
        let dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config/meh");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Returns the default config file path (`$HOME/.config/meh/config.toml`).
    pub fn default_config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// Returns the data directory for persistent state (history, etc.).
    ///
    /// Uses `$HOME/.config/meh/` (same as config dir for simplicity).
    pub fn data_dir() -> PathBuf {
        Self::config_dir()
    }

    /// Returns the history directory (`$HOME/.config/meh/history/`).
    pub fn history_dir() -> PathBuf {
        let dir = Self::data_dir().join("history");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Resolve the API key for a provider.
    ///
    /// Checks the environment variable first, then falls back to the inline key.
    pub fn resolve_api_key(&self, provider_name: &str) -> Option<String> {
        let settings = match provider_name {
            "anthropic" => &self.provider.anthropic,
            "openai" => &self.provider.openai,
            "gemini" => &self.provider.gemini,
            "openrouter" => &self.provider.openrouter,
            _ => return None,
        };

        if let Some(ref env_var) = settings.api_key_env {
            if let Ok(val) = std::env::var(env_var) {
                if !val.is_empty() {
                    return Some(val);
                }
            }
        }

        settings.api_key.as_ref().filter(|k| !k.is_empty()).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_config_serializes_to_valid_toml() {
        let config = AppConfig::default();
        let toml_str =
            toml::to_string_pretty(&config).expect("default config should serialize to TOML");
        let parsed: AppConfig =
            toml::from_str(&toml_str).expect("serialized TOML should parse back");
        assert_eq!(parsed.provider.default, "anthropic");
    }

    #[test]
    fn config_load_missing_file_returns_default() {
        let result = AppConfig::load(Some(Path::new("/nonexistent/path/config.toml")));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().provider.default, "anthropic");
    }

    #[test]
    fn config_load_none_returns_default() {
        let result = AppConfig::load(None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().mode.default, "act");
    }

    #[test]
    fn config_save_and_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = AppConfig::default();
        config.provider.default = "openai".to_string();
        config.save(&path).unwrap();
        let loaded = AppConfig::load(Some(&path)).unwrap();
        assert_eq!(loaded.provider.default, "openai");
    }

    #[test]
    fn config_save_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deep/nested/config.toml");
        let config = AppConfig::default();
        config.save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn resolve_api_key_from_env() {
        let config = AppConfig {
            provider: ProviderConfig {
                anthropic: ProviderSettings {
                    api_key_env: Some("PATH".to_string()),
                    ..ProviderSettings::default()
                },
                ..ProviderConfig::default()
            },
            ..AppConfig::default()
        };
        let result = config.resolve_api_key("anthropic");
        assert!(result.is_some());
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn resolve_api_key_inline_fallback() {
        let config = AppConfig {
            provider: ProviderConfig {
                anthropic: ProviderSettings {
                    api_key_env: None,
                    api_key: Some("sk-inline".to_string()),
                    ..ProviderSettings::default()
                },
                ..ProviderConfig::default()
            },
            ..AppConfig::default()
        };
        assert_eq!(
            config.resolve_api_key("anthropic"),
            Some("sk-inline".to_string())
        );
    }

    #[test]
    fn resolve_api_key_env_takes_precedence_over_inline() {
        let config = AppConfig {
            provider: ProviderConfig {
                anthropic: ProviderSettings {
                    api_key_env: Some("PATH".to_string()),
                    api_key: Some("sk-inline".to_string()),
                    ..ProviderSettings::default()
                },
                ..ProviderConfig::default()
            },
            ..AppConfig::default()
        };
        let result = config.resolve_api_key("anthropic");
        assert!(result.is_some());
        assert_ne!(result.unwrap(), "sk-inline");
    }

    #[test]
    fn resolve_api_key_nonexistent_env_falls_back_to_inline() {
        let config = AppConfig {
            provider: ProviderConfig {
                anthropic: ProviderSettings {
                    api_key_env: Some("MEH_DEFINITELY_NOT_SET_XYZ_999".to_string()),
                    api_key: Some("sk-inline".to_string()),
                    ..ProviderSettings::default()
                },
                ..ProviderConfig::default()
            },
            ..AppConfig::default()
        };
        assert_eq!(
            config.resolve_api_key("anthropic"),
            Some("sk-inline".to_string())
        );
    }

    #[test]
    fn resolve_api_key_unknown_provider_returns_none() {
        let config = AppConfig::default();
        assert_eq!(config.resolve_api_key("unknown_provider"), None);
    }

    #[test]
    fn resolve_api_key_no_key_configured_returns_none() {
        let config = AppConfig {
            provider: ProviderConfig {
                anthropic: ProviderSettings {
                    api_key_env: None,
                    api_key: None,
                    ..ProviderSettings::default()
                },
                ..ProviderConfig::default()
            },
            ..AppConfig::default()
        };
        assert_eq!(config.resolve_api_key("anthropic"), None);
    }

    #[test]
    fn mode_config_defaults() {
        let config = ModeConfig::default();
        assert_eq!(config.default, "act");
        assert!(!config.strict_plan);
    }

    #[test]
    fn permissions_config_defaults() {
        let config = PermissionsConfig::default();
        assert_eq!(config.mode, "ask");
        assert!(!config.auto_approve.read_files);
        assert!(!config.auto_approve.execute_all_commands);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let toml_str = r#"
[provider]
default = "gemini"
"#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider.default, "gemini");
        assert_eq!(config.mode.default, "act");
    }

    #[test]
    fn provider_settings_default_env_vars() {
        let config = ProviderConfig::default();
        assert_eq!(
            config.anthropic.api_key_env.as_deref(),
            Some("ANTHROPIC_API_KEY")
        );
        assert_eq!(config.openai.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
        assert_eq!(config.gemini.api_key_env.as_deref(), Some("GEMINI_API_KEY"));
        assert_eq!(
            config.openrouter.api_key_env.as_deref(),
            Some("OPENROUTER_API_KEY")
        );
    }

    #[test]
    fn config_load_invalid_toml_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is not [[[valid toml").unwrap();
        let result = AppConfig::load(Some(&path));
        assert!(result.is_err());
    }
}
