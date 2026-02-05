//! Model registry: maps model IDs to provider types and pricing info.

use crate::key_pool::ProviderType;
use std::collections::HashMap;

/// Pricing information for a model.
#[derive(Debug, Clone)]
pub struct PricingInfo {
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
}

/// Registry mapping model IDs to their provider and pricing metadata.
pub struct ModelRegistry {
    models: HashMap<String, ModelInfo>,
}

impl ModelRegistry {
    pub fn new(models: HashMap<String, ModelInfo>) -> Self {
        Self { models }
    }

    /// Create a registry with well-known models pre-populated.
    pub fn with_defaults() -> Self {
        let mut models = HashMap::new();

        // Anthropic models
        for id in &[
            "claude-opus-4-20250514",
            "claude-opus-4-6",
        ] {
            models.insert(id.to_string(), ModelInfo {
                provider: ProviderType::Anthropic,
                input_price_per_million: 15.0,
                output_price_per_million: 75.0,
                context_window: 200_000,
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
            });
        }
        models.insert("claude-haiku-4-5-20251001".to_string(), ModelInfo {
            provider: ProviderType::Anthropic,
            input_price_per_million: 0.80,
            output_price_per_million: 4.0,
            context_window: 200_000,
        });

        // OpenAI models
        models.insert("gpt-4o".to_string(), ModelInfo {
            provider: ProviderType::OpenAI,
            input_price_per_million: 2.50,
            output_price_per_million: 10.0,
            context_window: 128_000,
        });
        models.insert("gpt-4o-mini".to_string(), ModelInfo {
            provider: ProviderType::OpenAI,
            input_price_per_million: 0.15,
            output_price_per_million: 0.60,
            context_window: 128_000,
        });

        Self { models }
    }

    /// Resolve which provider handles a given model ID.
    pub fn resolve_provider(&self, model_id: &str) -> Option<ProviderType> {
        self.models.get(model_id).map(|info| info.provider)
    }

    /// Get pricing info for a model.
    pub fn get_pricing(&self, model_id: &str) -> Option<PricingInfo> {
        self.models.get(model_id).map(|info| PricingInfo {
            input_price_per_million: info.input_price_per_million,
            output_price_per_million: info.output_price_per_million,
        })
    }

    /// Get full model info.
    pub fn get_model_info(&self, model_id: &str) -> Option<&ModelInfo> {
        self.models.get(model_id)
    }

    /// Register or update a model entry.
    pub fn register(&mut self, model_id: String, info: ModelInfo) {
        self.models.insert(model_id, info);
    }

    /// List all registered model IDs.
    pub fn model_ids(&self) -> Vec<&str> {
        self.models.keys().map(|s| s.as_str()).collect()
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
            registry.resolve_provider("gpt-4o"),
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
        });
        let registry = ModelRegistry::new(models);
        assert_eq!(registry.resolve_provider("my-model"), Some(ProviderType::Ollama));
        assert_eq!(registry.resolve_provider("gpt-4o"), None);
    }

    #[test]
    fn test_register_model() {
        let mut registry = ModelRegistry::new(HashMap::new());
        registry.register("new-model".to_string(), ModelInfo {
            provider: ProviderType::OpenAI,
            input_price_per_million: 1.0,
            output_price_per_million: 2.0,
            context_window: 32_000,
        });
        assert_eq!(registry.resolve_provider("new-model"), Some(ProviderType::OpenAI));
    }
}
