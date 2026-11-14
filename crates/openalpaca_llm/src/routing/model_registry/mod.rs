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
    /// The higher rate a long prompt pays, when the model has one.
    pub long_prompt: Option<LongPromptRate>,
}

/// A second rate a model's whole request pays once its prompt is longer than
/// a threshold (O-17 (a)).
///
/// The prompt's length is its input tokens, cached ones included — the
/// folded `Usage.input_tokens` — and the request is priced on its own:
/// above the threshold every input, cache and output token takes this rate,
/// the cache rates scaling with the input rate as they do at the standard
/// one. At the threshold itself the standard rate holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LongPromptRate {
    /// The longest prompt, in input tokens, still priced at the standard rate.
    pub above_input_tokens: u32,
    pub input_price_per_million: f64,
    pub output_price_per_million: f64,
}

/// What a model's API refuses in a request, as its provider's own pages say
/// (C7a, M-3d): each flag set only on a compiled row whose page, read on the
/// day the row was written, says so, and named there with the page and the
/// date. A row with none set gets the request its caller asked for.
///
/// The router meets the contract where a request first names its model
/// (`LlmRouter::meet_request_contract`): it leaves out what the model
/// refuses, sends `auto` for a forced tool choice and says so once a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RequestContract {
    /// `temperature`, `top_p` and `top_k` are refused (a 400 at a non-default
    /// value under the API version the Anthropic arm sends).
    pub no_sampling_parameters: bool,
    /// Thinking is always on: `thinking: {"type": "disabled"}` is refused.
    pub thinking_always_on: bool,
    /// Manual thinking budgets (`thinking: {"type": "enabled", ...}`) are
    /// refused; adaptive thinking is the model's only mode.
    pub adaptive_thinking_only: bool,
    /// A forced tool choice (`any`, or a named tool) is refused.
    pub no_forced_tool_choice: bool,
}

impl RequestContract {
    /// A model whose pages state no request rule the arm would break.
    pub const NONE: Self = Self {
        no_sampling_parameters: false,
        thinking_always_on: false,
        adaptive_thinking_only: false,
        no_forced_tool_choice: false,
    };
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
    /// Whether this model reports reasoning tokens (OpenAI's reasoning models).
    /// Recorded, not read by any call path.
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
    /// The rate a request pays once its prompt passes a length, for a model
    /// whose page prices long prompts higher; `None` for one priced the same
    /// at any length. Only a compiled row carries one: a `[models]` row the
    /// person writes replaces the compiled row whole, rate included.
    pub long_prompt: Option<LongPromptRate>,
    /// What the model's API refuses in a request (C7a). A compiled row's;
    /// a `[models]` row for the same id keeps it, since it is the model's
    /// contract rather than the person's price, and a discovered id that no
    /// compiled row names has none.
    pub request_contract: RequestContract,
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

        // Anthropic models (discovered: false — only for internal routing/pricing).
        // Prices from platform.claude.com/docs/en/about-claude/pricing, read
        // 2026-11-12. The windows of the rows before Opus 5.5 stay as listed
        // until step 19's live check (O-17 (a)); the pricing page's "Long
        // context pricing" gives Claude 4.6 and later (but Haiku 5.5) their
        // whole window at the standard rate, so none has a long-prompt rate.
        // `claude-opus-4-20250514` and `claude-sonnet-4-20250514` are gone:
        // "Retired" on June 15, 2026 (the model-deprecations page, read
        // 2026-11-12), O-17 (a).
        //
        // Claude Opus 5.5, the latest Opus and `strong`'s row (O-13 (a)):
        // $4 / $20, a 1M-token window (the pricing page and the models
        // overview, platform.claude.com/docs/en/models/overview, read
        // 2026-11-12); text and image in and tool use ("All current models
        // support text and image input, ... vision, and tool use", the
        // overview); PDFs ("All active models support PDF processing",
        // .../build-with-claude/pdf-support, read 2026-11-12).
        models.insert(
            "claude-opus-5-5".to_string(),
            ModelInfo {
                provider: ProviderType::Anthropic,
                input_price_per_million: 4.0,
                output_price_per_million: 20.0,
                context_window: 1_000_000,
                discovered: false,
                supports_image: true,
                supports_audio: false,
                supports_document: true,
                supports_reasoning: false,
                supports_tools: true,
                declared: true,
                long_prompt: None,
                // C7a: its request contract, from the pages read 2026-11-12.
                // The Opus 5.5 migration guide (platform.claude.com/docs/en/
                // models/opus-5-5/migration-guide), "What every request to
                // Claude Opus 5.5 must satisfy": "Omit `temperature`, `top_p`,
                // and `top_k`"; `thinking: {"type": "disabled"}` and manual
                // budgets (`"enabled"`) "are rejected" — "adaptive thinking is
                // always on"; forcing a tool call with `any` or a named tool
                // "is rejected". The thinking troubleshooting page's table
                // (.../build-with-claude/thinking-troubleshooting): Opus 5.5
                // "Adaptive only", "Always on", rejecting `"enabled"` and
                // `"disabled"`. Model deprecations (.../about-claude/
                // model-deprecations): sampling parameters "Removed in API
                // version `2026-10-21`"; "Earlier API versions return it [a
                // 400] when set to a non-default value on Claude 4.7 and
                // later models" — the arm sends `2023-06-01`.
                request_contract: RequestContract {
                    no_sampling_parameters: true,
                    thinking_always_on: true,
                    adaptive_thinking_only: true,
                    no_forced_tool_choice: true,
                },
            },
        );
        // Claude Opus 4.6: $5 / $25 (the pricing page, read 2026-11-12; the
        // catalogue had $15 / $75, Opus 4's and 4.1's rate).
        models.insert(
            "claude-opus-4-6".to_string(),
            ModelInfo {
                provider: ProviderType::Anthropic,
                input_price_per_million: 5.0,
                output_price_per_million: 25.0,
                context_window: 200_000,
                discovered: false,
                supports_image: true,
                supports_audio: false,
                supports_document: true,
                supports_reasoning: false,
                supports_tools: true,
                declared: true,
                long_prompt: None,
                request_contract: RequestContract::NONE,
            },
        );
        // Claude Sonnet 4.6 and 4.5: $3 / $15 (the pricing page, read
        // 2026-11-12). Sonnet 4.5 is "Deprecated", retiring November 30,
        // 2026 (the model-deprecations page): kept until then.
        for id in &["claude-sonnet-4-6", "claude-sonnet-4-5-20250929"] {
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
                    long_prompt: None,
                    request_contract: RequestContract::NONE,
                },
            );
        }
        // Claude Haiku 4.5: $1 / $5 (the pricing page, read 2026-11-12).
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
                long_prompt: None,
                request_contract: RequestContract::NONE,
            },
        );

        // OpenAI models. `gpt-5.2` $1.75 / $14, `gpt-5-mini` $0.25 / $2 and
        // `gpt-5-nano` $0.05 / $0.40, as the catalogue had them
        // (developers.openai.com/api/docs/pricing and each model's page, read
        // 2026-11-12); the last two "Deprecated", shut down on Dec 11, 2026
        // (the deprecations page): kept until O-16 (a) moves the aliases off
        // them. The windows stay as listed until step 19's live check.
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
                long_prompt: None,
                request_contract: RequestContract::NONE,
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
                long_prompt: None,
                request_contract: RequestContract::NONE,
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
                long_prompt: None,
                request_contract: RequestContract::NONE,
            },
        );

        // The seeded `llm.toml`'s OpenAI default (O-18 (c)), at the rates and
        // window of OpenAI's own model page (developers.openai.com, "GPT-4o
        // mini", read 2026-11-12): text and image in, no audio, function
        // calling; the page marks it deprecated.
        models.insert(
            "gpt-4o-mini".to_string(),
            ModelInfo {
                provider: ProviderType::OpenAI,
                input_price_per_million: 0.15,
                output_price_per_million: 0.60,
                context_window: 128_000,
                discovered: false,
                supports_image: true,
                supports_audio: false,
                supports_document: false,
                supports_reasoning: false,
                supports_tools: true,
                declared: true,
                long_prompt: None,
                request_contract: RequestContract::NONE,
            },
        );

        // GPT-6.1 Sol, the latest Sol (O-13 (a); `strong`'s OpenAI row once
        // C7b lands, O-15 (a)): $2 / $10, and "API prompts with more than
        // 272K input tokens are priced at 2x input and cache rates and 1.5x
        // output for the full request" — $4 / $15; a 1,050,000-token window;
        // text and image in, audio unsupported; reasoning tokens; function
        // calling, though "Tool calling requires the Responses API", which
        // the arm reaches only with C7b (developers.openai.com/api/docs/
        // models/gpt-6.1-sol and .../pricing, read 2026-11-12).
        models.insert(
            "gpt-6.1-sol".to_string(),
            ModelInfo {
                provider: ProviderType::OpenAI,
                input_price_per_million: 2.0,
                output_price_per_million: 10.0,
                context_window: 1_050_000,
                discovered: false,
                supports_image: true,
                supports_audio: false,
                supports_document: false,
                supports_reasoning: true,
                supports_tools: true,
                declared: true,
                long_prompt: Some(LongPromptRate {
                    above_input_tokens: 272_000,
                    input_price_per_million: 4.0,
                    output_price_per_million: 15.0,
                }),
                request_contract: RequestContract::NONE,
            },
        );

        // o3: $2 / $8 (its page, developers.openai.com/api/docs/models/o3,
        // and the pricing page, read 2026-11-12; the catalogue had $10 /
        // $40), a 200,000-token window. "Deprecated", shut down on Dec 11,
        // 2026 (the deprecations page): kept until then. `o3-mini`, `o1` and
        // `o1-mini` are gone: "Shut down" (.../models/all and the
        // deprecations page, read 2026-11-12), O-17 (a).
        models.insert(
            "o3".to_string(),
            ModelInfo {
                provider: ProviderType::OpenAI,
                input_price_per_million: 2.0,
                output_price_per_million: 8.0,
                context_window: 200_000,
                discovered: false,
                supports_image: true,
                supports_audio: false,
                supports_document: false,
                supports_reasoning: true,
                supports_tools: true,
                declared: true,
                long_prompt: None,
                request_contract: RequestContract::NONE,
            },
        );

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
        // A row the person writes prices a model; its request contract is
        // the model's own, so the compiled row's is kept (C7a).
        let compiled = Self::default_models();
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
                        long_prompt: None,
                        request_contract: compiled
                            .get(model_id)
                            .map_or(RequestContract::NONE, |row| row.request_contract),
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

    /// The price the registry knows for a model.
    ///
    /// `None` for an id it has never met, and for one only a keyed provider's
    /// discovery registered ([`Self::is_unpriced`]): that row's 0 / 0 is the
    /// absence of a price, not a price (O-18).
    pub fn get_pricing(&self, model_id: &str) -> Option<PricingInfo> {
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .filter(|info| !Self::unpriced(info))
            .map(|info| PricingInfo {
                input_price_per_million: info.input_price_per_million,
                output_price_per_million: info.output_price_per_million,
                long_prompt: info.long_prompt,
            })
    }

    /// Whether this id is registered with no price anyone stated: a keyed
    /// provider's discovery listed it, and neither a compiled row nor a
    /// `[models]` row prices it.
    ///
    /// Such a row stays routable; the cost tracker counts it at its fallback
    /// for an unknown id (O-18 (b)). A local provider's discovered row is
    /// priced at 0 instead, because it runs on this machine (L8) — and
    /// "local" is the set whose provider needs no key: only Ollama is either.
    pub fn is_unpriced(&self, model_id: &str) -> bool {
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .is_some_and(Self::unpriced)
    }

    fn unpriced(info: &ModelInfo) -> bool {
        !info.declared && !info.provider.is_local()
    }

    /// What `model_id`'s API refuses in a request (C7a); none for an id the
    /// registry has never met or whose row states nothing.
    pub fn request_contract(&self, model_id: &str) -> RequestContract {
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .map_or(RequestContract::NONE, |info| info.request_contract)
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

#[cfg(test)]
mod catalogue_tests;
