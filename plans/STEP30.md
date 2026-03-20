# STEP 30 — Comprehensive Error Messages

## Objective
Implement user-friendly error messages throughout the application. Every error that surfaces to the user should be clear, actionable, and context-specific. No raw error strings or stack traces in the TUI.

## Prerequisites
- All prior steps (this is the final polish step)

## Detailed Instructions

### 30.1 Define error types (`src/util/mod.rs` or new `src/error.rs`)

```rust
//! Application error types with user-friendly messages.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum MehError {
    // === Provider errors ===
    #[error("Authentication failed for {provider}. Check your API key.\n  Hint: Set {env_var} environment variable or add it to ~/.meh/config.toml")]
    AuthFailed {
        provider: String,
        env_var: String,
    },

    #[error("Rate limited by {provider}. {}", retry_msg(.retry_after))]
    RateLimited {
        provider: String,
        retry_after: Option<std::time::Duration>,
    },

    #[error("{provider} server error ({status}): {message}\n  This is a temporary issue. The request will be retried automatically.")]
    ProviderServerError {
        provider: String,
        status: u16,
        message: String,
    },

    #[error("Cannot connect to {provider}. Check your internet connection.\n  URL: {url}")]
    ConnectionFailed {
        provider: String,
        url: String,
    },

    #[error("Model '{model}' not found for {provider}.\n  Available models: {suggestions}")]
    ModelNotFound {
        provider: String,
        model: String,
        suggestions: String,
    },

    #[error("No API key configured for {provider}.\n  Set {env_var} or add to config:\n    [provider.{provider_lower}]\n    api_key_env = \"{env_var}\"")]
    NoApiKey {
        provider: String,
        provider_lower: String,
        env_var: String,
    },

    // === Tool errors ===
    #[error("Tool '{tool}' failed: {reason}")]
    ToolFailed {
        tool: String,
        reason: String,
    },

    #[error("Permission denied for '{tool}'. Use 'y' to approve or --yolo to skip approvals.")]
    PermissionDenied {
        tool: String,
    },

    #[error("Command timed out after {seconds}s: {command}\n  Try increasing the timeout or running the command manually.")]
    CommandTimeout {
        command: String,
        seconds: u64,
    },

    // === MCP errors ===
    #[error("MCP server '{server}' failed to start: {reason}\n  Check the server command in ~/.meh/mcp_settings.json")]
    McpServerFailed {
        server: String,
        reason: String,
    },

    #[error("MCP tool '{tool}' on server '{server}' returned an error: {message}")]
    McpToolError {
        server: String,
        tool: String,
        message: String,
    },

    // === Config errors ===
    #[error("Invalid configuration in {file}: {reason}\n  Fix the file or delete it to use defaults.")]
    InvalidConfig {
        file: String,
        reason: String,
    },

    // === Task errors ===
    #[error("Task '{task_id}' not found in history.\n  Use 'meh' without --resume to start a new task.")]
    TaskNotFound {
        task_id: String,
    },

    #[error("Context window exceeded ({used} / {limit} tokens).\n  The conversation is too long. Start a new task or use a model with a larger context window.")]
    ContextWindowExceeded {
        used: u64,
        limit: u32,
    },
}

fn retry_msg(after: &Option<std::time::Duration>) -> String {
    match after {
        Some(d) => format!("Retrying in {}s.", d.as_secs()),
        None => "Retrying with backoff.".to_string(),
    }
}
```

### 30.2 Error rendering in TUI

When an error occurs, display it in the chat view with:
- Red color for the error text
- A distinct prefix: `⚠ Error: `
- The actionable hint (if any) in yellow/dim
- No stack traces or internal details

```rust
pub fn render_error(error: &MehError) -> ChatMessage {
    ChatMessage {
        role: ChatRole::System,
        content: error.to_string(),
        timestamp: chrono::Utc::now(),
        streaming: false,
    }
}
```

### 30.3 Convert provider errors to MehError

In each provider, map errors to `MehError`:
```rust
fn map_error(err: anyhow::Error, provider: &str) -> MehError {
    if let Some(pe) = err.downcast_ref::<ProviderError>() {
        match pe {
            ProviderError::Auth(msg) => MehError::AuthFailed {
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
            _ => MehError::ToolFailed { tool: provider.to_string(), reason: pe.to_string() },
        }
    } else {
        MehError::ConnectionFailed {
            provider: provider.to_string(),
            url: "unknown".to_string(),
        }
    }
}

fn default_env_var(provider: &str) -> String {
    match provider {
        "anthropic" => "ANTHROPIC_API_KEY".to_string(),
        "openai" => "OPENAI_API_KEY".to_string(),
        "gemini" => "GEMINI_API_KEY".to_string(),
        "openrouter" => "OPENROUTER_API_KEY".to_string(),
        _ => format!("{}_API_KEY", provider.to_uppercase()),
    }
}
```

### 30.4 Startup validation

Before entering the main loop, validate critical configuration:
```rust
pub fn validate_config(config: &AppConfig) -> Vec<MehError> {
    let mut errors = Vec::new();

    // Check that default provider has an API key
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
```

### 30.5 Graceful panic handling

Install a panic hook that restores the terminal before printing the panic message:
```rust
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Restore terminal
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen
        );
        // Print panic info
        original(info);
    }));
}
```

Call this at the very beginning of `main()`.

## Tests

```rust
#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn test_auth_error_message() {
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
    fn test_no_api_key_message() {
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
    fn test_rate_limited_with_retry() {
        let err = MehError::RateLimited {
            provider: "anthropic".to_string(),
            retry_after: Some(Duration::from_secs(5)),
        };
        assert!(err.to_string().contains("5s"));
    }

    #[test]
    fn test_rate_limited_without_retry() {
        let err = MehError::RateLimited {
            provider: "anthropic".to_string(),
            retry_after: None,
        };
        assert!(err.to_string().contains("backoff"));
    }

    #[test]
    fn test_command_timeout_message() {
        let err = MehError::CommandTimeout {
            command: "npm install".to_string(),
            seconds: 300,
        };
        let msg = err.to_string();
        assert!(msg.contains("300s"));
        assert!(msg.contains("npm install"));
    }

    #[test]
    fn test_context_window_message() {
        let err = MehError::ContextWindowExceeded {
            used: 250_000,
            limit: 200_000,
        };
        let msg = err.to_string();
        assert!(msg.contains("250000"));
        assert!(msg.contains("200000"));
    }

    #[test]
    fn test_mcp_server_error() {
        let err = MehError::McpServerFailed {
            server: "filesystem".to_string(),
            reason: "command not found: npx".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("filesystem"));
        assert!(msg.contains("mcp_settings.json"));
    }

    #[test]
    fn test_validate_config_missing_key() {
        let config = AppConfig::default(); // No API keys set
        let errors = validate_config(&config);
        assert!(!errors.is_empty());
        assert!(matches!(&errors[0], MehError::NoApiKey { .. }));
    }

    #[test]
    fn test_validate_config_with_key() {
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");
        let config = AppConfig::default();
        let errors = validate_config(&config);
        assert!(errors.is_empty());
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    fn test_error_display_includes_hints() {
        let err = MehError::InvalidConfig {
            file: "~/.meh/config.toml".to_string(),
            reason: "unknown field 'typo'".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("delete it to use defaults"));
    }
}
```

## Acceptance Criteria
- [ ] Every user-facing error has a clear, actionable message
- [ ] Provider auth errors suggest how to set API key
- [ ] Rate limit errors show retry timing
- [ ] Connection errors suggest checking internet
- [ ] Command timeouts suggest manual execution
- [ ] MCP errors point to settings file
- [ ] Config errors suggest fix or reset
- [ ] Panic hook restores terminal before printing
- [ ] Startup validation catches missing API keys
- [ ] No raw error strings or stack traces in TUI
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass (10+ cases)
- [ ] This is the final step — after this, `cargo test` passes across ALL modules with ZERO warnings
