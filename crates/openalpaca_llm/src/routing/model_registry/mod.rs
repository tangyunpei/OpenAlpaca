//! Model registry: maps model IDs to provider types and pricing info.

use crate::keys::key_pool::ProviderType;
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
    /// Whether this model accepts image content parts natively.
    pub supports_image: bool,
    /// Whether this model accepts audio content parts natively.
    pub supports_audio: bool,
    /// Whether this model accepts document (PDF) content parts natively.
    pub supports_document: bool,
    /// Whether this model supports reasoning (OpenAI o-series).
    pub supports_reasoning: bool,
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
        for id in &["claude-opus-4-20250514", "claude-opus-4-6"] {
            models.insert(
                id.to_string(),
                ModelInfo {
                    provider: ProviderType::Anthropic,
                    input_price_per_million: 15.0,
                    output_price_per_million: 75.0,
                    context_window: 200_000,
                    discovered: false,
                    supports_image: true,
                    supports_audio: false,
                    supports_document: true,
                    supports_reasoning: false,
                },
            );
        }
        for id in &["claude-sonnet-4-6", "claude-sonnet-4-5-20250929", "claude-sonnet-4-20250514"] {
            models.insert(
                id.to_string(),
                ModelInfo {
                    provider: ProviderType::Anthropic,
                    input_price_per_million: 3.0,
                    output_price_per_million: 15.0,
                    context_window: 200_000,
                    discovered: false,
                    supports_image: true,
                    supports_audio: false,
                    supports_document: true,
                    supports_reasoning: false,
                },
            );
        }
        models.insert(
            "claude-haiku-4-5-20251001".to_string(),
            ModelInfo {
                provider: ProviderType::Anthropic,
                input_price_per_million: 1.0,
                output_price_per_million: 5.0,
                context_window: 200_000,
                discovered: false,
                supports_image: true,
                supports_audio: false,
                supports_document: true,
                supports_reasoning: false,
            },
        );

        // OpenAI models
        models.insert(
            "gpt-5.2".to_string(),
            ModelInfo {
                provider: ProviderType::OpenAI,
                input_price_per_million: 1.75,
                output_price_per_million: 14.0,
                context_window: 128_000,
                discovered: false,
                supports_image: true,
                supports_audio: true,
                supports_document: false,
                supports_reasoning: false,
            },
        );
        models.insert(
            "gpt-5-mini".to_string(),
            ModelInfo {
                provider: ProviderType::OpenAI,
                input_price_per_million: 0.25,
                output_price_per_million: 2.0,
                context_window: 128_000,
                discovered: false,
                supports_image: true,
                supports_audio: true,
                supports_document: false,
                supports_reasoning: false,
            },
        );
        models.insert(
            "gpt-5-nano".to_string(),
            ModelInfo {
                provider: ProviderType::OpenAI,
                input_price_per_million: 0.05,
                output_price_per_million: 0.40,
                context_window: 128_000,
                discovered: false,
                supports_image: true,
                supports_audio: true,
                supports_document: false,
                supports_reasoning: false,
            },
        );

        // OpenAI o-series reasoning models
        for id in &["o3", "o3-mini", "o1", "o1-mini"] {
            models.insert(
                id.to_string(),
                ModelInfo {
                    provider: ProviderType::OpenAI,
                    input_price_per_million: match *id {
                        "o3" => 10.0,
                        "o3-mini" => 1.10,
                        "o1" => 15.0,
                        "o1-mini" => 1.10,
                        _ => 0.0,
                    },
                    output_price_per_million: match *id {
                        "o3" => 40.0,
                        "o3-mini" => 4.40,
                        "o1" => 60.0,
                        "o1-mini" => 4.40,
                        _ => 0.0,
                    },
                    context_window: 200_000,
                    discovered: false,
                    supports_image: matches!(*id, "o1" | "o3"),
                    supports_audio: false,
                    supports_document: false,
                    supports_reasoning: true,
                },
            );
        }

        Self {
            models: RwLock::new(models),
        }
    }

    /// Create a registry with well-known models, overridden by config models.
    /// Config models take precedence over compiled defaults.
    pub fn with_defaults_and_config(
        config_models: &HashMap<String, crate::config::ModelConfigEntry>,
    ) -> Self {
        let registry = Self::with_defaults();
        for (model_id, entry) in config_models {
            if let Some(provider) = crate::config::parse_provider_type_pub(&entry.provider) {
                registry.register(
                    model_id.clone(),
                    ModelInfo {
                        provider,
                        input_price_per_million: entry.input_price.unwrap_or(0.0),
                        output_price_per_million: entry.output_price.unwrap_or(0.0),
                        context_window: entry.context.unwrap_or(200_000),
                        discovered: false,
                        supports_image: entry.supports_image.unwrap_or(false),
                        supports_audio: entry.supports_audio.unwrap_or(false),
                        supports_document: entry.supports_document.unwrap_or(false),
                        supports_reasoning: entry.supports_reasoning.unwrap_or(false),
                    },
                );
            }
        }
        registry
    }

    /// Reload model registry entries from config (hot-reload).
    /// Config models override any existing entries.
    pub fn reload_from_config(
        &self,
        config_models: &HashMap<String, crate::config::ModelConfigEntry>,
    ) {
        for (model_id, entry) in config_models {
            if let Some(provider) = crate::config::parse_provider_type_pub(&entry.provider) {
                self.register(
                    model_id.clone(),
                    ModelInfo {
                        provider,
                        input_price_per_million: entry.input_price.unwrap_or(0.0),
                        output_price_per_million: entry.output_price.unwrap_or(0.0),
                        context_window: entry.context.unwrap_or(200_000),
                        discovered: false,
                        supports_image: entry.supports_image.unwrap_or(false),
                        supports_audio: entry.supports_audio.unwrap_or(false),
                        supports_document: entry.supports_document.unwrap_or(false),
                        supports_reasoning: entry.supports_reasoning.unwrap_or(false),
                    },
                );
            }
        }
    }

    /// Resolve which provider handles a given model ID.
    pub fn resolve_provider(&self, model_id: &str) -> Option<ProviderType> {
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .map(|info| info.provider.clone())
    }

    /// Resolve provider name as a string for a model ID.
    pub fn resolve_provider_name(&self, model_id: &str) -> Option<String> {
        self.resolve_provider(model_id).map(|p| p.to_string())
    }

    /// Get pricing info for a model.
    pub fn get_pricing(&self, model_id: &str) -> Option<PricingInfo> {
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .map(|info| PricingInfo {
                input_price_per_million: info.input_price_per_million,
                output_price_per_million: info.output_price_per_million,
            })
    }

    /// Get full model info (cloned).
    pub fn get_model_info(&self, model_id: &str) -> Option<ModelInfo> {
        self.models.read().unwrap_or_else(|p| p.into_inner()).get(model_id).cloned()
    }

    /// Check if a model supports image input.
    pub fn supports_image(&self, model_id: &str) -> bool {
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .map(|info| info.supports_image)
            .unwrap_or(false)
    }

    /// Check if a model supports audio input.
    pub fn supports_audio(&self, model_id: &str) -> bool {
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .map(|info| info.supports_audio)
            .unwrap_or(false)
    }

    /// Check if a model supports document (PDF) input.
    pub fn supports_document(&self, model_id: &str) -> bool {
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .map(|info| info.supports_document)
            .unwrap_or(false)
    }

    /// Check if a model supports reasoning (OpenAI o-series).
    pub fn supports_reasoning(&self, model_id: &str) -> bool {
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .map(|info| info.supports_reasoning)
            .unwrap_or(false)
    }

    /// Register or update a model entry.
    pub fn register(&self, model_id: String, info: ModelInfo) {
        self.models.write().unwrap_or_else(|p| p.into_inner()).insert(model_id, info);
    }

    /// Remove a model from the registry. Returns true if it existed.
    pub fn remove(&self, model_id: &str) -> bool {
        self.models.write().unwrap_or_else(|p| p.into_inner()).remove(model_id).is_some()
    }

    /// Remove all models for a given provider type.
    /// Returns the list of model IDs that were removed.
    pub fn remove_by_provider(&self, provider_type: &ProviderType) -> Vec<String> {
        let mut models = self.models.write().unwrap_or_else(|p| p.into_inner());
        let to_remove: Vec<String> = models
            .iter()
            .filter(|(_, info)| &info.provider == provider_type)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &to_remove {
            models.remove(id);
        }
        to_remove
    }

    /// Register a model only if it's not already present.
    pub fn register_if_absent(&self, model_id: String, info: ModelInfo) {
        let mut models = self.models.write().unwrap_or_else(|p| p.into_inner());
        models.entry(model_id).or_insert(info);
    }

    /// Register a model from API discovery. If the model already exists
    /// (e.g. from defaults), mark it as discovered but preserve its
    /// pricing/context metadata. If new, insert with discovered=true.
    pub fn register_discovered(&self, model_id: String, info: ModelInfo) {
        let mut models = self.models.write().unwrap_or_else(|p| p.into_inner());
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
    pub fn mark_defaults_discovered_for_provider(&self, provider: &ProviderType) -> usize {
        let mut models = self.models.write().unwrap_or_else(|p| p.into_inner());
        let mut count = 0;
        for info in models.values_mut() {
            if info.provider == *provider && !info.discovered {
                info.discovered = true;
                count += 1;
            }
        }
        count
    }

    /// List all registered model IDs.
    pub fn model_ids(&self) -> Vec<String> {
        self.models.read().unwrap_or_else(|p| p.into_inner()).keys().cloned().collect()
    }

    /// List all models with full metadata, sorted by provider then name.
    pub fn list_models(&self) -> Vec<ModelEntry> {
        let models = self.models.read().unwrap_or_else(|p| p.into_inner());
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
        let models = self.models.read().unwrap_or_else(|p| p.into_inner());
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
mod tests;
