//! Application error types with user-friendly messages.
//!
//! Every error that surfaces to the user should be clear, actionable,
//! and context-specific. No raw error strings or stack traces in the TUI.

use crate::provider::common::ProviderError;
use crate::state::config::AppConfig;
use std::time::Duration;
use thiserror::Error;

/// User-facing errors with actionable messages.
#[derive(Error, Debug)]
pub enum MehError {
    /// Authentication/key failure.
    #[error(
        "Authentication failed for {provider}. Check your API key.\n  Hint: Set {env_var} environment variable or add it to ~/.meh/config.toml"
    )]
    AuthFailed { provider: String, env_var: String },

    /// Rate limited by provider.
    #[error("Rate limited by {provider}. {}", retry_msg(.retry_after))]
    RateLimited {
        provider: String,
        retry_after: Option<Duration>,
    },

    /// Provider 5xx error.
    #[error(
        "{provider} server error ({status}): {message}\n  This is a temporary issue. The request will be retried automatically."
    )]
    ProviderServerError {
        provider: String,
        status: u16,
        message: String,
    },

    /// Network/connection failure.
    #[error("Cannot connect to {provider}. Check your internet connection.\n  URL: {url}")]
    ConnectionFailed { provider: String, url: String },

    /// Model not found.
    #[error("Model '{model}' not found for {provider}.\n  Available models: {suggestions}")]
    ModelNotFound {
        provider: String,
        model: String,
        suggestions: String,
    },

    /// No API key configured.
    #[error(
        "No API key configured for {provider}.\n  Set {env_var} or add to config:\n    [provider.{provider_lower}]\n    api_key_env = \"{env_var}\""
    )]
    NoApiKey {
        provider: String,
        provider_lower: String,
        env_var: String,
    },

    /// Tool execution failure.
    #[error("Tool '{tool}' failed: {reason}")]
    ToolFailed { tool: String, reason: String },

    /// Permission denied for tool.
    #[error("Permission denied for '{tool}'. Use 'y' to approve or --yolo to skip approvals.")]
    PermissionDenied { tool: String },

    /// Command execution timeout.
    #[error(
        "Command timed out after {seconds}s: {command}\n  Try increasing the timeout or running the command manually."
    )]
    CommandTimeout { command: String, seconds: u64 },

    /// MCP server startup failure.
    #[error(
        "MCP server '{server}' failed to start: {reason}\n  Check the server command in ~/.meh/mcp_settings.json"
    )]
    McpServerFailed { server: String, reason: String },

    /// MCP tool error.
    #[error("MCP tool '{tool}' on server '{server}' returned an error: {message}")]
    McpToolError {
        server: String,
        tool: String,
        message: String,
    },

    /// Invalid configuration file.
    #[error(
        "Invalid configuration in {file}: {reason}\n  Fix the file or delete it to use defaults."
    )]
    InvalidConfig { file: String, reason: String },

    /// Task not found in history.
    #[error(
        "Task '{task_id}' not found in history.\n  Use 'meh' without --resume to start a new task."
    )]
    TaskNotFound { task_id: String },

    /// Context window exceeded.
    #[error(
        "Context window exceeded ({used} / {limit} tokens).\n  The conversation is too long. Start a new task or use a model with a larger context window."
    )]
    ContextWindowExceeded { used: u64, limit: u32 },
}

/// Format retry timing for rate limit messages.
#[allow(clippy::ref_option)]
fn retry_msg(after: &Option<Duration>) -> String {
    after.as_ref().map_or_else(
        || "Retrying with backoff.".to_string(),
        |d| format!("Retrying in {}s.", d.as_secs()),
    )
}

/// Map a `ProviderError` to a user-friendly `MehError`.
pub fn map_provider_error(err: &anyhow::Error, provider: &str) -> MehError {
    err.downcast_ref::<ProviderError>().map_or_else(
        || MehError::ConnectionFailed {
            provider: provider.to_string(),
            url: "unknown".to_string(),
        },
        |pe| match pe {
            ProviderError::Auth(_) => MehError::AuthFailed {
                provider: provider.to_string(),
                env_var: default_env_var(provider),
            },
            ProviderError::RateLimit { retry_after } => MehError::RateLimited {
                provider: provider.to_string(),
                retry_after: *retry_after,
            },
            ProviderError::Server { status, message } => MehError::ProviderServerError {
                provider: provider.to_string(),
                status: *status,
                message: message.clone(),
            },
            _ => MehError::ToolFailed {
                tool: provider.to_string(),
                reason: pe.to_string(),
            },
        },
    )
}

/// Returns the default env var name for a provider.
pub fn default_env_var(provider: &str) -> String {
    match provider {
        "anthropic" => "ANTHROPIC_API_KEY".to_string(),
        "openai" => "OPENAI_API_KEY".to_string(),
        "gemini" => "GEMINI_API_KEY".to_string(),
        "openrouter" => "OPENROUTER_API_KEY".to_string(),
        _ => format!("{}_API_KEY", provider.to_uppercase()),
    }
}

/// Validate configuration at startup and return any issues.
pub fn validate_config(config: &AppConfig) -> Vec<MehError> {
    let mut errors = Vec::new();

    let provider = &config.provider.default;
    if config.resolve_api_key(provider).is_none() {
        errors.push(MehError::NoApiKey {
            provider: provider.clone(),
            provider_lower: provider.to_lowercase(),
            env_var: default_env_var(provider),
        });
    }

    errors
}

/// Install a panic hook that restores the terminal before printing.
pub fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = ratatui::crossterm::terminal::disable_raw_mode();
        let _ = ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::terminal::LeaveAlternateScreen
        );
        original(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_error_message() {
        let err = MehError::AuthFailed {
            provider: "anthropic".to_string(),
            env_var: "ANTHROPIC_API_KEY".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Authentication failed"));
        assert!(msg.contains("ANTHROPIC_API_KEY"));
        assert!(msg.contains("config.toml"));
    }

    #[test]
    fn no_api_key_message() {
        let err = MehError::NoApiKey {
            provider: "OpenAI".to_string(),
            provider_lower: "openai".to_string(),
            env_var: "OPENAI_API_KEY".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("No API key"));
        assert!(msg.contains("[provider.openai]"));
    }

    #[test]
    fn rate_limited_with_retry() {
        let err = MehError::RateLimited {
            provider: "anthropic".to_string(),
            retry_after: Some(Duration::from_secs(5)),
        };
        assert!(err.to_string().contains("5s"));
    }

    #[test]
    fn rate_limited_without_retry() {
        let err = MehError::RateLimited {
            provider: "anthropic".to_string(),
            retry_after: None,
        };
        assert!(err.to_string().contains("backoff"));
    }

    #[test]
    fn command_timeout_message() {
        let err = MehError::CommandTimeout {
            command: "npm install".to_string(),
            seconds: 300,
        };
        let msg = err.to_string();
        assert!(msg.contains("300s"));
        assert!(msg.contains("npm install"));
    }

    #[test]
    fn context_window_message() {
        let err = MehError::ContextWindowExceeded {
            used: 250_000,
            limit: 200_000,
        };
        let msg = err.to_string();
        assert!(msg.contains("250000"));
        assert!(msg.contains("200000"));
    }

    #[test]
    fn mcp_server_error() {
        let err = MehError::McpServerFailed {
            server: "filesystem".to_string(),
            reason: "command not found: npx".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("filesystem"));
        assert!(msg.contains("mcp_settings.json"));
    }

    #[test]
    fn invalid_config_message() {
        let err = MehError::InvalidConfig {
            file: "~/.meh/config.toml".to_string(),
            reason: "unknown field 'typo'".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("delete it to use defaults"));
    }

    #[test]
    fn task_not_found_message() {
        let err = MehError::TaskNotFound {
            task_id: "abc-123".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("abc-123"));
        assert!(msg.contains("--resume"));
    }

    #[test]
    fn permission_denied_message() {
        let err = MehError::PermissionDenied {
            tool: "execute_command".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("execute_command"));
        assert!(msg.contains("--yolo"));
    }

    #[test]
    fn model_not_found_message() {
        let err = MehError::ModelNotFound {
            provider: "anthropic".to_string(),
            model: "claude-5".to_string(),
            suggestions: "claude-sonnet-4, claude-opus-4".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("claude-5"));
        assert!(msg.contains("claude-sonnet-4"));
    }

    #[test]
    fn mcp_tool_error_message() {
        let err = MehError::McpToolError {
            server: "fs".to_string(),
            tool: "read_file".to_string(),
            message: "file not found".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("fs"));
        assert!(msg.contains("read_file"));
    }

    #[test]
    fn map_provider_auth_error() {
        let err: anyhow::Error = ProviderError::Auth("invalid key".to_string()).into();
        let mapped = map_provider_error(&err, "anthropic");
        assert!(matches!(mapped, MehError::AuthFailed { .. }));
    }

    #[test]
    fn map_provider_rate_limit_error() {
        let err: anyhow::Error = ProviderError::RateLimit {
            retry_after: Some(Duration::from_secs(10)),
        }
        .into();
        let mapped = map_provider_error(&err, "openai");
        assert!(matches!(mapped, MehError::RateLimited { .. }));
    }

    #[test]
    fn map_provider_server_error() {
        let err: anyhow::Error = ProviderError::Server {
            status: 500,
            message: "internal".to_string(),
        }
        .into();
        let mapped = map_provider_error(&err, "gemini");
        assert!(matches!(mapped, MehError::ProviderServerError { .. }));
    }

    #[test]
    fn map_unknown_error() {
        let err = anyhow::anyhow!("network timeout");
        let mapped = map_provider_error(&err, "anthropic");
        assert!(matches!(mapped, MehError::ConnectionFailed { .. }));
    }

    #[test]
    fn default_env_var_known_providers() {
        assert_eq!(default_env_var("anthropic"), "ANTHROPIC_API_KEY");
        assert_eq!(default_env_var("openai"), "OPENAI_API_KEY");
        assert_eq!(default_env_var("gemini"), "GEMINI_API_KEY");
        assert_eq!(default_env_var("openrouter"), "OPENROUTER_API_KEY");
    }

    #[test]
    fn default_env_var_unknown_provider() {
        assert_eq!(default_env_var("custom"), "CUSTOM_API_KEY");
    }

    #[test]
    fn validate_config_missing_key() {
        let config = AppConfig {
            provider: crate::state::config::ProviderConfig {
                anthropic: crate::state::config::ProviderSettings {
                    api_key_env: Some("MEH_NONEXISTENT_KEY_XYZ".to_string()),
                    api_key: None,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let errors = validate_config(&config);
        assert!(!errors.is_empty());
        assert!(matches!(&errors[0], MehError::NoApiKey { .. }));
    }
}
