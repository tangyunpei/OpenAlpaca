use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use super::router_config::LlmRouterConfig;
use crate::keys::key_pool::ProviderType;
use crate::routing::model_alias::ModelAlias;

/// Whether a base URL names a machine of the person's own: this one first
/// — `localhost`, `127.0.0.0/8` or `::1` (ruling LR7's second clause) — or
/// another on the home network (item 17, widened by the owner's second
/// answers of 2026-11-14: "local can include other computers on the home
/// network, but mainly localhost"): an IPv4 private address (`10/8`,
/// `172.16/12`, `192.168/16`) or link-local one (`169.254/16`), an IPv6
/// unique-local (`fc00::/7`) or link-local (`fe80::/10`) one, or an mDNS
/// name ending in `.local`. The host is read with the URL parser `reqwest`
/// connects by, so the host judged is the host reached.
///
/// No name is looked up: any other name (`gpu-box`, `gpu-box.lan`,
/// `localhost.` or `gpu-box.local.`, a trailing dot being another
/// spelling) is not local whatever it resolves to, and neither is an
/// IPv4-mapped IPv6 address. A false yes would price a paid host at 0, so
/// every doubtful spelling fails closed.
///
/// The one locality test: learning's yield rule
/// ([`LlmRuntimeConfig::is_on_local_server`]) and the cost tracker's
/// exemption for what a keyed provider's discovery lists (item 17 of the
/// owner's answer of 2026-11-14) both read it.
pub fn is_local_url(base_url: &str) -> bool {
    let Ok(url) = url::Url::parse(base_url) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Domain(name)) => {
            name.eq_ignore_ascii_case("localhost") || is_mdns_name(name)
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
        }
        None => false,
    }
}

/// A name of one or more labels under `.local`, mDNS's domain, no label
/// empty: `.local` and `gpu-box..local` are not, nor is `gpu-box.local.`,
/// whose last label is the empty one after the dot. The URL parser has
/// already lowercased an `http` or `https` name; the comparison ignores case
/// all the same, so the rule does not lean on the scheme.
fn is_mdns_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(head, last)| {
        last.eq_ignore_ascii_case("local") && head.split('.').all(|label| !label.is_empty())
    })
}

fn default_usage_cache_ttl() -> u64 {
    300
}
fn default_usage_fetch_timeout() -> u64 {
    10
}
fn default_cli_timeout() -> u64 {
    120
}
fn default_llm_request_timeout() -> u64 {
    120
}

/// The range [`LlmRuntimeConfig::request_timeout_for`] will accept: below a
/// second no call can finish, and a day is already past any wall clock a caller
/// has. A value outside it is clamped and said out loud, never taken as meant.
const MIN_REQUEST_TIMEOUT_SECS: u64 = 1;
const MAX_REQUEST_TIMEOUT_SECS: u64 = 86_400;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutsConfig {
    #[serde(default = "default_usage_cache_ttl")]
    pub usage_cache_ttl_secs: u64,
    #[serde(default = "default_usage_fetch_timeout")]
    pub usage_fetch_timeout_secs: u64,
    #[serde(default = "default_cli_timeout")]
    pub cli_backend_timeout_secs: u64,
    /// Wall-clock budget for one **non-streaming** LLM HTTP call, in seconds
    /// (default 120 — what the client was hard-coded to). A streamed call is
    /// not bounded by it: the HTTP layer bounds the connect and the idle gap
    /// between chunks, and the wall clock belongs to the loop's
    /// `max_stream_duration`, or `max_keyless_stream_duration` for a
    /// provider that needs no key (L7, LOCAL-CALL-HOUR).
    /// `[providers.<name>] request_timeout_secs`
    /// overrides it for one provider — a local model generating at 20 tok/s
    /// needs minutes where a cloud call needs seconds.
    #[serde(default = "default_llm_request_timeout")]
    pub llm_request_timeout_secs: u64,
}

impl Default for TimeoutsConfig {
    fn default() -> Self {
        Self {
            usage_cache_ttl_secs: default_usage_cache_ttl(),
            usage_fetch_timeout_secs: default_usage_fetch_timeout(),
            cli_backend_timeout_secs: default_cli_timeout(),
            llm_request_timeout_secs: default_llm_request_timeout(),
        }
    }
}

fn default_anthropic_usage_url() -> String {
    "https://api.anthropic.com/api/oauth/usage".to_string()
}
fn default_openai_usage_url() -> String {
    "https://api.openai.com/dashboard/billing/usage".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointsConfig {
    #[serde(default = "default_anthropic_usage_url")]
    pub anthropic_usage: String,
    #[serde(default = "default_openai_usage_url")]
    pub openai_usage: String,
}

impl Default for EndpointsConfig {
    fn default() -> Self {
        Self {
            anthropic_usage: default_anthropic_usage_url(),
            openai_usage: default_openai_usage_url(),
        }
    }
}

fn default_anthropic_env() -> String {
    "ANTHROPIC_API_KEY".to_string()
}
fn default_openai_env() -> String {
    "OPENAI_API_KEY".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVarsConfig {
    #[serde(default = "default_anthropic_env")]
    pub anthropic_api_key: String,
    #[serde(default = "default_openai_env")]
    pub openai_api_key: String,
}

impl Default for EnvVarsConfig {
    fn default() -> Self {
        Self {
            anthropic_api_key: default_anthropic_env(),
            openai_api_key: default_openai_env(),
        }
    }
}

/// Per-provider default settings resolved from config.
#[derive(Debug, Clone)]
pub struct ProviderDefaults {
    pub default_model: String,
    pub default_max_tokens: u32,
    pub base_url: Option<String>,
    /// `[providers.<name>] request_timeout_secs` — this provider's own
    /// non-streaming budget. `None` means the `[timeouts]` default applies.
    pub request_timeout_secs: Option<u64>,
    /// `[providers.<name>.model_aliases]`: the person's pin for each model
    /// alias on this provider, read at load and on every hot reload. An
    /// alias with no pin takes the compiled row (`ALIAS_TABLE`); a key that
    /// names no alias, or a pin that names no model, was warned of and left
    /// out ([`LlmRuntimeConfig::from`]).
    pub model_aliases: BTreeMap<ModelAlias, String>,
}

/// The orchestrator model a config that names none starts with, wherever
/// one is filled in: the router's default when `llm.toml` has no
/// `[orchestrator]` and no Anthropic default, the row
/// `update_orchestrator_config` inserts, and the CLI's own default and
/// `config reset ai.default_model` (the storage schema's `ai.default_model`
/// default spells the same id; a CLI test pins the two). Claude Sonnet 5.5,
/// the tier these sites held as `claude-sonnet-4-5-20250929` (the subagent
/// design's §4.9.9 ML-8; MODELS-LATEST).
pub const DEFAULT_ORCHESTRATOR_MODEL: &str = "claude-sonnet-5-5";

/// Runtime representation of all externalized LLM configuration.
/// Wrapped in ArcSwap for lock-free hot-reload.
#[derive(Debug, Clone)]
pub struct LlmRuntimeConfig {
    pub timeouts: TimeoutsConfig,
    pub endpoints: EndpointsConfig,
    pub env_vars: EnvVarsConfig,
    pub provider_defaults: HashMap<String, ProviderDefaults>,
}

/// The output budget of a request that names no `max_tokens`, when the
/// provider's `[providers.<name>] default_max_tokens` names none either:
/// 16,000 tokens on every provider (OUTPUT-BUDGET; the owner's item 8,
/// "budget can be 16000 or higher"), where it was 4096. It counts thinking
/// and reasoning as well as the answer. Every catalogued row takes it: each
/// of the eight latest models' pages gives a maximum output of 128K
/// (platform.claude.com/docs/en/models/{fable-5-2,opus-5-5,sonnet-5-5,
/// haiku-5-5}/overview; developers.openai.com/api/docs/models/{gpt-6-astra,
/// gpt-6.1-sol,gpt-6.1-terra,gpt-6-luna}; read 2026-11-15T05:47Z), so no
/// row is clamped. It stays under the figure above which Anthropic's SDK
/// "requires streaming", 21,333 (platform.claude.com/docs/en/
/// build-with-claude/thinking, read 2026-11-15T05:52Z); a lead's and a
/// subagent's rounds are streamed all the same (`LoopConfig::stream_rounds`
/// in `openalpaca_core`), so a long answer is not cut by the non-streaming
/// request timeout.
pub(crate) const DEFAULT_MAX_TOKENS: u32 = 16_000;

impl Default for LlmRuntimeConfig {
    fn default() -> Self {
        let mut provider_defaults = HashMap::new();
        provider_defaults.insert(
            "anthropic".to_string(),
            ProviderDefaults {
                // MODELS-LATEST (§4.9.5): Sonnet 4.5 is "Deprecated",
                // retiring November 30, 2026, its replacement
                // `claude-sonnet-5-5` (platform.claude.com/docs/en/
                // about-claude/model-deprecations, read 2026-11-14).
                default_model: "claude-sonnet-5-5".to_string(),
                default_max_tokens: DEFAULT_MAX_TOKENS,
                base_url: None,
                request_timeout_secs: None,
                model_aliases: BTreeMap::new(),
            },
        );
        provider_defaults.insert(
            "openai".to_string(),
            ProviderDefaults {
                // MODELS-LATEST (§4.9.5): GPT-4o's replacement is
                // `gpt-6-luna` (developers.openai.com/api/docs/deprecations,
                // read 2026-11-14).
                default_model: "gpt-6-luna".to_string(),
                default_max_tokens: DEFAULT_MAX_TOKENS,
                base_url: Some("https://api.openai.com/v1".to_string()),
                request_timeout_secs: None,
                model_aliases: BTreeMap::new(),
            },
        );
        provider_defaults.insert(
            "ollama".to_string(),
            ProviderDefaults {
                default_model: "llama3".to_string(),
                default_max_tokens: DEFAULT_MAX_TOKENS,
                base_url: Some("http://localhost:11434/v1".to_string()),
                request_timeout_secs: None,
                model_aliases: BTreeMap::new(),
            },
        );
        Self {
            timeouts: TimeoutsConfig::default(),
            endpoints: EndpointsConfig::default(),
            env_vars: EnvVarsConfig::default(),
            provider_defaults,
        }
    }
}

impl LlmRuntimeConfig {
    /// How long one **non-streaming** call to `provider` may take in total.
    ///
    /// `[providers.<name>] request_timeout_secs` when the file names one, else
    /// `[timeouts] llm_request_timeout_secs`, else 120 s — the value the HTTP
    /// client used to be built with, so a cloud provider that configures
    /// nothing behaves exactly as before (L7). Both provider-construction
    /// paths — the boot builder and the runtime registration a toggle performs
    /// — resolve it here, so they cannot disagree.
    pub fn request_timeout_for(&self, provider: &str) -> std::time::Duration {
        let configured = self
            .provider_defaults
            .get(provider)
            .and_then(|d| d.request_timeout_secs)
            .unwrap_or(self.timeouts.llm_request_timeout_secs);
        let clamped = configured.clamp(MIN_REQUEST_TIMEOUT_SECS, MAX_REQUEST_TIMEOUT_SECS);
        if clamped != configured {
            tracing::warn!(
                provider,
                configured,
                used = clamped,
                "request_timeout_secs is outside {MIN_REQUEST_TIMEOUT_SECS}..={MAX_REQUEST_TIMEOUT_SECS}s — clamped"
            );
        }
        std::time::Duration::from_secs(clamped)
    }

    /// Whether `provider`'s `base_url` is local ([`is_local_url`]): a
    /// llama.cpp or LM Studio server behind the `openai` provider runs on
    /// this machine, or another on the home network, though it is keyed.
    pub fn is_on_local_server(&self, provider: &ProviderType) -> bool {
        self.provider_defaults
            .get(&provider.to_string())
            .and_then(|d| d.base_url.as_deref())
            .is_some_and(is_local_url)
    }
}

/// The alias pins of `[providers.<provider>.model_aliases]` (M-3).
///
/// A key is matched as an alias word ignoring case; any other key, and a pin
/// that names no model, is warned of here — at load and at each hot reload,
/// which both build the runtime config through this — and ignored. An
/// unknown key is not refused: `llm.toml` keeps unknown keys everywhere else.
fn model_aliases_of(
    provider: &str,
    table: Option<&HashMap<String, String>>,
) -> BTreeMap<ModelAlias, String> {
    let mut pins = BTreeMap::new();
    let Some(table) = table else {
        return pins;
    };
    let mut keys: Vec<&String> = table.keys().collect();
    keys.sort();
    for key in keys {
        let model = table[key].trim();
        match ModelAlias::parse(key) {
            Some(alias) if !model.is_empty() => {
                pins.insert(alias, model.to_string());
            }
            Some(alias) => tracing::warn!(
                provider,
                alias = alias.as_str(),
                "[providers.{provider}.model_aliases] {alias} names no model — ignored; \
                 the alias keeps its built-in model"
            ),
            None => tracing::warn!(
                provider,
                key = key.as_str(),
                "[providers.{provider}.model_aliases] has a key '{key}' that is no model \
                 alias — ignored. The keys are fast, balanced or strong."
            ),
        }
    }
    pins
}

impl From<&LlmRouterConfig> for LlmRuntimeConfig {
    fn from(config: &LlmRouterConfig) -> Self {
        let timeouts = config.timeouts.clone().unwrap_or_default();
        let endpoints = config.endpoints.clone().unwrap_or_default();
        let env_vars = config.env_vars.clone().unwrap_or_default();

        let mut provider_defaults = LlmRuntimeConfig::default().provider_defaults;

        if let Some(ref providers) = config.providers {
            for (name, pc) in providers {
                let existing = provider_defaults.get(name);
                let defaults = ProviderDefaults {
                    default_model: pc
                        .default_model
                        .clone()
                        .or_else(|| existing.map(|e| e.default_model.clone()))
                        .unwrap_or_else(|| "unknown".to_string()),
                    default_max_tokens: pc
                        .default_max_tokens
                        .or_else(|| existing.map(|e| e.default_max_tokens))
                        .unwrap_or(DEFAULT_MAX_TOKENS),
                    base_url: pc
                        .base_url
                        .clone()
                        .or_else(|| existing.and_then(|e| e.base_url.clone())),
                    request_timeout_secs: pc
                        .request_timeout_secs
                        .or_else(|| existing.and_then(|e| e.request_timeout_secs)),
                    model_aliases: model_aliases_of(name, pc.model_aliases.as_ref()),
                };
                provider_defaults.insert(name.clone(), defaults);
            }
        }

        Self {
            timeouts,
            endpoints,
            env_vars,
            provider_defaults,
        }
    }
}
