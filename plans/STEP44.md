# STEP 44 — Centralized Model Registry from Config File

## Objective
Extract all hardcoded model IDs, pricing, and capabilities into a single `~/.config/meh/models.toml` file. All provider constructors, pricing lookups, settings UI dropdowns, and model resolution read from this one registry. Adding a new model becomes a one-line TOML edit.

## Prerequisites
- STEP 43 complete (settings UI)

## Context: Current Fragmentation

The audit found **85+ hardcoded model ID strings across 14 files** with no shared registry:

| Layer | Files | Count | Problem |
|-------|-------|-------|---------|
| Provider defaults | anthropic.rs, openai.rs, gemini.rs, openrouter.rs | 5 | Each hardcodes its own default in constructor |
| Model info functions | openai.rs (`openai_model_info`), gemini.rs (`gemini_model_info`) | 7 | Private match statements |
| Pricing lookup | cost.rs (`get_known_pricing`) | 23 | Disconnected from providers |
| Settings UI | settings_view.rs (`build_api_rows`) | 13 | Static dropdown lists |
| Provider resolution | resolve.rs (`default_model_for_provider`) | 4 | Separate defaults |
| Fallback defaults | controller.rs | 1 | Inconsistent |

---

## Detailed Instructions

### 44.1 Define the models.toml format

```toml
# ~/.config/meh/models.toml
# Model definitions loaded at startup. Edit to add/remove models.

[[models]]
id = "claude-sonnet-4-6"
name = "Claude Sonnet 4.6"
provider = "anthropic"
context_window = 1_000_000
max_output = 64_000
input_price_per_mtok = 3.0
output_price_per_mtok = 15.0
supports_tools = true
supports_thinking = true
supports_images = true
default = true  # default model for this provider

[[models]]
id = "claude-opus-4-6"
name = "Claude Opus 4.6"
provider = "anthropic"
context_window = 1_000_000
max_output = 128_000
input_price_per_mtok = 5.0
output_price_per_mtok = 25.0
supports_tools = true
supports_thinking = true
supports_images = true

[[models]]
id = "claude-haiku-4-5"
name = "Claude Haiku 4.5"
provider = "anthropic"
context_window = 200_000
max_output = 64_000
input_price_per_mtok = 1.0
output_price_per_mtok = 5.0
supports_tools = true
supports_thinking = true
supports_images = true

[[models]]
id = "gpt-5.4"
name = "GPT-5.4"
provider = "openai"
context_window = 1_000_000
max_output = 128_000
input_price_per_mtok = 2.0
output_price_per_mtok = 8.0
supports_tools = true
supports_thinking = false
supports_images = true
default = true

[[models]]
id = "gpt-5.4-mini"
name = "GPT-5.4 Mini"
provider = "openai"
context_window = 400_000
max_output = 128_000
input_price_per_mtok = 0.4
output_price_per_mtok = 1.6
supports_tools = true
supports_thinking = false
supports_images = true

[[models]]
id = "gpt-5.4-nano"
name = "GPT-5.4 Nano"
provider = "openai"
context_window = 400_000
max_output = 128_000
input_price_per_mtok = 0.1
output_price_per_mtok = 0.4
supports_tools = true
supports_thinking = false
supports_images = true

[[models]]
id = "gemini-3.1-pro-preview"
name = "Gemini 3.1 Pro"
provider = "gemini"
context_window = 1_000_000
max_output = 65_536
input_price_per_mtok = 1.25
output_price_per_mtok = 10.0
supports_tools = true
supports_thinking = true
supports_images = true
default = true

[[models]]
id = "gemini-3-flash-preview"
name = "Gemini 3 Flash"
provider = "gemini"
context_window = 1_000_000
max_output = 65_536
input_price_per_mtok = 0.15
output_price_per_mtok = 0.60
supports_tools = true
supports_thinking = true
supports_images = true

[[models]]
id = "gemini-2.5-pro"
name = "Gemini 2.5 Pro"
provider = "gemini"
context_window = 1_000_000
max_output = 65_536
input_price_per_mtok = 1.25
output_price_per_mtok = 10.0
supports_tools = true
supports_thinking = true
supports_images = true

[[models]]
id = "gemini-2.5-flash"
name = "Gemini 2.5 Flash"
provider = "gemini"
context_window = 1_000_000
max_output = 65_536
input_price_per_mtok = 0.15
output_price_per_mtok = 0.60
supports_tools = true
supports_thinking = true
supports_images = true
```

### 44.2 Create ModelRegistry (`src/provider/model_registry.rs`)

```rust
//! Centralized model registry — single source of truth for all model metadata.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// A single model definition from models.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDef {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub context_window: u32,
    pub max_output: u32,
    pub input_price_per_mtok: f64,
    pub output_price_per_mtok: f64,
    #[serde(default)]
    pub cache_read_price_per_mtok: Option<f64>,
    #[serde(default)]
    pub cache_write_price_per_mtok: Option<f64>,
    #[serde(default)]
    pub thinking_price_per_mtok: Option<f64>,
    #[serde(default)]
    pub supports_tools: bool,
    #[serde(default)]
    pub supports_thinking: bool,
    #[serde(default)]
    pub supports_images: bool,
    #[serde(default)]
    pub default: bool,
}

/// The TOML file structure.
#[derive(Debug, Deserialize)]
struct ModelsFile {
    models: Vec<ModelDef>,
}

/// Central registry holding all known models.
pub struct ModelRegistry {
    models: HashMap<String, ModelDef>,
    by_provider: HashMap<String, Vec<String>>,
    defaults: HashMap<String, String>,
}

impl ModelRegistry {
    /// Load from a TOML file, falling back to built-in defaults.
    pub fn load(path: Option<&Path>) -> Self { ... }

    /// Get a model by ID.
    pub fn get(&self, model_id: &str) -> Option<&ModelDef> { ... }

    /// Get all models for a provider, sorted by name.
    pub fn models_for_provider(&self, provider: &str) -> Vec<&ModelDef> { ... }

    /// Get the default model ID for a provider.
    pub fn default_for_provider(&self, provider: &str) -> &str { ... }

    /// Get pricing for a model (replaces get_known_pricing).
    pub fn pricing(&self, model_id: &str) -> Option<(f64, f64)> { ... }

    /// Convert a ModelDef to a provider::ModelInfo.
    pub fn to_model_info(&self, model_id: &str) -> Option<crate::provider::ModelInfo> { ... }

    /// Get all model IDs as strings (for autocomplete).
    pub fn all_model_ids(&self) -> Vec<&str> { ... }

    /// Built-in defaults when no models.toml exists.
    fn builtin_defaults() -> Vec<ModelDef> { ... }
}
```

### 44.3 Refactor consumers to use the registry

**A. Provider constructors** — Accept `ModelDef` instead of hardcoding:

```rust
// BEFORE (anthropic.rs):
model_info: ModelInfo {
    id: "claude-sonnet-4-6".to_string(),
    name: "Claude Sonnet 4.6".to_string(),
    ...
}

// AFTER:
// ModelInfo built from registry.to_model_info(model_id)
```

Each provider's `new()` takes a `model_id: &str` and looks it up from the registry.

**B. Cost tracking** — Replace `get_known_pricing()` match statement:

```rust
// BEFORE:
pub fn get_known_pricing(model_id: &str) -> Option<(f64, f64)> {
    match model_id {
        "claude-sonnet-4-6" => Some((3.0, 15.0)),
        // ... 23 entries
    }
}

// AFTER:
pub fn get_known_pricing(model_id: &str, registry: &ModelRegistry) -> Option<(f64, f64)> {
    registry.pricing(model_id)
}
```

**C. Settings UI** — Generate dropdowns dynamically:

```rust
// BEFORE:
options: vec![
    "claude-sonnet-4-6".to_string(),
    "claude-opus-4-6".to_string(),
    ...
]

// AFTER:
options: registry.models_for_provider("anthropic")
    .iter()
    .map(|m| m.id.clone())
    .collect()
```

**D. Provider resolution** — Use registry defaults:

```rust
// BEFORE:
fn default_model_for_provider(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "claude-sonnet-4-6",
        "openai" => "gpt-5.4",
        ...
    }
}

// AFTER:
fn default_model_for_provider(provider: &str, registry: &ModelRegistry) -> &str {
    registry.default_for_provider(provider)
}
```

### 44.4 Wire registry into the application

The `ModelRegistry` is loaded once at startup and shared:

```rust
// In App::new() or App::run():
let models_path = AppConfig::config_dir().join("models.toml");
let model_registry = Arc::new(ModelRegistry::load(
    if models_path.exists() { Some(&models_path) } else { None }
));

// Pass to Controller::new():
Controller::new(state, permission_mode, model_registry.clone())

// Pass to run_tui_async():
run_tui_async(..., &model_registry)
```

### 44.5 Auto-generate default models.toml on first run

If `~/.config/meh/models.toml` doesn't exist, write the built-in defaults:

```rust
if !models_path.exists() {
    let defaults = ModelRegistry::builtin_defaults();
    let toml_str = toml::to_string_pretty(&ModelsFile { models: defaults })?;
    std::fs::write(&models_path, toml_str)?;
}
```

This lets users customize by editing the file, while first-run works out of the box.

---

## Files Changed

| File | Change |
|------|--------|
| **NEW** `src/provider/model_registry.rs` | ModelDef, ModelRegistry, load/query/defaults |
| **NEW** `~/.config/meh/models.toml` | Auto-generated on first run |
| `src/provider/mod.rs` | Register module, ModelInfo conversion |
| `src/provider/anthropic.rs` | Use registry for ModelInfo instead of hardcoded |
| `src/provider/openai.rs` | Remove `openai_model_info()` match, use registry |
| `src/provider/gemini.rs` | Remove `gemini_model_info()` match, use registry |
| `src/provider/resolve.rs` | Use `registry.default_for_provider()` |
| `src/util/cost.rs` | `get_known_pricing` queries registry |
| `src/tui/settings_view.rs` | Generate model dropdowns from registry |
| `src/controller/mod.rs` | Hold `Arc<ModelRegistry>`, pass to providers |
| `src/app.rs` | Load registry at startup, pass to controller + TUI |

---

## Tests

```rust
#[cfg(test)]
mod registry_tests {
    #[test]
    fn load_builtin_defaults() {
        let registry = ModelRegistry::load(None);
        assert!(registry.get("claude-sonnet-4-6").is_some());
        assert!(registry.get("gpt-5.4").is_some());
        assert!(registry.get("gemini-3.1-pro-preview").is_some());
    }

    #[test]
    fn models_for_provider() {
        let registry = ModelRegistry::load(None);
        let anthropic = registry.models_for_provider("anthropic");
        assert!(anthropic.len() >= 3);
        assert!(anthropic.iter().any(|m| m.id == "claude-sonnet-4-6"));
    }

    #[test]
    fn default_for_provider() {
        let registry = ModelRegistry::load(None);
        assert_eq!(registry.default_for_provider("anthropic"), "claude-sonnet-4-6");
        assert_eq!(registry.default_for_provider("openai"), "gpt-5.4");
    }

    #[test]
    fn pricing_from_registry() {
        let registry = ModelRegistry::load(None);
        let (input, output) = registry.pricing("claude-sonnet-4-6").unwrap();
        assert!((input - 3.0).abs() < f64::EPSILON);
        assert!((output - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn to_model_info_conversion() {
        let registry = ModelRegistry::load(None);
        let info = registry.to_model_info("claude-sonnet-4-6").unwrap();
        assert_eq!(info.id, "claude-sonnet-4-6");
        assert_eq!(info.context_window, 1_000_000);
    }

    #[test]
    fn load_from_toml_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("models.toml");
        std::fs::write(&path, r#"
            [[models]]
            id = "custom-model"
            name = "Custom"
            provider = "custom"
            context_window = 100000
            max_output = 4096
            input_price_per_mtok = 1.0
            output_price_per_mtok = 5.0
            default = true
        "#).unwrap();
        let registry = ModelRegistry::load(Some(&path));
        assert!(registry.get("custom-model").is_some());
    }

    #[test]
    fn unknown_model_returns_none() {
        let registry = ModelRegistry::load(None);
        assert!(registry.get("nonexistent").is_none());
        assert!(registry.pricing("nonexistent").is_none());
    }
}
```

## Acceptance Criteria
- [x] `ModelDef` struct with all model metadata (id, name, provider, pricing, capabilities)
- [x] `ModelRegistry` loads from `~/.config/meh/models.toml` or falls back to built-in defaults
- [x] `models.toml` auto-generated on first run with all current models
- [x] `registry.get()`, `models_for_provider()`, `default_for_provider()`, `pricing()` work
- [x] `to_model_info()` converts ModelDef to provider::ModelInfo
- [x] Settings UI dropdowns generated from registry (not hardcoded)
- [x] `get_known_pricing()` queries registry first, legacy fallback second
- [x] Global registry via `OnceLock` initialized at app startup
- [x] Adding a new model only requires editing models.toml
- [x] All existing tests pass (no regressions)
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes
