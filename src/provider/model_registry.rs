//! Centralized model registry — single source of truth for all model metadata.
//!
//! Loads model definitions from `~/.config/meh/models.toml` or falls back to
//! built-in defaults. All pricing, capabilities, context windows, and provider
//! mappings come from here. Adding a new model is a one-line TOML edit.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// Global registry instance, initialized once at startup.
static GLOBAL_REGISTRY: OnceLock<ModelRegistry> = OnceLock::new();

/// Initialize the global model registry. Call once at app startup.
pub fn init_global(path: Option<&Path>) {
    let _ = GLOBAL_REGISTRY.set(ModelRegistry::load(path));
}

/// Get the global model registry. Panics if not initialized.
pub fn global() -> &'static ModelRegistry {
    GLOBAL_REGISTRY.get_or_init(|| ModelRegistry::load(None))
}

/// A single model definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct ModelDef {
    /// Model ID used in API calls (e.g., "claude-sonnet-4-6").
    pub id: String,
    /// Human-readable name (e.g., "Claude Sonnet 4.6").
    pub name: String,
    /// Provider name (e.g., "anthropic", "openai", "gemini").
    pub provider: String,
    /// Context window size in tokens.
    pub context_window: u32,
    /// Maximum output tokens.
    pub max_output: u32,
    /// Price per million input tokens (USD).
    pub input_price_per_mtok: f64,
    /// Price per million output tokens (USD).
    pub output_price_per_mtok: f64,
    /// Price per million cache-read tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_price_per_mtok: Option<f64>,
    /// Price per million cache-write tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_price_per_mtok: Option<f64>,
    /// Price per million thinking tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_price_per_mtok: Option<f64>,
    /// Whether this model supports native tool calling.
    #[serde(default)]
    pub supports_tools: bool,
    /// Whether this model supports extended thinking.
    #[serde(default)]
    pub supports_thinking: bool,
    /// Whether this model supports image inputs.
    #[serde(default)]
    pub supports_images: bool,
    /// Whether this is the default model for its provider.
    #[serde(default)]
    pub default: bool,
}

/// TOML file structure.
#[derive(Debug, Serialize, Deserialize)]
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
    pub fn load(path: Option<&Path>) -> Self {
        let defs = path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str::<ModelsFile>(&s).ok())
            .map_or_else(Self::builtin_defaults, |f| f.models);

        Self::from_defs(defs)
    }

    /// Build a registry from a list of model definitions.
    fn from_defs(defs: Vec<ModelDef>) -> Self {
        let mut models = HashMap::new();
        let mut by_provider: HashMap<String, Vec<String>> = HashMap::new();
        let mut defaults = HashMap::new();

        for def in defs {
            by_provider
                .entry(def.provider.clone())
                .or_default()
                .push(def.id.clone());
            if def.default {
                defaults.insert(def.provider.clone(), def.id.clone());
            }
            models.insert(def.id.clone(), def);
        }

        Self {
            models,
            by_provider,
            defaults,
        }
    }

    /// Get a model by ID.
    pub fn get(&self, model_id: &str) -> Option<&ModelDef> {
        self.models.get(model_id)
    }

    /// Get all model IDs for a provider, in insertion order.
    pub fn models_for_provider(&self, provider: &str) -> Vec<&ModelDef> {
        self.by_provider.get(provider).map_or_else(Vec::new, |ids| {
            ids.iter().filter_map(|id| self.models.get(id)).collect()
        })
    }

    /// Get model IDs for a provider as strings (for UI dropdowns).
    pub fn model_ids_for_provider(&self, provider: &str) -> Vec<String> {
        self.by_provider.get(provider).cloned().unwrap_or_default()
    }

    /// Get the default model ID for a provider.
    pub fn default_for_provider(&self, provider: &str) -> &str {
        self.defaults
            .get(provider)
            .map(String::as_str)
            .or_else(|| {
                self.by_provider
                    .get(provider)
                    .and_then(|ids| ids.first())
                    .map(String::as_str)
            })
            .unwrap_or("claude-sonnet-4-6")
    }

    /// Get pricing for a model: `(input_per_mtok, output_per_mtok)`.
    pub fn pricing(&self, model_id: &str) -> Option<(f64, f64)> {
        self.models
            .get(model_id)
            .map(|m| (m.input_price_per_mtok, m.output_price_per_mtok))
    }

    /// Convert a `ModelDef` to a `provider::ModelInfo`.
    pub fn to_model_info(&self, model_id: &str) -> Option<crate::provider::ModelInfo> {
        self.models
            .get(model_id)
            .map(|m| crate::provider::ModelInfo {
                id: m.id.clone(),
                name: m.name.clone(),
                provider: m.provider.clone(),
                max_tokens: m.max_output,
                context_window: m.context_window,
                supports_tools: m.supports_tools,
                supports_thinking: m.supports_thinking,
                supports_images: m.supports_images,
                input_price_per_mtok: m.input_price_per_mtok,
                output_price_per_mtok: m.output_price_per_mtok,
                cache_read_price_per_mtok: m.cache_read_price_per_mtok,
                cache_write_price_per_mtok: m.cache_write_price_per_mtok,
                thinking_price_per_mtok: m.thinking_price_per_mtok,
            })
    }

    /// All known provider names.
    pub fn providers(&self) -> Vec<&str> {
        let mut providers: Vec<&str> = self.by_provider.keys().map(String::as_str).collect();
        providers.sort_unstable();
        providers
    }

    /// Write the current registry to a TOML file.
    pub fn write_defaults(path: &Path) -> anyhow::Result<()> {
        let file = ModelsFile {
            models: Self::builtin_defaults(),
        };
        let toml_str = toml::to_string_pretty(&file)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml_str)?;
        Ok(())
    }

    /// Built-in model definitions (used when no models.toml exists).
    #[allow(clippy::too_many_lines)]
    pub fn builtin_defaults() -> Vec<ModelDef> {
        vec![
            // Anthropic
            ModelDef {
                id: "claude-sonnet-4-6".into(),
                name: "Claude Sonnet 4.6".into(),
                provider: "anthropic".into(),
                context_window: 1_000_000,
                max_output: 64_000,
                input_price_per_mtok: 3.0,
                output_price_per_mtok: 15.0,
                cache_read_price_per_mtok: Some(0.30),
                cache_write_price_per_mtok: Some(3.75),
                thinking_price_per_mtok: None,
                supports_tools: true,
                supports_thinking: true,
                supports_images: true,
                default: true,
            },
            ModelDef {
                id: "claude-opus-4-6".into(),
                name: "Claude Opus 4.6".into(),
                provider: "anthropic".into(),
                context_window: 1_000_000,
                max_output: 128_000,
                input_price_per_mtok: 5.0,
                output_price_per_mtok: 25.0,
                cache_read_price_per_mtok: Some(0.50),
                cache_write_price_per_mtok: Some(6.25),
                thinking_price_per_mtok: None,
                supports_tools: true,
                supports_thinking: true,
                supports_images: true,
                default: false,
            },
            ModelDef {
                id: "claude-haiku-4-5".into(),
                name: "Claude Haiku 4.5".into(),
                provider: "anthropic".into(),
                context_window: 200_000,
                max_output: 64_000,
                input_price_per_mtok: 1.0,
                output_price_per_mtok: 5.0,
                cache_read_price_per_mtok: Some(0.10),
                cache_write_price_per_mtok: Some(1.25),
                thinking_price_per_mtok: None,
                supports_tools: true,
                supports_thinking: true,
                supports_images: true,
                default: false,
            },
            // OpenAI
            ModelDef {
                id: "gpt-5.4".into(),
                name: "GPT-5.4".into(),
                provider: "openai".into(),
                context_window: 1_000_000,
                max_output: 128_000,
                input_price_per_mtok: 2.0,
                output_price_per_mtok: 8.0,
                cache_read_price_per_mtok: None,
                cache_write_price_per_mtok: None,
                thinking_price_per_mtok: None,
                supports_tools: true,
                supports_thinking: false,
                supports_images: true,
                default: true,
            },
            ModelDef {
                id: "gpt-5.4-mini".into(),
                name: "GPT-5.4 Mini".into(),
                provider: "openai".into(),
                context_window: 400_000,
                max_output: 128_000,
                input_price_per_mtok: 0.4,
                output_price_per_mtok: 1.6,
                cache_read_price_per_mtok: None,
                cache_write_price_per_mtok: None,
                thinking_price_per_mtok: None,
                supports_tools: true,
                supports_thinking: false,
                supports_images: true,
                default: false,
            },
            ModelDef {
                id: "gpt-5.4-nano".into(),
                name: "GPT-5.4 Nano".into(),
                provider: "openai".into(),
                context_window: 400_000,
                max_output: 128_000,
                input_price_per_mtok: 0.1,
                output_price_per_mtok: 0.4,
                cache_read_price_per_mtok: None,
                cache_write_price_per_mtok: None,
                thinking_price_per_mtok: None,
                supports_tools: true,
                supports_thinking: false,
                supports_images: true,
                default: false,
            },
            // Google Gemini
            ModelDef {
                id: "gemini-3.1-pro-preview".into(),
                name: "Gemini 3.1 Pro".into(),
                provider: "gemini".into(),
                context_window: 1_000_000,
                max_output: 65_536,
                input_price_per_mtok: 1.25,
                output_price_per_mtok: 10.0,
                cache_read_price_per_mtok: None,
                cache_write_price_per_mtok: None,
                thinking_price_per_mtok: None,
                supports_tools: true,
                supports_thinking: true,
                supports_images: true,
                default: true,
            },
            ModelDef {
                id: "gemini-3-flash-preview".into(),
                name: "Gemini 3 Flash".into(),
                provider: "gemini".into(),
                context_window: 1_000_000,
                max_output: 65_536,
                input_price_per_mtok: 0.15,
                output_price_per_mtok: 0.60,
                cache_read_price_per_mtok: None,
                cache_write_price_per_mtok: None,
                thinking_price_per_mtok: None,
                supports_tools: true,
                supports_thinking: true,
                supports_images: true,
                default: false,
            },
            ModelDef {
                id: "gemini-2.5-pro".into(),
                name: "Gemini 2.5 Pro".into(),
                provider: "gemini".into(),
                context_window: 1_000_000,
                max_output: 65_536,
                input_price_per_mtok: 1.25,
                output_price_per_mtok: 10.0,
                cache_read_price_per_mtok: None,
                cache_write_price_per_mtok: None,
                thinking_price_per_mtok: None,
                supports_tools: true,
                supports_thinking: true,
                supports_images: true,
                default: false,
            },
            ModelDef {
                id: "gemini-2.5-flash".into(),
                name: "Gemini 2.5 Flash".into(),
                provider: "gemini".into(),
                context_window: 1_000_000,
                max_output: 65_536,
                input_price_per_mtok: 0.15,
                output_price_per_mtok: 0.60,
                cache_read_price_per_mtok: None,
                cache_write_price_per_mtok: None,
                thinking_price_per_mtok: None,
                supports_tools: true,
                supports_thinking: true,
                supports_images: true,
                default: false,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn model_ids_for_provider() {
        let registry = ModelRegistry::load(None);
        let ids = registry.model_ids_for_provider("openai");
        assert!(ids.contains(&"gpt-5.4".to_string()));
    }

    #[test]
    fn default_for_provider() {
        let registry = ModelRegistry::load(None);
        assert_eq!(
            registry.default_for_provider("anthropic"),
            "claude-sonnet-4-6"
        );
        assert_eq!(registry.default_for_provider("openai"), "gpt-5.4");
        assert_eq!(
            registry.default_for_provider("gemini"),
            "gemini-3.1-pro-preview"
        );
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
        assert!(info.supports_tools);
    }

    #[test]
    fn unknown_model_returns_none() {
        let registry = ModelRegistry::load(None);
        assert!(registry.get("nonexistent").is_none());
        assert!(registry.pricing("nonexistent").is_none());
    }

    #[test]
    fn load_from_toml_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("models.toml");
        std::fs::write(
            &path,
            r#"
[[models]]
id = "custom-model"
name = "Custom"
provider = "custom"
context_window = 100000
max_output = 4096
input_price_per_mtok = 1.0
output_price_per_mtok = 5.0
default = true
"#,
        )
        .unwrap();
        let registry = ModelRegistry::load(Some(&path));
        assert!(registry.get("custom-model").is_some());
        assert_eq!(registry.default_for_provider("custom"), "custom-model");
    }

    #[test]
    fn providers_list() {
        let registry = ModelRegistry::load(None);
        let providers = registry.providers();
        assert!(providers.contains(&"anthropic"));
        assert!(providers.contains(&"openai"));
        assert!(providers.contains(&"gemini"));
    }

    #[test]
    fn write_defaults_to_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("models.toml");
        ModelRegistry::write_defaults(&path).unwrap();
        assert!(path.exists());
        let registry = ModelRegistry::load(Some(&path));
        assert!(registry.get("claude-sonnet-4-6").is_some());
    }

    #[test]
    fn default_fallback_for_unknown_provider() {
        let registry = ModelRegistry::load(None);
        assert_eq!(
            registry.default_for_provider("unknown"),
            "claude-sonnet-4-6"
        );
    }
}
