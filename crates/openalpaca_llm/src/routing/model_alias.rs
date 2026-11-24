//! The model aliases `fast`, `balanced` and `strong` (the subagent design's
//! SA-3, M-2…M-5; C7).
//!
//! A template or a call names a model as `inherit`, one of the three
//! aliases, or a model id ([`ModelChoice`]). An alias means a different
//! model on each provider: [`ALIAS_TABLE`] is the compiled map, one row per
//! provider that has one, and a person may pin any alias per provider under
//! `[providers.<name>.model_aliases]` in `llm.toml`
//! ([`crate::config::ProviderDefaults::model_aliases`]). The router resolves
//! an alias against the parent's model ([`crate::LlmRouter::resolve_alias`]).
//!
//! "Aliases", not "tiers": `tier` already names an API key's rate tier
//! (`ApiKey.tier`). And not `haiku`, `sonnet` or `opus`: those are one
//! vendor's family names, and a model on an OpenAI- or Ollama-only install
//! reads the words.

use crate::keys::key_pool::ProviderType;

/// One of the three model aliases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModelAlias {
    /// The provider's small, cheap model.
    Fast,
    /// The provider's middle model.
    Balanced,
    /// The provider's latest flagship of the line the owner named (O-13).
    Strong,
}

impl ModelAlias {
    /// Every alias, in the order the docs list them.
    pub const ALL: [ModelAlias; 3] = [Self::Fast, Self::Balanced, Self::Strong];

    /// The word as written in a template, a call or `llm.toml`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Strong => "strong",
        }
    }

    /// The alias a word names, ignoring case and surrounding space; `None`
    /// for any other word.
    pub fn parse(word: &str) -> Option<Self> {
        let word = word.trim();
        Self::ALL
            .into_iter()
            .find(|alias| alias.as_str().eq_ignore_ascii_case(word))
    }
}

impl std::fmt::Display for ModelAlias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a template's `model:` or a call's `model` asks for (M-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelChoice {
    /// The parent's model (M-1): no value, an empty one, or `inherit`.
    Inherit,
    /// One of the three aliases, resolved per provider.
    Alias(ModelAlias),
    /// A model id, as written.
    Id(String),
}

impl ModelChoice {
    /// Read a value. `inherit` and the alias words are matched ignoring
    /// case; anything else is a model id, kept as written (trimmed).
    pub fn parse(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("inherit") {
            return Self::Inherit;
        }
        match ModelAlias::parse(value) {
            Some(alias) => Self::Alias(alias),
            None => Self::Id(value.to_string()),
        }
    }
}

/// One provider's compiled answer for each alias.
#[derive(Debug, Clone)]
pub struct AliasRow {
    pub provider: ProviderType,
    pub fast: &'static str,
    pub balanced: &'static str,
    pub strong: &'static str,
}

impl AliasRow {
    /// The model id this row names for `alias`.
    pub fn model(&self, alias: ModelAlias) -> &'static str {
        match alias {
            ModelAlias::Fast => self.fast,
            ModelAlias::Balanced => self.balanced,
            ModelAlias::Strong => self.strong,
        }
    }
}

/// The compiled alias map: a row per provider that has one (§4.3).
///
/// A compiled id per release (M-3b): "latest" is as current as the build,
/// and moving it is one line here with its catalogue row
/// (`ModelRegistry::default_models`). Every id is a catalogue row that can be
/// given tools, which a test pins. A provider with no row — Ollama, a
/// plugin's — answers every alias with the parent's model unless the person
/// pins one (M-4 step 3).
pub static ALIAS_TABLE: [AliasRow; 2] = [
    AliasRow {
        provider: ProviderType::Anthropic,
        // Claude Haiku 5.5 (MODELS-LATEST; the owner agreed 2026-11-14): "For
        // high-volume, latency-sensitive tasks such as classification,
        // extraction, and routing" (platform.claude.com/docs/en/models/
        // haiku-5-5/overview, read 2026-11-14).
        fast: "claude-haiku-5-5",
        // Claude Sonnet 5.5 (MODELS-LATEST): "The best combination of speed
        // and intelligence" (.../models/sonnet-5-5/overview, read
        // 2026-11-14).
        balanced: "claude-sonnet-5-5",
        // The latest Opus (O-13 (a)): Claude Opus 5.5, `claude-opus-5-5`
        // (the models overview, platform.claude.com/docs/en/models/overview,
        // read 2026-11-12, and again 2026-11-14). Claude Fable 5.2, the
        // flagship, is catalogued and no alias's.
        strong: "claude-opus-5-5",
    },
    AliasRow {
        provider: ProviderType::OpenAI,
        // GPT-6 Luna (MODELS-LATEST): "Our most efficient model for focused,
        // high-volume tasks" (developers.openai.com/api/docs/models/
        // gpt-6-luna), named `gpt-5-nano`'s replacement by .../deprecations
        // (both read 2026-11-14).
        fast: "gpt-6-luna",
        // GPT-6.1 Terra (MODELS-LATEST): "A fast, efficient model" (its
        // page), named `gpt-5-mini`'s replacement by the deprecations page
        // (both read 2026-11-14).
        balanced: "gpt-6.1-terra",
        // The latest GPT Sol (O-13 (a), O-15 (a)): GPT-6.1 Sol, `gpt-6.1-sol`
        // (developers.openai.com/api/docs/models/gpt-6.1-sol, read
        // 2026-11-12: "Tool calling requires the Responses API", which the
        // OpenAI arm speaks for a request with tools since C7b). GPT-6 Astra,
        // the flagship, is catalogued and no alias's.
        strong: "gpt-6.1-sol",
    },
];

/// The compiled id for `alias` on `provider`, if the table has a row for it.
pub fn compiled_alias(provider: &ProviderType, alias: ModelAlias) -> Option<&'static str> {
    ALIAS_TABLE
        .iter()
        .find(|row| &row.provider == provider)
        .map(|row| row.model(alias))
}
