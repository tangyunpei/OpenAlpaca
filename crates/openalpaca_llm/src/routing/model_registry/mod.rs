//! Model registry: maps model IDs to provider types and pricing info.

use crate::keys::key_pool::ProviderType;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, RwLock};

/// The ids of this build's compiled catalogue ([`ModelRegistry::default_models`]).
static COMPILED_IDS: LazyLock<HashSet<String>> =
    LazyLock::new(|| ModelRegistry::default_models().into_keys().collect());

/// Pricing information for a model.
#[derive(Debug, Clone)]
pub struct PricingInfo {
    pub input_price_per_million: f64,
    pub output_price_per_million: f64,
    /// The higher rate a long prompt pays, when the model has one.
    pub long_prompt: Option<LongPromptRate>,
    /// What a cache read and a cache write cost, when the row states it;
    /// `None` is [`CacheRates::UNSTATED`]'s figures.
    pub cache_rates: Option<CacheRates>,
    /// The prices no row states (a `[models]` row left them out and no
    /// compiled row names the id) on a provider counted as paid: the cost
    /// tracker counts each at its fallback's figure for that half, as it
    /// does an unpriced discovered row (MODEL-ROWS, *Rev 97* (b)). The
    /// figure above is then the row's placeholder 0, not a price.
    pub unpriced: PriceGaps,
}

/// Which of a row's two prices no one stated (MODEL-ROWS, *Rev 97* (b)):
/// a `[models."<id>"]` row that leaves a price out states none, and the
/// compiled row's figure stands in where the id has one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PriceGaps {
    pub input: bool,
    pub output: bool,
}

impl PriceGaps {
    /// Both prices missing: the row is counted as an unpriced discovered
    /// row is.
    pub const BOTH: Self = Self {
        input: true,
        output: true,
    };

    /// Whether either price is missing.
    pub fn any(self) -> bool {
        self.input || self.output
    }
}

/// What the registry keeps of a `[models."<id>"]` row the person wrote
/// ([`ModelRegistry::reload_from_config`]): the prices it left with no
/// figure (MODEL-ROWS) and the capability flags discovery reads that it
/// left out (CAP-FLAGS, *Rev 110* (3) (b)).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PersonRow {
    gaps: PriceGaps,
    left_out: DiscoveredFlags,
}

/// The capability flags a provider's discovery reads
/// ([`DiscoveredModel`]): kept per id as discovery last read them
/// ([`ModelRegistry::register_discovered`]) and, in a [`PersonRow`],
/// `true` for each flag the row left out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DiscoveredFlags {
    supports_image: bool,
    supports_tools: bool,
}

/// What a model's prompt cache costs, as multiples of the input rate the
/// request pays (CACHE-MULT and CACHE-WRITE, the owner's items 9 and 10 of
/// 2026-11-14).
///
/// Multiples, not prices, so that a long-prompt rate scales them as the
/// pages do ("2x input and cache rates", `gpt-6.1-sol`'s page). A write is
/// priced at the lifetime the arm's requests get, since neither arm asks
/// for another: Anthropic's 5-minute default and OpenAI's `"30m"` default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheRates {
    /// A cached input token read (a hit), as a multiple of the input rate.
    pub read: f64,
    /// An input token written to the cache, as a multiple of the input
    /// rate; 1.0 where the page charges no more than ordinary input.
    pub write: f64,
}

impl CacheRates {
    /// A row that states no figure — a discovered model, an id no row
    /// names, a `[models]` row for an id no compiled row names — keeps the
    /// tracker's figures from before the per-row ones: reads at 0.1x,
    /// writes at 1.25x.
    pub const UNSTATED: CacheRates = CacheRates {
        read: 0.10,
        write: 1.25,
    };
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
/// (C7a, M-3d), and what it must be asked for to give what the app shows
/// (ANTHROPIC-THINKING): each flag set only on a compiled row whose page,
/// read on the day the flag was written, says so, and named there with the
/// page and the date. A row with none set gets the request its caller asked
/// for.
///
/// The router meets the contract where a request first names its model
/// (`LlmRouter::meet_request_contract`): it leaves out what the model
/// refuses, sends `auto` for a forced tool choice and says so once a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RequestContract {
    /// `temperature`, `top_p` and `top_k` are refused: on Anthropic a 400 at
    /// a non-default value under the API version the arm sends; on OpenAI's
    /// GPT-6 rows, removed "when reasoning effort is not `none`", which it
    /// never is on a request the arm builds (it sends no effort).
    pub no_sampling_parameters: bool,
    /// Thinking is always on: `thinking: {"type": "disabled"}` is refused.
    pub thinking_always_on: bool,
    /// Manual thinking budgets (`thinking: {"type": "enabled", ...}`) are
    /// refused; adaptive thinking is the model's only mode.
    pub adaptive_thinking_only: bool,
    /// A forced tool choice (`any`, or a named tool) is refused.
    pub no_forced_tool_choice: bool,
    /// The model takes adaptive thinking and its thinking's `display`
    /// defaults to `"omitted"`, so a request asks for `"summarized"`: the
    /// app's reasoning stream otherwise gets no text. Never set on a row
    /// whose model rejects `adaptive`.
    pub summarized_thinking: bool,
}

impl RequestContract {
    /// A model whose pages state no request rule the arm would break.
    pub const NONE: Self = Self {
        no_sampling_parameters: false,
        thinking_always_on: false,
        adaptive_thinking_only: false,
        no_forced_tool_choice: false,
        summarized_thinking: false,
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
    /// at any length. Only a compiled row states one; a `[models]` row the
    /// person writes for the same id keeps it for each price the row leaves
    /// out, since that half counts at the compiled prices, and a price the
    /// row states is its own at every length (MODEL-ROWS): `None` when the
    /// row states both.
    pub long_prompt: Option<LongPromptRate>,
    /// What a cache read and a cache write cost on this model, for a
    /// compiled row whose pages state them (CACHE-MULT, CACHE-WRITE);
    /// `None` for any other row, priced at [`CacheRates::UNSTATED`]. A
    /// `[models]` row, which has no cache keys, keeps the compiled row's
    /// figures for the same id (*Rev 67*, decided with *Rev 97* (b)).
    pub cache_rates: Option<CacheRates>,
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
    /// The providers, by name, the router judges to run on a local server:
    /// the runtime config's `base_url` and the address the provider was
    /// built to call (`LlmProvider::base_url`) both local (`is_local_url`:
    /// loopback or the home network). Kept in step by the router
    /// (`LlmRouter::note_local_server_providers`) at construction, on every
    /// runtime-config reload and whenever a provider is registered or
    /// removed: what their discovery lists runs on a machine of the
    /// person's own (item 17).
    local_server_providers: RwLock<HashSet<String>>,
    /// The ids a `[models]` row the person wrote put here
    /// ([`Self::reload_from_config`]): such a row is the person's, not a
    /// compiled one ([`Self::is_compiled_only`], COMPAT-SERVER), and with
    /// each the prices it left with no figure, neither the row nor a
    /// compiled row stating one (MODEL-ROWS, *Rev 97* (b)).
    /// The flags each row left out ride with it (CAP-FLAGS).
    person_rows: RwLock<HashMap<String, PersonRow>>,
    /// The window a provider's discovery last gave each id, when it gave
    /// one (Ollama's `/api/show`, or its 8,192 default): what a `[models]`
    /// row that leaves out `context` keeps when no compiled row names the
    /// id, however the row and the discovery are ordered (MODEL-ROWS-2,
    /// *Rev 102* (4) (b)). Kept apart from the row so that a reload never
    /// reads back, as discovered, a figure a deleted `context` line gave.
    discovered_windows: RwLock<HashMap<String, u32>>,
    /// The capability flags a provider's discovery last read for each id
    /// (Ollama's `/api/show`; a bare listing's defaults): what a `[models]`
    /// row that leaves a flag out keeps when no compiled row names the id,
    /// however the row and the discovery are ordered (CAP-FLAGS, *Rev 110*
    /// (3) (b), as [`Self::discovered_windows`] does for `context`).
    discovered_flags: RwLock<HashMap<String, DiscoveredFlags>>,
}

impl ModelRegistry {
    pub fn new(models: HashMap<String, ModelInfo>) -> Self {
        Self {
            models: RwLock::new(models),
            local_server_providers: RwLock::new(HashSet::new()),
            person_rows: RwLock::new(HashMap::new()),
            discovered_windows: RwLock::new(HashMap::new()),
            discovered_flags: RwLock::new(HashMap::new()),
        }
    }

    /// Create a registry with well-known models pre-populated.
    pub fn with_defaults() -> Self {
        Self::new(Self::default_models())
    }

    /// The providers that run on a local server from now on: both the
    /// runtime config's `base_url` and the provider's own address local, as
    /// `LlmRouter::note_local_server_providers` judges them at construction,
    /// on each reload and on each provider registration or removal.
    pub(crate) fn set_local_server_providers(&self, providers: HashSet<String>) {
        *self
            .local_server_providers
            .write()
            .unwrap_or_else(|p| p.into_inner()) = providers;
    }

    /// The compiled default catalogue — every model this build knows how to
    /// price and route without asking a provider's API.
    fn default_models() -> HashMap<String, ModelInfo> {
        // MODELS-LATEST (the subagent design's §4.9; the owner's models
        // directive of 2026-11-14, "retire all the old models ... and
        // replace them with the latest", and his agreement the same day, "I
        // agree on the new models"): the eight latest models, undated, and
        // no other row. An undated id is a pinned snapshot, never a moving
        // pointer ("Every Claude model ID is a pinned snapshot, including the
        // dateless IDs", platform.claude.com/docs/en/about-claude/models/
        // overview; each OpenAI row's page gives the undated id as its
        // "Default snapshot"), so "latest" moves only with a build. Every
        // page below was read on 2026-11-14.
        let mut models = HashMap::new();

        // Anthropic (discovered: false — only for internal routing/pricing).
        // The models overview's "Latest models" table and each model's own
        // overview (platform.claude.com/docs/en/models/<model>/overview):
        // "Context window: 1M tokens · Max output: 128K tokens"; text and
        // image in and tool use ("All current models support text and image
        // input, ... vision, and tool use", the overview); PDFs ("All active
        // models support PDF processing", .../build-with-claude/pdf-support).
        //
        // Prices, the pricing page (.../about-claude/pricing, "Model
        // pricing"): a read ("Cache hits and refreshes") at 0.025x the input
        // price on Fable 5.2 (footnote 1), 0.05x on Opus 5.5 and Sonnet 5.5
        // (footnote 2) and "the standard 0.1x multiplier" on Haiku 5.5; a
        // write at 1.25x, the "5m cache writes" column, the lifetime the
        // arm's `cache_control: {"type": "ephemeral"}` gets ("By default,
        // the cache has a 5-minute lifetime", .../build-with-claude/
        // prompt-caching); the arm never asks for the 1-hour write (2x).
        // "Long context pricing": "Claude 4.6 and later models (except
        // Claude Haiku 5.5) ... include the full 1M token context window at
        // standard pricing"; Haiku 5.5's "prompt is over 100,000 tokens pays
        // higher prices", counting "all of its input tokens, including cache
        // reads and cache writes" — the folded `Usage.input_tokens` the
        // cost tracker compares.
        //
        // Request contracts (C7a, ANTHROPIC-THINKING; the design's §4.9.3):
        // .../build-with-claude/thinking, "Sampling parameters": on all four
        // "non-default `temperature`, `top_p`, or `top_k` values return a 400
        // error on every request, regardless of whether thinking is used";
        // "Response prefill and forced tool use": Opus 5.5, Sonnet 5.5 and
        // Fable 5.2 "reject forced tool use on every request with a 400
        // error"; *Controlling thinking display*: `"omitted"` "is the
        // default" on all four. The thinking-troubleshooting table
        // (.../build-with-claude/thinking-troubleshooting): Fable 5.2 and
        // Opus 5.5 "Adaptive only", "Always on", rejecting `"enabled"` and
        // `"disabled"`; Sonnet 5.5 "Adaptive, `between_tools`", rejecting
        // both — `between_tools` "in place of `"disabled"`", not sent
        // (ML-5), so a `Disabled` request is dropped and the model thinks;
        // Haiku 5.5 "Adaptive only", rejecting `"enabled"` and accepting
        // `"disabled"` "at effort `high` or below" (the arm sends no effort,
        // so its default `medium` holds). The model-deprecations page: the
        // sampling parameters "Removed in API version `2026-10-21`"; the
        // arm sends `2023-06-01`, one of the "Earlier API versions" that
        // "return it [a 400] when set to a non-default value on Claude 4.7
        // and later models".
        let anthropic = |input: f64,
                         output: f64,
                         cache_read: f64,
                         long_prompt: Option<LongPromptRate>,
                         request_contract: RequestContract| ModelInfo {
            provider: ProviderType::Anthropic,
            input_price_per_million: input,
            output_price_per_million: output,
            context_window: 1_000_000,
            discovered: false,
            supports_image: true,
            supports_audio: false,
            supports_document: true,
            supports_reasoning: false,
            supports_tools: true,
            declared: true,
            long_prompt,
            cache_rates: Some(CacheRates {
                read: cache_read,
                write: 1.25,
            }),
            request_contract,
        };
        let refuses_all = RequestContract {
            no_sampling_parameters: true,
            thinking_always_on: true,
            adaptive_thinking_only: true,
            no_forced_tool_choice: true,
            summarized_thinking: true,
        };
        // Claude Fable 5.2, the flagship ("For demanding reasoning and
        // long-horizon agentic work"; released November 9, 2026): $8 / $40,
        // reads $0.20, writes $10. Catalogued, no alias's (O-13 kept); its
        // migration guide's 30-day retention rule and prefix check reach only
        // a person who names it.
        models.insert(
            "claude-fable-5-2".to_string(),
            anthropic(8.0, 40.0, 0.025, None, refuses_all),
        );
        // Claude Opus 5.5, the latest Opus and `strong`'s row (O-13 (a)):
        // $4 / $20, reads $0.20, writes $5. Its migration guide, "What every
        // request to Claude Opus 5.5 must satisfy", as C7a read it.
        models.insert(
            "claude-opus-5-5".to_string(),
            anthropic(4.0, 20.0, 0.05, None, refuses_all),
        );
        // Claude Sonnet 5.5, `balanced`'s row ("The best combination of
        // speed and intelligence"): $2 / $10, reads $0.10, writes $2.50.
        models.insert(
            "claude-sonnet-5-5".to_string(),
            anthropic(2.0, 10.0, 0.05, None, refuses_all),
        );
        // Claude Haiku 5.5, `fast`'s row ("For high-volume,
        // latency-sensitive tasks"): $0.10 / $0.50, reads $0.01, writes
        // $0.125 "for prompts up to 100,000 tokens"; over it $0.50 / $2.50,
        // reads $0.05, writes $0.625 — the same multiples of the upper input
        // rate. Its migration guide: it "accepts a forced `tool_choice` (`any`
        // or a named tool), but the response starts with the tool call and
        // has no `thinking` block", so a forced choice is sent as asked.
        models.insert(
            "claude-haiku-5-5".to_string(),
            anthropic(
                0.10,
                0.50,
                0.10,
                Some(LongPromptRate {
                    above_input_tokens: 100_000,
                    input_price_per_million: 0.50,
                    output_price_per_million: 2.50,
                }),
                RequestContract {
                    thinking_always_on: false,
                    no_forced_tool_choice: false,
                    ..refuses_all
                },
            ),
        );

        // OpenAI: the "Flagship models" of developers.openai.com/api/docs/
        // models/all and each model's page (.../models/<id>): "1,050,000
        // context window" (the window O-17 (a) keeps until step 19's live
        // check; "Maximum input tokens: 922,000" is the owner's option,
        // ML-3), "128,000 max output tokens", "Input modalities: text,
        // image", "Reasoning token support", function calling; no audio.
        // PDFs: the arm sends none as a file part (a PDF reaches the model as
        // extracted text), so no row says it takes one.
        //
        // Prices, each model's page and the pricing page (.../pricing,
        // "Standard pricing data"): input, "Cached input" (0.1x on Astra and
        // Luna, "5% of the uncached input token rate" on Sol and Terra) and
        // "Cache writes are billed at 1.25x the uncached input token rate",
        // the default `"30m"` lifetime: the arm sends no
        // `prompt_cache_options`, and "Omitting both settings preserves the
        // default `"30m"` behavior" (.../guides/prompt-caching); it never
        // asks for the 12-hour write. "Prompts with more than 272K input
        // tokens are priced at 2x input and cache rates and 1.5x output for
        // the full request" on all four.
        //
        // Request contract (item 4 of the owner's answers of 2026-11-14,
        // taken: `no_sampling_parameters` on `gpt-6.1-sol` "and on the other
        // OpenAI rows whose pages say so"): the GPT-6 guide (.../guides/
        // latest-model) — "When reasoning effort is not `none`, remove
        // `temperature`, `top_p`, and `top_logprobs`"; the arm sends no
        // effort for a request that carries `Disabled` or no thinking, so
        // each model reasons at its default (`medium` on Sol, Terra and
        // Luna), and none of the four may be sent a sampling parameter. The
        // other four flags describe Anthropic's `thinking` wire. Tool calls:
        // "tool calling requires Responses" (the same guide), which the arm
        // speaks for a request with tools to OpenAI's own API (C7b).
        let openai =
            |input: f64, output: f64, cache_read: f64, long_input: f64, long_output: f64| {
                ModelInfo {
                    provider: ProviderType::OpenAI,
                    input_price_per_million: input,
                    output_price_per_million: output,
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
                        input_price_per_million: long_input,
                        output_price_per_million: long_output,
                    }),
                    cache_rates: Some(CacheRates {
                        read: cache_read,
                        write: 1.25,
                    }),
                    request_contract: RequestContract {
                        no_sampling_parameters: true,
                        ..RequestContract::NONE
                    },
                }
            };
        // GPT-6 Astra, the flagship ("Our most capable model for the most
        // demanding work"): $10 / $50, cached $1, writes $12.50; above 272K
        // $20 / $75. Its `reasoning.effort` has no `none`. Catalogued, no
        // alias's (O-13 kept: "Choose GPT-6.1 Sol to balance intelligence
        // and cost", .../models).
        models.insert(
            "gpt-6-astra".to_string(),
            openai(10.0, 50.0, 0.10, 20.0, 75.0),
        );
        // GPT-6.1 Sol, the latest Sol and `strong`'s row (O-13 (a), O-15
        // (a)): $2 / $10, cached $0.10, writes $2.50; above 272K $4 / $15.
        models.insert(
            "gpt-6.1-sol".to_string(),
            openai(2.0, 10.0, 0.05, 4.0, 15.0),
        );
        // GPT-6.1 Terra, `balanced`'s row (`gpt-5-mini`'s replacement on the
        // deprecations page): $1 / $5, cached $0.05, writes $1.25; above
        // 272K $2 / $7.50.
        models.insert(
            "gpt-6.1-terra".to_string(),
            openai(1.0, 5.0, 0.05, 2.0, 7.50),
        );
        // GPT-6 Luna, `fast`'s row and the seeded OpenAI default (the
        // deprecations page names it the replacement of `gpt-5-nano`, GPT-4o
        // and GPT-4o mini; "Our most efficient model for focused,
        // high-volume tasks"): $0.10 / $0.50, cached $0.01, writes $0.125;
        // above 272K $0.20 / $0.75. "Chat Completions supports function
        // calling only with `reasoning_effort` set to `none`": a request
        // with tools to OpenAI's own API goes to Responses (C7b).
        models.insert(
            "gpt-6-luna".to_string(),
            openai(0.10, 0.50, 0.10, 0.20, 0.75),
        );

        // Retired by the directive, with every site that named them: Claude
        // Opus 4.6, Sonnet 4.6, Sonnet 4.5 (`claude-sonnet-4-5-20250929`)
        // and Haiku 4.5 (`claude-haiku-4-5-20251001`); `gpt-5.2`,
        // `gpt-5-mini`, `gpt-5-nano`, `gpt-4o-mini` and `o3`. Several still
        // answer until their shutdown dates; a discovered listing of one is
        // a bare row priced at the fallback (§4.9.8, the owner's open item).
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
    ///
    /// A price the row leaves out is no price (MODEL-ROWS, *Rev 97* (b),
    /// the advisor's pick of 2026-11-16 under the owner's delegation): the
    /// compiled row's for the same id where there is one, else none — the
    /// cost tracker then counts it as it counts an unpriced discovered row
    /// ([`Self::get_pricing`]'s `unpriced`). An explicit `0` is a price.
    /// The compiled row's cache figures are kept, the row having no cache
    /// keys (*Rev 67*), and its long-prompt rate for each price the row
    /// leaves out: a price the row states is its own at every length.
    ///
    /// A `context` the row leaves out is the same: the compiled row's
    /// window, else the one discovery gave the id, else none (`0`, which a
    /// later discovery fills and the agent loop reads as its own 200,000
    /// default) — never a figure of its own (MODEL-ROWS-2, *Rev 102* (4)
    /// (b), *Rev 97*'s rule field by field).
    ///
    /// A capability flag the row leaves out is the same (CAP-FLAGS, *Rev
    /// 110* (3) (b)): the compiled row's flag, else what discovery last
    /// read for the id (`supports_image`, `supports_tools`), else the old
    /// default — `false`, but `true` for `supports_tools`. A flag the row
    /// states is its own. Discovery read after the row fills the flags it
    /// left out ([`Self::register_discovered`]).
    pub fn reload_from_config(
        &self,
        config_models: &HashMap<String, crate::config::ModelConfigEntry>,
        disabled: &HashSet<ProviderType>,
    ) {
        // A row the person writes prices a model; its request contract is
        // the model's own, so the compiled row's is kept (C7a), and so are
        // the compiled figures it leaves out (MODEL-ROWS).
        let compiled = Self::default_models();
        let discovered_windows = self
            .discovered_windows
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let discovered_flags = self
            .discovered_flags
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        for (model_id, entry) in config_models {
            if let Some(provider) = crate::config::parse_provider_type_pub(&entry.provider) {
                if disabled.contains(&provider) {
                    continue;
                }
                let compiled_row = compiled.get(model_id);
                let input = entry
                    .input_price
                    .or(compiled_row.map(|row| row.input_price_per_million));
                let output = entry
                    .output_price
                    .or(compiled_row.map(|row| row.output_price_per_million));
                let states_both_prices =
                    entry.input_price.is_some() && entry.output_price.is_some();
                self.person_rows
                    .write()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(
                        model_id.clone(),
                        PersonRow {
                            gaps: PriceGaps {
                                input: input.is_none(),
                                output: output.is_none(),
                            },
                            left_out: DiscoveredFlags {
                                supports_image: entry.supports_image.is_none(),
                                supports_tools: entry.supports_tools.is_none(),
                            },
                        },
                    );
                // A flag the row leaves out: the compiled row's, else what
                // discovery read, else the default (CAP-FLAGS).
                let read = discovered_flags.get(model_id);
                let flag = |stated: Option<bool>, of: fn(&ModelInfo) -> bool| {
                    stated.or(compiled_row.map(of))
                };
                self.register(
                    model_id.clone(),
                    ModelInfo {
                        provider,
                        // A placeholder where `PriceGaps` names the half.
                        input_price_per_million: input.unwrap_or(0.0),
                        output_price_per_million: output.unwrap_or(0.0),
                        context_window: entry
                            .context
                            .or(compiled_row.map(|row| row.context_window))
                            .or_else(|| discovered_windows.get(model_id).copied())
                            .unwrap_or(0),
                        discovered: false,
                        supports_image: flag(entry.supports_image, |row| row.supports_image)
                            .or(read.map(|flags| flags.supports_image))
                            .unwrap_or(false),
                        supports_audio: flag(entry.supports_audio, |row| row.supports_audio)
                            .unwrap_or(false),
                        supports_document: flag(entry.supports_document, |row| {
                            row.supports_document
                        })
                        .unwrap_or(false),
                        supports_reasoning: flag(entry.supports_reasoning, |row| {
                            row.supports_reasoning
                        })
                        .unwrap_or(false),
                        // The one flag that defaults on. The others withhold a
                        // content kind when omitted; this one only orders the
                        // ladder's preference, and a row the owner wrote for a
                        // model they mean to run agents on is tool-capable
                        // until they or discovery say otherwise.
                        supports_tools: flag(entry.supports_tools, |row| row.supports_tools)
                            .or(read.map(|flags| flags.supports_tools))
                            .unwrap_or(true),
                        declared: true,
                        // Per half: a price the row leaves out is the
                        // compiled row's, its long-prompt rate included; a
                        // price the row states is its own at every length.
                        long_prompt: compiled_row
                            .and_then(|row| row.long_prompt)
                            .filter(|_| !states_both_prices)
                            .map(|long| LongPromptRate {
                                above_input_tokens: long.above_input_tokens,
                                input_price_per_million: entry
                                    .input_price
                                    .unwrap_or(long.input_price_per_million),
                                output_price_per_million: entry
                                    .output_price
                                    .unwrap_or(long.output_price_per_million),
                            }),
                        cache_rates: compiled_row.and_then(|row| row.cache_rates),
                        request_contract: compiled_row
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

    /// Whether `model_id`'s row is this build's compiled row and nothing
    /// more: no discovery listed it and no `[models]` row of the person's
    /// wrote it. Such a row names a model of its provider type's vendor,
    /// which a provider built to call another server does not serve
    /// (`LlmProvider::serves_compiled_rows`, COMPAT-SERVER): the router
    /// does not route it there. `false` for an id with no row.
    pub fn is_compiled_only(&self, model_id: &str) -> bool {
        if !COMPILED_IDS.contains(model_id)
            || self
                .person_rows
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .contains_key(model_id)
        {
            return false;
        }
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .is_some_and(|info| !info.discovered)
    }

    /// The price the registry knows for a model.
    ///
    /// `None` for an id it has never met, and for one with no price anyone
    /// stated ([`Self::is_unpriced`], both halves): only a keyed provider's
    /// discovery registered it, or a `[models]` row left out both prices
    /// and no compiled row names it — that row's 0 / 0 is the absence of a
    /// price, not a price (O-18; MODEL-ROWS). A row missing one price
    /// names it in `unpriced`.
    pub fn get_pricing(&self, model_id: &str) -> Option<PricingInfo> {
        let local = self.local_server_providers();
        let person_rows = self.person_rows.read().unwrap_or_else(|p| p.into_inner());
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .map(|info| (info, Self::unpriced(model_id, info, &local, &person_rows)))
            .filter(|(_, gaps)| *gaps != PriceGaps::BOTH)
            .map(|(info, unpriced)| PricingInfo {
                input_price_per_million: info.input_price_per_million,
                output_price_per_million: info.output_price_per_million,
                long_prompt: info.long_prompt,
                cache_rates: info.cache_rates,
                unpriced,
            })
    }

    /// Whether this id is registered with a price no one stated: a keyed
    /// provider's discovery listed it and neither a compiled row nor a
    /// `[models]` row prices it, or a `[models]` row left a price out and
    /// no compiled row names the id (MODEL-ROWS, *Rev 97* (b)).
    ///
    /// Such a row stays routable; the cost tracker counts the missing price
    /// at its fallback for an unknown id (O-18 (b)). It is priced at 0
    /// instead where it runs on this machine: a local provider's (L8;
    /// "local" is the set whose provider needs no key, only Ollama), and a
    /// keyed provider's the router found on a local server (item 17): its
    /// runtime-config `base_url` (learning's LR7 test) and the address it
    /// was built to call both on loopback or the home network.
    pub fn is_unpriced(&self, model_id: &str) -> bool {
        let local = self.local_server_providers();
        let person_rows = self.person_rows.read().unwrap_or_else(|p| p.into_inner());
        self.models
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(model_id)
            .is_some_and(|info| Self::unpriced(model_id, info, &local, &person_rows).any())
    }

    /// Whether `provider` runs on a local server though keyed (item 17),
    /// as the router last judged it. Read by tests only since the window
    /// warning's scope became every server not OpenAI's own (MODEL-ROWS-2).
    #[cfg(test)]
    pub(crate) fn is_local_server_provider(&self, provider: &ProviderType) -> bool {
        self.local_server_providers()
            .contains(&provider.to_string())
    }

    /// The prices of `info` (registered as `model_id`) the tracker counts at
    /// its fallback: none on a provider that runs on this machine or the
    /// home network; both for a row only discovery wrote; for a person's
    /// row, the ones it left out with no compiled row behind them.
    fn unpriced(
        model_id: &str,
        info: &ModelInfo,
        local_servers: &HashSet<String>,
        person_rows: &HashMap<String, PersonRow>,
    ) -> PriceGaps {
        if info.provider.is_local() || local_servers.contains(&info.provider.to_string()) {
            PriceGaps::default()
        } else if !info.declared {
            PriceGaps::BOTH
        } else {
            person_rows
                .get(model_id)
                .map(|row| row.gaps)
                .unwrap_or_default()
        }
    }

    /// Taken before `person_rows` and `models`, never while holding either
    /// (nothing takes them the other way round; `person_rows` is likewise
    /// never taken while holding `models`). `discovered_windows` and
    /// `discovered_flags` are each never held while another of the five is
    /// taken, nor taken while one is held.
    fn local_server_providers(&self) -> std::sync::RwLockReadGuard<'_, HashSet<String>> {
        self.local_server_providers
            .read()
            .unwrap_or_else(|p| p.into_inner())
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
    ///
    /// Second only to that confirmation, the provider's `fast` alias row,
    /// when it is registered for this provider (the subagent design's §4.9.9
    /// ML-7, MODELS-LATEST): with the flagships catalogued the id order would
    /// put `claude-fable-5-2` before `claude-haiku-5-5` and `gpt-6-astra`
    /// before `gpt-6-luna`, running work nobody chose a model for on the
    /// dearest one. With no discovery every row is declared and the `fast`
    /// row leads; a discovery that lists it marks it discovered, so it leads
    /// again. A discovery that does not list it — a person's own
    /// OpenAI-compatible server behind `[providers.openai]`, say — keeps a
    /// model the server serves ahead of a compiled row it does not. A
    /// provider with no alias row (Ollama), or whose `fast` row is not
    /// registered, keeps the order above.
    pub fn first_model_for_provider(&self, provider: &ProviderType) -> Option<String> {
        self.models_for_provider_in_order(provider)
            .into_iter()
            .next()
    }

    /// Every model of `provider`, in [`Self::first_model_for_provider`]'s
    /// order: the router takes the first it can route, since a compiled
    /// row leads with no discovery and is not routable on a provider built
    /// to call another server than its vendor's (COMPAT-SERVER).
    pub fn models_for_provider_in_order(&self, provider: &ProviderType) -> Vec<String> {
        let models = self.models.read().unwrap_or_else(|p| p.into_inner());
        let fast = crate::routing::model_alias::compiled_alias(
            provider,
            crate::routing::model_alias::ModelAlias::Fast,
        );
        let mut candidates: Vec<(&String, &ModelInfo)> = models
            .iter()
            .filter(|(_, info)| &info.provider == provider)
            .collect();
        candidates.sort_by(|(a_id, a), (b_id, b)| {
            let is_fast = |id: &String| fast.is_some_and(|f| f == id.as_str());
            b.discovered
                .cmp(&a.discovered)
                .then(is_fast(b_id).cmp(&is_fast(a_id)))
                .then(b.supports_tools.cmp(&a.supports_tools))
                .then(a_id.cmp(b_id))
        });
        candidates.into_iter().map(|(id, _)| id.clone()).collect()
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
        self.person_rows
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(model_id);
        self.discovered_windows
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(model_id);
        self.discovered_flags
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(model_id);
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
        drop(models);
        let mut person_rows = self.person_rows.write().unwrap_or_else(|p| p.into_inner());
        for id in &to_remove {
            person_rows.remove(id);
        }
        drop(person_rows);
        let mut windows = self
            .discovered_windows
            .write()
            .unwrap_or_else(|p| p.into_inner());
        for id in &to_remove {
            windows.remove(id);
        }
        drop(windows);
        let mut flags = self
            .discovered_flags
            .write()
            .unwrap_or_else(|p| p.into_inner());
        for id in &to_remove {
            flags.remove(id);
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
    ///
    /// A window the provider gave is also recorded on its own, for a
    /// `[models]` row read later that leaves out `context`
    /// ([`Self::reload_from_config`]); so are the capability flags it read,
    /// for a row read later that leaves one out (CAP-FLAGS). A row read
    /// before, for an id no compiled row names, takes the flags it left
    /// out from this reading, the latest; a flag it states, and a compiled
    /// row's, stay.
    pub fn register_discovered(&self, model_id: String, info: ModelInfo) {
        if info.context_window > 0 {
            self.discovered_windows
                .write()
                .unwrap_or_else(|p| p.into_inner())
                .insert(model_id.clone(), info.context_window);
        }
        let read = DiscoveredFlags {
            supports_image: info.supports_image,
            supports_tools: info.supports_tools,
        };
        self.discovered_flags
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(model_id.clone(), read);
        let left_out = if COMPILED_IDS.contains(&model_id) {
            None
        } else {
            self.person_rows
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .get(&model_id)
                .map(|row| row.left_out)
        };
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
                if let Some(left_out) = left_out {
                    if left_out.supports_image {
                        existing.supports_image = read.supports_image;
                    }
                    if left_out.supports_tools {
                        existing.supports_tools = read.supports_tools;
                    }
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
        self.mark_discovered_for_provider(provider, true)
    }

    /// [`Self::mark_defaults_discovered_for_provider`] for a provider built
    /// to call another server than its vendor's (COMPAT-SERVER): its
    /// declared rows but the compiled ones, which that server does not
    /// serve ([`Self::is_compiled_only`]).
    pub fn mark_declared_discovered_for_provider(&self, provider: &ProviderType) -> usize {
        self.mark_discovered_for_provider(provider, false)
    }

    fn mark_discovered_for_provider(&self, provider: &ProviderType, compiled: bool) -> usize {
        let person_rows = self
            .person_rows
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut models = self.models.write().unwrap_or_else(|p| p.into_inner());
        let mut count = 0;
        for (id, info) in models.iter_mut() {
            let compiled_only = COMPILED_IDS.contains(id) && !person_rows.contains_key(id);
            if info.provider == *provider && !info.discovered && (compiled || !compiled_only) {
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
