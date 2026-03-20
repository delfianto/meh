# STEP 18 — Plan Mode Tool Restrictions + Mode-Specific Models

## Objective
Implement per-mode model selection so plan mode and act mode can use different LLM providers and models. After this step, users can configure e.g. a cheaper model for planning and a more capable model for acting.

## Prerequisites
- STEP 17 complete (plan/act mode switching)
- STEP 05, 08, 09, 10 complete (all providers)

## Detailed Instructions

### 18.1 Model selection per mode

Update `Controller` or `TaskManager` to resolve provider+model based on current mode:

```rust
/// Resolve which provider and model to use for a given mode.
fn resolve_model_for_mode(&self, mode: Mode) -> (String, String, Option<u32>) {
    let config = &self.state.config().mode;
    let (mode_config, default_provider) = match mode {
        Mode::Plan => (&config.plan, &config.default_provider_plan),
        Mode::Act => (&config.act, &config.default_provider_act),
    };

    let provider = mode_config.provider
        .as_deref()
        .or(default_provider.as_deref())
        .unwrap_or(&self.state.config().provider.default);

    let model = mode_config.model
        .as_deref()
        .unwrap_or_else(|| self.default_model_for_provider(provider));

    let thinking_budget = mode_config.thinking_budget;

    (provider.to_string(), model.to_string(), thinking_budget)
}

/// Get a sensible default model ID for a provider.
fn default_model_for_provider(provider: &str) -> &str {
    match provider {
        "anthropic" => "claude-sonnet-4-20250514",
        "openai" => "gpt-4.1",
        "gemini" => "gemini-2.5-flash",
        "openrouter" => "anthropic/claude-sonnet-4",
        _ => "claude-sonnet-4-20250514",
    }
}
```

### 18.2 Provider hot-swapping on mode switch

When switching from plan to act mode, if the act mode uses a different provider/model:

```rust
async fn handle_mode_switch(&mut self, new_mode: Mode) -> anyhow::Result<()> {
    let (provider_name, model_id, thinking_budget) = self.resolve_model_for_mode(new_mode);

    // Check if provider/model changed
    let needs_new_provider = provider_name != self.current_provider_name
        || model_id != self.current_model_id;

    if needs_new_provider {
        // Create new provider instance
        let api_key = self.state.resolve_api_key(&provider_name).await
            .ok_or_else(|| anyhow::anyhow!("No API key for provider: {provider_name}"))?;
        let new_provider = crate::provider::create_provider(&provider_name, &api_key, None)?;

        // Update model config
        let new_config = ModelConfig {
            model_id: model_id.clone(),
            max_tokens: new_provider.model_info().max_tokens,
            temperature: None,
            thinking_budget,
        };

        // Send to agent: new provider + config
        // Agent will use new provider for next API call
        self.send_to_agent(AgentMessage::ProviderSwitch {
            provider: new_provider,
            config: new_config,
        })?;

        self.current_provider_name = provider_name;
        self.current_model_id = model_id;
    }

    // Send mode switch
    self.send_to_agent(AgentMessage::ModeSwitch(new_mode))?;
    self.current_mode = new_mode;

    // Update TUI
    self.send_ui(UiUpdate::StatusUpdate {
        mode: Some(format!("{new_mode:?}").to_uppercase()),
        tokens: None,
        cost: None,
        is_streaming: None,
    })?;

    Ok(())
}
```

### 18.3 Add ProviderSwitch to AgentMessage

```rust
pub enum AgentMessage {
    ToolCallResult(ToolCallResult),
    Cancel,
    ModeSwitch(Mode),
    ProviderSwitch {
        provider: Box<dyn Provider>,
        config: ModelConfig,
    },
    ConfigUpdate(ModelConfig),
}
```

### 18.4 Agent handles ProviderSwitch

```rust
AgentMessage::ProviderSwitch { provider, config } => {
    self.provider = provider;
    self.config = config;
    tracing::info!(
        model = %self.config.model_id,
        "Provider switched for mode change"
    );
}
```

### 18.5 Config example

```toml
[mode]
default = "plan_then_act"
strict_plan = false

[mode.plan]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"  # Cheap model for planning
thinking_budget = 5000

[mode.act]
provider = "anthropic"
model = "claude-sonnet-4-20250514"  # Capable model for acting
thinking_budget = 10000
```

### 18.6 Update TUI status bar to show model

The status bar should display:
```
[PLAN] anthropic/claude-haiku-4-5-20251001  |  tokens: 0  |  $0.000
```

And after mode switch:
```
[ACT] anthropic/claude-sonnet-4-20250514  |  tokens: 1.2k  |  $0.003
```

## Tests

```rust
#[cfg(test)]
mod model_selection_tests {
    use super::*;

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
        let (provider, model, budget) = resolve_model_for_mode(&config, Mode::Plan);
        assert_eq!(provider, "anthropic");
        assert_eq!(model, "claude-haiku-4-5-20251001");
        assert_eq!(budget, Some(5000));
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
        let (provider, model, budget) = resolve_model_for_mode(&config, Mode::Act);
        assert_eq!(provider, "openai");
        assert_eq!(model, "gpt-4.1");
        assert!(budget.is_none());
    }

    #[test]
    fn test_resolve_model_falls_back_to_default() {
        let config = AppConfig::default();
        let (provider, model, _) = resolve_model_for_mode(&config, Mode::Plan);
        assert_eq!(provider, "anthropic"); // Default provider
        assert!(!model.is_empty()); // Has a default model
    }

    #[test]
    fn test_default_model_for_provider() {
        assert_eq!(default_model_for_provider("anthropic"), "claude-sonnet-4-20250514");
        assert_eq!(default_model_for_provider("openai"), "gpt-4.1");
        assert_eq!(default_model_for_provider("gemini"), "gemini-2.5-flash");
    }

    #[test]
    fn test_needs_new_provider_detection() {
        // Same provider+model → no switch needed
        // Different model same provider → switch needed
        // Different provider → switch needed
    }
}
```

## Acceptance Criteria
- [ ] Plan mode can use a different provider/model than act mode
- [ ] Mode switch triggers provider hot-swap when models differ
- [ ] Thinking budget is mode-specific
- [ ] Fallback to default provider/model when mode-specific not configured
- [ ] Status bar shows current provider/model, updates on switch
- [ ] Conversation history preserved across mode switch (messages carry over)
- [ ] API keys resolved for the correct provider on switch
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass
