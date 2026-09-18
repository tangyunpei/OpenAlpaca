//! Model registry: maps model IDs to provider types and pricing info.

use crate::keys::key_pool::ProviderType;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
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
    /// Whether the model can be given tools. Every agent path needs them, so a
    /// picker has to be able to say which installed model cannot serve one.
    pub supports_tools: bool,
}

/// A model as a provider's own API describes it.
///
/// What a discovery pass hands the registry: the id to call it by, plus
/// whatever metadata the provider volunteered. Everything optional is
/// genuinely optional — a provider that only lists names produces
/// [`DiscoveredModel::bare`] rows.
#[derive(Debug, Clone)]
pub struct DiscoveredModel {
    pub id: String,
    /// The context length the provider reports, when it reports one.
    pub context_window: Option<u32>,
    /// The model accepts image content natively.
    pub supports_image: bool,
    /// The model can be given tools.
    pub supports_tools: bool,
}

impl DiscoveredModel {
    /// A model the provider named and said nothing else about.
    ///
    /// Tool support is assumed: every agent path needs tools, and a provider
    /// that does not describe its models has not said this one lacks them.
    /// Ollama, which does describe them, says so explicitly.
    pub fn bare(id: String) -> Self {
        Self {
            id,
            context_window: None,
            supports_image: false,
            supports_tools: true,
        }
    }
}

/// What the last discovery pass learned about one provider.
///
/// Kept so the daemon can say, on the provider's status, that Ollama was
/// unreachable rather than leaving an empty model list to be read as "none
/// installed" (L2).
#[derive(Debug, Clone, Serialize)]
pub struct ProviderDiscovery {
    /// Models the provider's API reported.
    pub models: usize,
    /// Why the provider's API was not read, when it was not.
    pub error: Option<String>,
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
    /// Whether this model can be given tools.
    ///
    /// Recorded, not enforced: nothing withholds tools from a call on the
    /// strength of this flag. It orders the effective-model ladder, which
    /// prefers a tool-capable model when it has to pick one for the owner (L3).
    pub supports_tools: bool,
    /// Whether this entry has a source other than API discovery — a compiled
    /// default or a `[models]` row the owner wrote.
    ///
    /// Only discovery's own rows are *removed* when a local provider stops
    /// reporting them; a declared row is un-discovered instead, so it leaves
    /// the picker without the owner's declaration being deleted behind their
    /// back (L2).
    pub declared: bool,
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
        Self {
            models: RwLock::new(Self::default_models()),
        }
    }

    /// The compiled default catalogue — every model this build knows how to
    /// price and route without asking a provider's API.
    fn default_models() -> HashMap<String, ModelInfo> {
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
                    supports_tools: true,
                    declared: true,
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
                    supports_tools: true,
                    declared: true,
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
                supports_tools: true,
                declared: true,
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
                supports_tools: true,
                declared: true,
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
                supports_tools: true,
                declared: true,
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
                supports_tools: true,
                declared: true,
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
                    supports_tools: true,
                    declared: true,
                },
            );
        }

        models
    }

    /// Create a registry with well-known models, overridden by config models.
    /// Config models take precedence over compiled defaults.
    ///
    /// `disabled` names the providers `llm.toml` says are off
    /// ([`crate::config::disabled_providers`]). **A disabled provider
    /// contributes nothing** — neither its `[models]` rows nor its compiled
    /// defaults (R58b). A catalogue entry the router has no provider for is a
    /// model the picker offers and the call cannot serve.
    pub fn with_defaults_and_config(
        config_models: &HashMap<String, crate::config::ModelConfigEntry>,
        disabled: &HashSet<ProviderType>,
    ) -> Self {
        let mut models = Self::default_models();
        models.retain(|_, info| !disabled.contains(&info.provider));
        let registry = Self::new(models);
        registry.reload_from_config(config_models, disabled);
        registry
    }

    /// Reload model registry entries from config (hot-reload).
    /// Config models override any existing entries.
    ///
    /// Rows belonging to a provider in `disabled` are skipped — see
    /// [`Self::with_defaults_and_config`]. The watcher runs this on every
    /// `llm.toml` change, so without the skip a hand edit (or a settings write
    /// the dedup ring missed) would undo a disable's half of the work.
    pub fn reload_from_config(
        &self,
        config_models: &HashMap<String, crate::config::ModelConfigEntry>,
        disabled: &HashSet<ProviderType>,
    ) {
        for (model_id, entry) in config_models {
            if let Some(provider) = crate::config::parse_provider_type_pub(&entry.provider) {
                if disabled.contains(&provider) {
                    continue;
                }
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
                        // The one flag that defaults on. The others withhold a
                        // content kind when omitted; this one only orders the
                        // ladder's preference, and a row the owner wrote for a
                        // model they mean to run agents on is tool-capable
                        // until they say otherwise.
                        supports_tools: entry.supports_tools.unwrap_or(true),
                        declared: true,
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

    /// Check if a model can be given tools.
    ///
    /// Unknown ids answer `true`: nothing is withheld on the strength of this
    /// flag, so the safe direction is "assume it works" rather than steering
    /// the owner away from a model the registry simply has not met.
    pub fn supports_tools(&self, model_id: &str) -> bool {
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .map(|info| info.supports_tools)
            .unwrap_or(true)
    }

    /// A model this provider can serve, preferring a confirmed tool-capable one.
    ///
    /// The last rung of the effective-model ladder (L3): when a provider's
    /// configured `default_model` names something that is not there — an
    /// Ollama tag that was never pulled, say — this is what the owner actually
    /// has. A model the provider's API confirmed wins over one only declared,
    /// then a tool-capable one over one that is not, then the id — so the
    /// answer does not move between runs.
    pub fn first_model_for_provider(&self, provider: &ProviderType) -> Option<String> {
        let models = self.models.read().unwrap_or_else(|p| p.into_inner());
        let mut candidates: Vec<(&String, &ModelInfo)> = models
            .iter()
            .filter(|(_, info)| &info.provider == provider)
            .collect();
        candidates.sort_by(|(a_id, a), (b_id, b)| {
            b.discovered
                .cmp(&a.discovered)
                .then(b.supports_tools.cmp(&a.supports_tools))
                .then(a_id.cmp(b_id))
        });
        candidates.first().map(|(id, _)| (*id).clone())
    }

    /// Withdraw the models a provider no longer reports, returning what went.
    ///
    /// Only meaningful where the provider's API is the ground truth for what
    /// exists — a local one (L2). A tag that is no longer installed cannot be
    /// served, and a catalogue entry the call cannot serve is exactly what
    /// R58b refuses to offer.
    ///
    /// Two outcomes, because two kinds of entry: a row **discovery created** is
    /// removed, since discovery is all it ever was; a row the owner
    /// **declared** in `[models]` (or a compiled default) is only
    /// un-discovered, so it leaves the picker while their declaration stays —
    /// deleting it would also undo the config rows the settings service
    /// re-applies on every provider enable. The caller logs the ids, so
    /// nothing disappears quietly.
    pub fn withdraw_absent_for_provider(
        &self,
        provider: &ProviderType,
        present: &HashSet<String>,
    ) -> Vec<String> {
        let mut models = self.models.write().unwrap_or_else(|p| p.into_inner());
        let mut withdrawn = Vec::new();
        let mut remove = Vec::new();
        for (id, info) in models.iter_mut() {
            if &info.provider != provider || present.contains(id) {
                continue;
            }
            if info.declared {
                if info.discovered {
                    info.discovered = false;
                    withdrawn.push(id.clone());
                }
            } else {
                remove.push(id.clone());
            }
        }
        for id in &remove {
            models.remove(id);
        }
        withdrawn.extend(remove);
        withdrawn.sort();
        withdrawn
    }

    /// Whether every model in the catalogue belongs to a provider that runs on
    /// this machine.
    ///
    /// The cost tracker's last resort (L8): on a local-only install no call can
    /// have cost money, so an id the registry has never met is priced at 0
    /// rather than at the conservative cloud rate. An empty catalogue answers
    /// `false` — it proves nothing.
    pub fn is_local_only(&self) -> bool {
        let models = self.models.read().unwrap_or_else(|p| p.into_inner());
        !models.is_empty() && models.values().all(|info| info.provider.is_local())
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

    /// Put back the compiled defaults for a provider that [`Self::remove_by_provider`]
    /// stripped, returning how many entries were restored.
    ///
    /// Re-enabling a provider (GAP-15) has to do this before refreshing:
    /// `refresh_models` only *marks* entries it can already see, so without a
    /// restore a disable/enable cycle would leave the provider with no
    /// catalogue until the daemon restarted. Existing entries win, so a
    /// discovered model is never overwritten by its default.
    pub fn restore_defaults_for_provider(&self, provider: &ProviderType) -> usize {
        let mut models = self.models.write().unwrap_or_else(|p| p.into_inner());
        let mut restored = 0;
        for (id, info) in Self::default_models() {
            if info.provider == *provider && !models.contains_key(&id) {
                models.insert(id, info);
                restored += 1;
            }
        }
        restored
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
                // A zero window is not a declaration, it is the absence of one
                // — and the lead agent reads it verbatim. Fill it in when the
                // provider has now told us the real length; everything a
                // `[models]` row does declare still wins (L2).
                if existing.context_window == 0 && info.context_window > 0 {
                    existing.context_window = info.context_window;
                }
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
                supports_tools: info.supports_tools,
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
                supports_tools: info.supports_tools,
            })
            .collect();
        entries.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.id.cmp(&b.id)));
        entries
    }
}

#[cfg(test)]
mod tests;
