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
                let count = self
                    .model_registry
                    .mark_defaults_discovered_for_provider(provider_type);
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
                    let dc = self
                        .model_registry
                        .mark_defaults_discovered_for_provider(provider_type);
                    tracing::info!(
                        "Provider {:?} returned 0 models, marked {} defaults",
                        provider_type,
                        dc
                    );
                }
                let present: std::collections::HashSet<String> =
                    models.iter().map(|m| m.id.clone()).collect();
                for model in &models {
                    self.model_registry.register_discovered(
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
                        },
                    );
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
                self.model_registry
                    .mark_defaults_discovered_for_provider(provider_type);
                ProviderDiscovery {
                    models: 0,
                    error: Some(e.to_string()),
                }
            }
        };

        self.record_discovery(provider_type, status)
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
