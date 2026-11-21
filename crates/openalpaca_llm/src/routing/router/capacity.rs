use super::*;
use crate::error::LlmError;
use crate::routing::model_registry::{ModelEntry, ModelInfo, ProviderDiscovery};
use crate::keys::key_pool::ProviderType;

/// What a local model's context window is assumed to be when its provider
/// cannot say. Small on purpose: over-stating the window overruns the model,
/// while under-stating it only compacts sooner than necessary (L2).
const DEFAULT_LOCAL_CONTEXT_WINDOW: u32 = 8192;

impl LlmRouter {
    /// List models confirmed by provider API refresh (for GUI dropdowns).
    /// Returns only discovered models so the dropdown reflects real availability.
    pub fn available_models(&self) -> Vec<ModelEntry> {
        self.model_registry.list_discovered_models()
    }

    /// Refresh models by querying each loaded provider's API.
    pub async fn refresh_models(&self) {
        // Snapshot the keys first: the pass awaits each provider's API over the
        // network, and an iterator guard held across that would block a
        // concurrent `deregister_provider` for the whole refresh (R59). A
        // provider unloaded mid-refresh is simply skipped by the lookup inside.
        let provider_types: Vec<ProviderType> =
            self.providers.iter().map(|e| e.key().clone()).collect();
        for provider_type in provider_types {
            self.refresh_models_for(&provider_type).await;
        }
    }

    /// Ask one provider what it can serve and record the answer.
    ///
    /// Discovered models are added to the registry, existing entries keeping
    /// their own metadata (a `[models]` row overrides discovery, and is never
    /// required — L2). Two things a keyed refresh could not do:
    ///
    /// * A provider that needs no key is no longer gated on its key pool. The
    ///   old pass acquired a key first and, failing, only marked *compiled*
    ///   defaults discovered — of which Ollama has none — so a working local
    ///   install showed an empty model list.
    /// * A local provider's API is the ground truth for what exists, so a tag
    ///   it no longer reports is withdrawn from the catalogue instead of being
    ///   offered to a picker that cannot serve it.
    ///
    /// A provider that cannot be reached stays registered with the catalogue it
    /// already has, one WARN, and an error on its
    /// [discovery status](Self::discovery_status) — never a boot failure.
    pub async fn refresh_models_for(&self, provider_type: &ProviderType) -> ProviderDiscovery {
        let Some(prov_entry) = self.provider_entry(provider_type) else {
            return ProviderDiscovery {
                models: 0,
                error: Some("provider is not loaded".to_string()),
            };
        };

        let pool = prov_entry.key_pool.load();
        let key_secret = match pool.acquire_api_compatible().await {
            Ok(guard) => guard.secret.clone(),
            Err(_) if !prov_entry.provider.requires_key() => String::new(),
            Err(_) => {
                // No API-compatible key — fall back to hardcoded defaults
                let count = self.mark_unlisted_served(provider_type, &prov_entry);
                if count > 0 {
                    tracing::info!(
                        "No API key for {:?}, marked {} default models as discovered",
                        provider_type,
                        count
                    );
                }
                return self.record_discovery(
                    provider_type,
                    ProviderDiscovery {
                        models: 0,
                        error: Some("no API-compatible key configured".to_string()),
                    },
                );
            }
        };

        let status = match prov_entry.provider.discover_models(&key_secret).await {
            Ok(models) => {
                if models.is_empty() {
                    let dc = self.mark_unlisted_served(provider_type, &prov_entry);
                    tracing::info!(
                        "Provider {:?} returned 0 models, marked {} defaults",
                        provider_type,
                        dc
                    );
                }
                let present: std::collections::HashSet<String> =
                    models.iter().map(|m| m.id.clone()).collect();
                for model in &models {
                    // A model the provider only named — Ollama's failed
                    // `/api/show` (BARE-NOT-READING), or a keyed provider's
                    // listing of ids, which reads no capability or window
                    // (pending-decisions *Rev 118*, taken as (a)) — is no
                    // reading: it is listed, but the registry keeps what a
                    // reading gave, so Ollama's reading of an id a
                    // compatible server also lists stands in either order.
                    use crate::routing::model_registry::ModelRegistry;
                    let register = if model.described {
                        ModelRegistry::register_discovered
                    } else {
                        ModelRegistry::register_listed
                    };
                    register(
                        &self.model_registry,
                        model.id.clone(),
                        ModelInfo {
                            provider: provider_type.clone(),
                            input_price_per_million: 0.0,
                            output_price_per_million: 0.0,
                            context_window: model.context_window.unwrap_or(
                                if provider_type.is_local() {
                                    DEFAULT_LOCAL_CONTEXT_WINDOW
                                } else {
                                    0
                                },
                            ),
                            discovered: true,
                            supports_image: model.supports_image,
                            supports_audio: false,
                            supports_document: false,
                            supports_reasoning: false,
                            supports_tools: model.supports_tools,
                            declared: false,
                            long_prompt: None,
                            cache_rates: None,
                            request_contract: crate::routing::model_registry::RequestContract::NONE,
                        },
                    );
                }
                self.note_unknown_windows(provider_type, &models);
                let retired: std::collections::BTreeSet<&str> = models
                    .iter()
                    .map(|m| m.id.as_str())
                    .filter(|id| crate::routing::model_registry::RETIRED_IDS.contains(id))
                    .collect();
                for id in &retired {
                    warn_retired(id, provider_type);
                }
                self.retired_listed.insert(
                    provider_type.clone(),
                    retired.into_iter().map(str::to_string).collect(),
                );
                // The list was read across an await: a `deregister_provider`
                // that ran meanwhile cleared a record this insert has just
                // put back, and nothing else would clear it. Insert, then
                // look: `deregister_provider` drops the provider before its
                // record, so whichever order the two take, a provider gone
                // keeps none (the registry's own rows are R59's).
                if !self.has_provider(provider_type) {
                    self.retired_listed.remove(provider_type);
                }
                if provider_type.is_local() {
                    let withdrawn = self
                        .model_registry
                        .withdraw_absent_for_provider(provider_type, &present);
                    if !withdrawn.is_empty() {
                        tracing::info!(
                            provider = %provider_type,
                            withdrawn = ?withdrawn,
                            "Models are no longer installed — withdrawn from the catalogue"
                        );
                    }
                }
                if !models.is_empty() {
                    tracing::info!("Refreshed {} models from {:?}", models.len(), provider_type);
                }
                ProviderDiscovery {
                    models: models.len(),
                    error: None,
                }
            }
            Err(e) => {
                tracing::warn!(
                    provider = %provider_type,
                    error = %e,
                    "Could not list models — the provider stays registered with the catalogue it has"
                );
                self.mark_unlisted_served(provider_type, &prov_entry);
                ProviderDiscovery {
                    models: 0,
                    error: Some(e.to_string()),
                }
            }
        };

        self.record_discovery(provider_type, status)
    }

    /// Say once a model that a provider built to call another server than
    /// its vendor's — the `openai` provider pointed at llama.cpp, LM
    /// Studio, vLLM or a gateway, on this machine, the home network or
    /// elsewhere — listed with no context window and no row gives it one
    /// (MODEL-ROWS, *Rev 96* (5); MODEL-ROWS-2, *Rev 102* (5) (b): every
    /// server that is not OpenAI's own, where the scope had been the local
    /// ones).
    ///
    /// The window stays unknown, so a run on the model is budgeted at the
    /// agent loop's 200,000-token default rather than at
    /// [`DEFAULT_LOCAL_CONTEXT_WINDOW`]: the stock lead's system prompt and
    /// tools alone come to more than 8,192 tokens, and a loop sized there
    /// compacts on every round (the advisor's pick, built as its
    /// conditional's fallback, option (a) with this warning). OpenAI's own
    /// API (`LlmProvider::serves_compiled_rows`) is not said: its models
    /// are the compiled rows' vendor's, not a server's of unknown size.
    /// Ollama gives each model a window of its own.
    fn note_unknown_windows(
        &self,
        provider_type: &ProviderType,
        listed: &[crate::routing::model_registry::DiscoveredModel],
    ) {
        if provider_type.is_local() || self.serves_compiled_rows(provider_type) {
            return;
        }
        for model in listed {
            let unknown = model.context_window.is_none()
                && self
                    .model_registry
                    .get_model_info(&model.id)
                    .is_some_and(|info| info.context_window == 0);
            if unknown && self.unknown_windows_logged.insert(model.id.clone()) {
                tracing::warn!(
                    model = %model.id,
                    provider = %provider_type,
                    "A model on a server that is not OpenAI's own reports no context \
                     window, so a run on it is budgeted at 200,000 tokens and may overrun \
                     the window the server gives it. Give it a [models.\"<id>\"] row \
                     with provider = \"{provider_type}\" and context = <its window, in tokens> \
                     in llm.toml."
                );
            }
        }
    }

    /// Say once more, for a hot reload of `llm.toml`, each id a loaded
    /// provider's last read listing named that the catalogue retired
    /// ([`RETIRED_IDS`]), and return them, by id (RETIRED-ID,
    /// pending-decisions *Rev 125*: (a) with (c)). A discovery pass says
    /// its own listing's (`refresh_models_for`, which keeps them; a list
    /// that could not be read keeps the last one's); a reload of a
    /// provider already loaded runs none, so the daemon's watcher calls
    /// this. Only listed ids are said: an id `llm.toml` itself names, in
    /// `[orchestrator]` or a `[models]` row, is not (*Rev 105*, closed as
    /// no change).
    ///
    /// [`RETIRED_IDS`]: crate::routing::model_registry::RETIRED_IDS
    pub fn warn_retired_listed(&self) -> Vec<String> {
        let mut listed: Vec<(String, ProviderType)> = self
            .retired_listed
            .iter()
            .flat_map(|entry| {
                let provider = entry.key().clone();
                entry
                    .value()
                    .iter()
                    .map(move |id| (id.clone(), provider.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        listed.sort_by_key(|(id, provider)| (id.clone(), provider.to_string()));
        for (id, provider) in &listed {
            warn_retired(id, provider);
        }
        listed.into_iter().map(|(id, _)| id).collect()
    }

    /// Where a provider's list could not be read, or was empty, take its
    /// catalogue as served: every row, for a provider that calls its
    /// vendor's own API; for one built to call another server
    /// (COMPAT-SERVER), its declared rows but the compiled ones, which that
    /// server does not serve.
    fn mark_unlisted_served(&self, provider_type: &ProviderType, entry: &ProviderEntry) -> usize {
        if entry.provider.serves_compiled_rows() {
            self.model_registry
                .mark_defaults_discovered_for_provider(provider_type)
        } else {
            self.model_registry
                .mark_declared_discovered_for_provider(provider_type)
        }
    }

    fn record_discovery(
        &self,
        provider_type: &ProviderType,
        status: ProviderDiscovery,
    ) -> ProviderDiscovery {
        self.discovery
            .insert(provider_type.clone(), status.clone());
        status
    }

    /// What the last discovery pass learned about this provider, if it ran.
    pub fn discovery_status(&self, provider_type: &ProviderType) -> Option<ProviderDiscovery> {
        self.discovery.get(provider_type).map(|e| e.value().clone())
    }

    /// List models available from a specific provider using the given key.
    /// Used during key validation to show what models the key can access.
    pub async fn list_models_for_provider(
        &self,
        provider_type: ProviderType,
        key: &str,
    ) -> Result<Vec<String>, LlmError> {
        let entry = self
            .providers
            .get(&provider_type)
            .ok_or(LlmError::NotConfigured)?;
        entry.value().provider.list_models_with_key(key).await
    }
}

/// The one line that says a provider lists `id`, which the catalogue
/// retired: not an error — the id is offered as any id the catalogue
/// lacks, with none of a built-in row's price, window or request rules
/// (an Ollama listing still reads its window and flags, a local server's
/// still costs 0); a person's own `[models."<id>"]` row can give it a
/// price and a window.
fn warn_retired(id: &str, provider: &ProviderType) {
    tracing::warn!(
        model = %id,
        provider = %provider,
        "The provider lists a model retired in the catalogue: it stays \
         offered as any id the catalogue lacks, with none of a built-in \
         row's price, window or request rules; a [models.\"<id>\"] row in \
         llm.toml can give it a price and a window."
    );
}

/// Rough estimate of tokens in a request.
///
/// Uses 1 token ≈ 4 bytes heuristic. Intentionally overestimates slightly,
/// which is the safe direction for rate limiting (better to be conservative
/// than to exceed TPM limits).
pub(super) fn estimate_request_tokens(request: &RouterRequest) -> u32 {
    let msg_tokens: u32 = request
        .messages
        .iter()
        .map(|m| {
            if let Some(ref parts) = m.parts {
                parts.iter().map(estimate_content_part_tokens).sum::<u32>()
            } else {
                (m.content.len() / 4) as u32
            }
        })
        .sum();
    let tool_tokens = request.tools_token_estimate.unwrap_or_else(|| {
        let tool_bytes: usize = request
            .tools
            .iter()
            .map(|t| {
                let base = t.description.len() + t.parameters.to_string().len();
                let examples = t.input_examples.as_ref().map_or(0, |ex| {
                    ex.iter().map(|e| e.to_string().len()).sum()
                });
                base + examples
            })
            .sum();
        (tool_bytes / 4) as u32
    });
    (msg_tokens + tool_tokens).max(100)
}

/// Estimate tokens for a single content part (mirrors agentic_loop logic).
fn estimate_content_part_tokens(part: &crate::ContentPart) -> u32 {
    match part {
        crate::ContentPart::Text { text } => (text.len() / 4) as u32,
        crate::ContentPart::Image { detail, .. } => match detail.as_deref() {
            Some("low") => 85,
            _ => 1590,
        },
        crate::ContentPart::Audio { data, .. } => {
            ((data.len() as f64 / 4096.0) * 25.0).ceil().max(25.0) as u32
        }
        crate::ContentPart::Document { extracted_text, .. } => extracted_text
            .as_ref()
            .map_or(500, |t| (t.len() / 4) as u32),
        crate::ContentPart::FileRef { .. } => 50,
    }
}
