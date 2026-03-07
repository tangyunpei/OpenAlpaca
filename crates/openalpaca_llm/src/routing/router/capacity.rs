use super::*;
use crate::error::LlmError;
use crate::routing::model_registry::{ModelEntry, ModelInfo};
use crate::keys::key_pool::ProviderType;

impl LlmRouter {
    /// Estimate the parallel LLM capacity given the current state of API keys
    /// and rate limiters.
    ///
    /// Returns a [`LlmCapacityInfo`] struct so callers can base stagger delay
    /// on the raw key count and reserve slots for the lead agent.
    ///
    /// CLI fallback is **not** counted as parallel bandwidth — it is only used
    /// when all API keys are exhausted (see `try_fallback()`). When
    /// `key_capacity == 0` and a CLI backend exists, `effective_capacity` is 1
    /// so at least one subagent can proceed via fallback.
    ///
    /// Used by `SpawnSubagentTool` to dynamically reduce parallelism when
    /// the number of available API keys cannot support `max_concurrent_subagents`.
    pub async fn estimated_llm_capacity(&self, model: Option<&str>) -> LlmCapacityInfo {
        let zero = LlmCapacityInfo {
            available_api_keys: 0,
            per_key_concurrency: 0,
            key_capacity: 0,
            has_cli_fallback: false,
            effective_capacity: 0,
        };

        let default = self.default_model();
        let model_id = model.unwrap_or(&default);

        let provider_type = match self.model_registry.resolve_provider(model_id) {
            Some(pt) => pt,
            None => return zero,
        };

        let available_keys = match self.providers.get(&provider_type) {
            Some(entry) => {
                let pool = entry.value().key_pool.load();
                pool.available_api_key_count().await
            }
            None => return zero,
        };

        let per_key = self.rate_limiter_registry.config().per_key_concurrency;
        let key_capacity = available_keys * per_key;
        let has_cli_fallback = self.cli_backends.contains_key(&provider_type);

        let effective_capacity = if key_capacity > 0 {
            let global_available = self.concurrency_limiter.available_permits();
            key_capacity.min(global_available)
        } else if has_cli_fallback {
            // All keys exhausted but CLI fallback can handle 1 request
            1
        } else {
            0
        };

        LlmCapacityInfo {
            available_api_keys: available_keys,
            per_key_concurrency: per_key,
            key_capacity,
            has_cli_fallback,
            effective_capacity,
        }
    }

    /// List models confirmed by provider API refresh (for GUI dropdowns).
    /// Returns only discovered models so the dropdown reflects real availability.
    pub fn available_models(&self) -> Vec<ModelEntry> {
        self.model_registry.list_discovered_models()
    }

    /// Refresh models by querying each configured provider's API.
    /// Discovered models are added to the registry (existing entries preserved).
    /// Falls back to hardcoded defaults when no API-compatible key is available
    /// or when the provider returns 0 models (e.g. managed/OAuth keys only).
    pub async fn refresh_models(&self) {
        for entry in self.providers.iter() {
            let provider_type = *entry.key();
            let prov_entry = entry.value();
            let pool = prov_entry.key_pool.load();

            let key_secret = match pool.acquire_api_compatible().await {
                Ok(guard) => guard.secret.clone(),
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
                    continue;
                }
            };

            match prov_entry.provider.list_models_with_key(&key_secret).await {
                Ok(model_ids) => {
                    let count = model_ids.len();
                    if count == 0 {
                        let dc = self
                            .model_registry
                            .mark_defaults_discovered_for_provider(provider_type);
                        tracing::info!(
                            "Provider {:?} returned 0 models, marked {} defaults",
                            provider_type,
                            dc
                        );
                        continue;
                    }
                    for model_id in model_ids {
                        self.model_registry.register_discovered(
                            model_id,
                            ModelInfo {
                                provider: provider_type,
                                input_price_per_million: 0.0,
                                output_price_per_million: 0.0,
                                context_window: 0,
                                discovered: true,
                                supports_image: false,
                                supports_audio: false,
                                supports_document: false,
                                supports_reasoning: false,
                            },
                        );
                    }
                    tracing::info!("Refreshed {} models from {:?}", count, provider_type);
                }
                Err(e) => {
                    tracing::warn!("Failed to list models from {:?}: {}", provider_type, e);
                    self.model_registry
                        .mark_defaults_discovered_for_provider(provider_type);
                }
            }
        }
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
