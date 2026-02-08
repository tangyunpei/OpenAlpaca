//! Model registry: maps model IDs to provider types and pricing info.

use crate::key_pool::ProviderType;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::RwLock;

/// Pricing information for a model.
#[derive(Debug, Clone)]
pub struct PricingInfo {
    pub input_price_per_million: f64,
    pub output_price_per_million: f64,
}

/// Serializable model entry for API responses.
#[derive(Debug, Clone, Serialize)]
pub struct ModelEntry {
    pub id: String,
    pub provider: String,
    pub context_window: u32,
    pub input_price_per_million: f64,
    pub output_price_per_million: f64,
}

/// Information about a registered model.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub provider: ProviderType,
    pub input_price_per_million: f64,
    pub output_price_per_million: f64,
    pub context_window: u32,
    /// Whether this model was confirmed by a provider API refresh.
    /// Defaults are `false`; models discovered via API get `true`.
    /// The GUI dropdown only shows discovered models.
    pub discovered: bool,
}

/// Registry mapping model IDs to their provider and pricing metadata.
/// Thread-safe via internal RwLock for dynamic model discovery.
pub struct ModelRegistry {
    models: RwLock<HashMap<String, ModelInfo>>,
}

impl ModelRegistry {
    pub fn new(models: HashMap<String, ModelInfo>) -> Self {
        Self {
            models: RwLock::new(models),
        }
    }

    /// Create a registry with well-known models pre-populated.
    pub fn with_defaults() -> Self {
        let mut models = HashMap::new();

        // Anthropic models (discovered: false — only for internal routing/pricing)
        for id in &[
            "claude-opus-4-20250514",
            "claude-opus-4-6",
        ] {
            models.insert(id.to_string(), ModelInfo {
                provider: ProviderType::Anthropic,
                input_price_per_million: 15.0,
                output_price_per_million: 75.0,
                context_window: 200_000,
                discovered: false,
            });
        }
        for id in &[
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-20250514",
        ] {
            models.insert(id.to_string(), ModelInfo {
                provider: ProviderType::Anthropic,
                input_price_per_million: 3.0,
                output_price_per_million: 15.0,
                context_window: 200_000,
                discovered: false,
            });
        }
        models.insert("claude-haiku-4-5-20251001".to_string(), ModelInfo {
            provider: ProviderType::Anthropic,
            input_price_per_million: 1.0,
            output_price_per_million: 5.0,
            context_window: 200_000,
            discovered: false,
        });

        // OpenAI models
        models.insert("gpt-5.2".to_string(), ModelInfo {
            provider: ProviderType::OpenAI,
            input_price_per_million: 1.75,
            output_price_per_million: 14.0,
            context_window: 128_000,
            discovered: false,
        });
        models.insert("gpt-5-mini".to_string(), ModelInfo {
            provider: ProviderType::OpenAI,
            input_price_per_million: 0.25,
            output_price_per_million: 2.0,
            context_window: 128_000,
            discovered: false,
        });
        models.insert("gpt-5-nano".to_string(), ModelInfo {
            provider: ProviderType::OpenAI,
            input_price_per_million: 0.05,
            output_price_per_million: 0.40,
            context_window: 128_000,
            discovered: false,
        });

        Self {
            models: RwLock::new(models),
        }
    }

    /// Resolve which provider handles a given model ID.
    pub fn resolve_provider(&self, model_id: &str) -> Option<ProviderType> {
        self.models
            .read()
            .unwrap()
            .get(model_id)
            .map(|info| info.provider)
    }

    /// Resolve provider name as a string for a model ID.
    pub fn resolve_provider_name(&self, model_id: &str) -> Option<String> {
        self.resolve_provider(model_id).map(|p| p.to_string())
    }

    /// Get pricing info for a model.
    pub fn get_pricing(&self, model_id: &str) -> Option<PricingInfo> {
        self.models.read().unwrap().get(model_id).map(|info| PricingInfo {
            input_price_per_million: info.input_price_per_million,
            output_price_per_million: info.output_price_per_million,
        })
    }

    /// Get full model info (cloned).
    pub fn get_model_info(&self, model_id: &str) -> Option<ModelInfo> {
        self.models.read().unwrap().get(model_id).cloned()
    }

    /// Register or update a model entry.
    pub fn register(&self, model_id: String, info: ModelInfo) {
        self.models.write().unwrap().insert(model_id, info);
    }

    /// Register a model only if it's not already present.
    pub fn register_if_absent(&self, model_id: String, info: ModelInfo) {
        let mut models = self.models.write().unwrap();
        models.entry(model_id).or_insert(info);
    }

    /// Register a model from API discovery. If the model already exists
    /// (e.g. from defaults), mark it as discovered but preserve its
    /// pricing/context metadata. If new, insert with discovered=true.
    pub fn register_discovered(&self, model_id: String, info: ModelInfo) {
        let mut models = self.models.write().unwrap();
        match models.get_mut(&model_id) {
            Some(existing) => {
                existing.discovered = true;
            }
            None => {
                let mut new_info = info;
                new_info.discovered = true;
                models.insert(model_id, new_info);
            }
        }
    }

    /// Mark all default models for a provider as discovered.
    /// Fallback for providers with only managed keys.
    pub fn mark_defaults_discovered_for_provider(&self, provider: ProviderType) -> usize {
        let mut models = self.models.write().unwrap();
        let mut count = 0;
        for info in models.values_mut() {
            if info.provider == provider && !info.discovered {
                info.discovered = true;
                count += 1;
            }
        }
        count
    }

    /// List all registered model IDs.
    pub fn model_ids(&self) -> Vec<String> {
        self.models.read().unwrap().keys().cloned().collect()
    }

    /// List all models with full metadata, sorted by provider then name.
    pub fn list_models(&self) -> Vec<ModelEntry> {
        let models = self.models.read().unwrap();
        let mut entries: Vec<ModelEntry> = models
            .iter()
            .map(|(id, info)| ModelEntry {
                id: id.clone(),
                provider: info.provider.to_string(),
                context_window: info.context_window,
                input_price_per_million: info.input_price_per_million,
                output_price_per_million: info.output_price_per_million,
            })
            .collect();
        entries.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.id.cmp(&b.id)));
        entries
    }

    /// List only models confirmed by a provider API refresh.
    /// Returns an empty vec if no models have been discovered yet
    /// (the caller can decide whether to fall back to defaults).
    pub fn list_discovered_models(&self) -> Vec<ModelEntry> {
        let models = self.models.read().unwrap();
        let mut entries: Vec<ModelEntry> = models
            .iter()
            .filter(|(_, info)| info.discovered)
            .map(|(id, info)| ModelEntry {
                id: id.clone(),
                provider: info.provider.to_string(),
                context_window: info.context_window,
                input_price_per_million: info.input_price_per_million,
                output_price_per_million: info.output_price_per_million,
            })
            .collect();
        entries.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.id.cmp(&b.id)));
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_provider_anthropic() {
        let registry = ModelRegistry::with_defaults();
        assert_eq!(
            registry.resolve_provider("claude-sonnet-4-5-20250929"),
            Some(ProviderType::Anthropic)
        );
    }

    #[test]
    fn test_resolve_provider_openai() {
        let registry = ModelRegistry::with_defaults();
        assert_eq!(
            registry.resolve_provider("gpt-5.2"),
            Some(ProviderType::OpenAI)
        );
    }

    #[test]
    fn test_resolve_unknown() {
        let registry = ModelRegistry::with_defaults();
        assert_eq!(registry.resolve_provider("unknown-model"), None);
    }

    #[test]
    fn test_get_pricing() {
        let registry = ModelRegistry::with_defaults();
        let pricing = registry.get_pricing("claude-sonnet-4-5-20250929").unwrap();
        assert!((pricing.input_price_per_million - 3.0).abs() < 0.01);
        assert!((pricing.output_price_per_million - 15.0).abs() < 0.01);
    }

    #[test]
    fn test_custom_registry() {
        let mut models = HashMap::new();
        models.insert("my-model".to_string(), ModelInfo {
            provider: ProviderType::Ollama,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 4096,
            discovered: false,
        });
        let registry = ModelRegistry::new(models);
        assert_eq!(registry.resolve_provider("my-model"), Some(ProviderType::Ollama));
        assert_eq!(registry.resolve_provider("gpt-5.2"), None);
    }

    #[test]
    fn test_register_model() {
        let registry = ModelRegistry::new(HashMap::new());
        registry.register("new-model".to_string(), ModelInfo {
            provider: ProviderType::OpenAI,
            input_price_per_million: 1.0,
            output_price_per_million: 2.0,
            context_window: 32_000,
            discovered: false,
        });
        assert_eq!(registry.resolve_provider("new-model"), Some(ProviderType::OpenAI));
    }

    #[test]
    fn test_register_if_absent() {
        let registry = ModelRegistry::with_defaults();
        // Should not overwrite existing
        registry.register_if_absent("gpt-5.2".to_string(), ModelInfo {
            provider: ProviderType::Ollama,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 0,
            discovered: false,
        });
        assert_eq!(registry.resolve_provider("gpt-5.2"), Some(ProviderType::OpenAI));

        // Should add new
        registry.register_if_absent("new-model".to_string(), ModelInfo {
            provider: ProviderType::Ollama,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 4096,
            discovered: false,
        });
        assert_eq!(registry.resolve_provider("new-model"), Some(ProviderType::Ollama));
    }

    #[test]
    fn test_register_discovered() {
        let registry = ModelRegistry::with_defaults();

        // Existing default model should not be discovered
        let info = registry.get_model_info("gpt-5.2").unwrap();
        assert!(!info.discovered);

        // register_discovered marks existing model as discovered, preserves pricing
        registry.register_discovered("gpt-5.2".to_string(), ModelInfo {
            provider: ProviderType::OpenAI,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 0,
            discovered: true,
        });
        let info = registry.get_model_info("gpt-5.2").unwrap();
        assert!(info.discovered);
        // Pricing preserved from defaults
        assert!((info.input_price_per_million - 1.75).abs() < 0.01);

        // register_discovered inserts new model with discovered=true
        registry.register_discovered("new-api-model".to_string(), ModelInfo {
            provider: ProviderType::Anthropic,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 0,
            discovered: false, // even if passed false, method forces true
        });
        let info = registry.get_model_info("new-api-model").unwrap();
        assert!(info.discovered);
    }

    #[test]
    fn test_list_discovered_models() {
        let registry = ModelRegistry::with_defaults();

        // No models discovered yet → empty
        let discovered = registry.list_discovered_models();
        assert!(discovered.is_empty());

        // Mark one model as discovered
        registry.register_discovered("gpt-5.2".to_string(), ModelInfo {
            provider: ProviderType::OpenAI,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 0,
            discovered: true,
        });

        let discovered = registry.list_discovered_models();
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].id, "gpt-5.2");

        // list_models still returns all
        let all = registry.list_models();
        assert!(all.len() > 1);
    }

    #[test]
    fn test_list_models() {
        let registry = ModelRegistry::with_defaults();
        let models = registry.list_models();
        assert!(!models.is_empty());
        // Should be sorted by provider then id
        for w in models.windows(2) {
            assert!(w[0].provider <= w[1].provider
                || (w[0].provider == w[1].provider && w[0].id <= w[1].id));
        }
    }
}
