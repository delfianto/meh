//! Mode-specific model resolution — selects provider, model, and thinking budget
//! based on the current mode (Plan/Act) and configuration.

use crate::state::config::AppConfig;
use crate::state::task_state::Mode;

/// Resolved model selection for a given mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModel {
    /// Provider name (e.g., "anthropic", "openai").
    pub provider: String,
    /// Model identifier (e.g., "claude-sonnet-4-6").
    pub model_id: String,
    /// Thinking token budget, if configured.
    pub thinking_budget: Option<u32>,
}

/// Resolve which provider, model, and thinking budget to use for a given mode.
pub fn resolve_model_for_mode(config: &AppConfig, mode: Mode) -> ResolvedModel {
    let mode_config = match mode {
        Mode::Plan => &config.mode.plan,
        Mode::Act => &config.mode.act,
    };

    let provider = mode_config
        .provider
        .as_deref()
        .unwrap_or(&config.provider.default)
        .to_string();

    let model_id = mode_config
        .model
        .as_deref()
        .unwrap_or_else(|| default_model_for_provider(&provider))
        .to_string();

    let thinking_budget = mode_config.thinking_budget;

    ResolvedModel {
        provider,
        model_id,
        thinking_budget,
    }
}

/// Get a sensible default model ID for a provider.
pub fn default_model_for_provider(provider: &str) -> &str {
    match provider {
        "openai" => "gpt-4.1",
        "gemini" => "gemini-2.5-flash",
        "openrouter" => "anthropic/claude-sonnet-4",
        // Default to Anthropic's flagship model for unknown providers
        _ => "claude-sonnet-4-6",
    }
}

/// Check whether two resolved models require a provider switch.
pub fn needs_provider_switch(current: &ResolvedModel, new: &ResolvedModel) -> bool {
    current.provider != new.provider || current.model_id != new.model_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::config::{ModeConfig, ModeModelConfig};

    #[test]
    fn test_resolve_model_plan_mode() {
        let config = AppConfig {
            mode: ModeConfig {
                plan: ModeModelConfig {
                    provider: Some("anthropic".to_string()),
                    model: Some("claude-haiku-4-5-20251001".to_string()),
                    thinking_budget: Some(5000),
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolve_model_for_mode(&config, Mode::Plan);
        assert_eq!(resolved.provider, "anthropic");
        assert_eq!(resolved.model_id, "claude-haiku-4-5-20251001");
        assert_eq!(resolved.thinking_budget, Some(5000));
    }

    #[test]
    fn test_resolve_model_act_mode() {
        let config = AppConfig {
            mode: ModeConfig {
                act: ModeModelConfig {
                    provider: Some("openai".to_string()),
                    model: Some("gpt-4.1".to_string()),
                    thinking_budget: None,
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolve_model_for_mode(&config, Mode::Act);
        assert_eq!(resolved.provider, "openai");
        assert_eq!(resolved.model_id, "gpt-4.1");
        assert!(resolved.thinking_budget.is_none());
    }

    #[test]
    fn test_resolve_model_falls_back_to_default_provider() {
        let config = AppConfig::default();
        let resolved = resolve_model_for_mode(&config, Mode::Plan);
        assert_eq!(resolved.provider, "anthropic");
        assert_eq!(resolved.model_id, "claude-sonnet-4-6");
    }

    #[test]
    fn test_resolve_model_mode_provider_overrides_default() {
        let config = AppConfig {
            mode: ModeConfig {
                plan: ModeModelConfig {
                    provider: Some("gemini".to_string()),
                    model: None,
                    thinking_budget: None,
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolve_model_for_mode(&config, Mode::Plan);
        assert_eq!(resolved.provider, "gemini");
        assert_eq!(resolved.model_id, "gemini-2.5-flash");
    }

    #[test]
    fn test_resolve_model_different_modes_different_models() {
        let config = AppConfig {
            mode: ModeConfig {
                plan: ModeModelConfig {
                    provider: Some("anthropic".to_string()),
                    model: Some("claude-haiku-4-5-20251001".to_string()),
                    thinking_budget: Some(5000),
                },
                act: ModeModelConfig {
                    provider: Some("anthropic".to_string()),
                    model: Some("claude-sonnet-4-6".to_string()),
                    thinking_budget: Some(10000),
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let plan = resolve_model_for_mode(&config, Mode::Plan);
        let act = resolve_model_for_mode(&config, Mode::Act);
        assert_ne!(plan.model_id, act.model_id);
        assert_ne!(plan.thinking_budget, act.thinking_budget);
    }

    #[test]
    fn test_default_model_for_provider() {
        assert_eq!(default_model_for_provider("anthropic"), "claude-sonnet-4-6");
        assert_eq!(default_model_for_provider("openai"), "gpt-4.1");
        assert_eq!(default_model_for_provider("gemini"), "gemini-2.5-flash");
        assert_eq!(
            default_model_for_provider("openrouter"),
            "anthropic/claude-sonnet-4"
        );
        assert_eq!(default_model_for_provider("unknown"), "claude-sonnet-4-6");
    }

    #[test]
    fn test_needs_provider_switch_same() {
        let a = ResolvedModel {
            provider: "anthropic".to_string(),
            model_id: "claude-sonnet-4".to_string(),
            thinking_budget: None,
        };
        assert!(!needs_provider_switch(&a, &a));
    }

    #[test]
    fn test_needs_provider_switch_different_model() {
        let a = ResolvedModel {
            provider: "anthropic".to_string(),
            model_id: "claude-haiku-4-5".to_string(),
            thinking_budget: None,
        };
        let b = ResolvedModel {
            provider: "anthropic".to_string(),
            model_id: "claude-sonnet-4".to_string(),
            thinking_budget: None,
        };
        assert!(needs_provider_switch(&a, &b));
    }

    #[test]
    fn test_needs_provider_switch_different_provider() {
        let a = ResolvedModel {
            provider: "anthropic".to_string(),
            model_id: "claude-sonnet-4".to_string(),
            thinking_budget: None,
        };
        let b = ResolvedModel {
            provider: "openai".to_string(),
            model_id: "gpt-4.1".to_string(),
            thinking_budget: None,
        };
        assert!(needs_provider_switch(&a, &b));
    }
}
