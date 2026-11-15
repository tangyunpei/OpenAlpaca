//! Which model a lead and each subagent it spawns run on (C8; the subagent
//! design's SA-3, M-1…M-10, `tasks/subagent-brief-design.md` §4).
//!
//! A template's `model:` and a spawn call's `model` are a word or an id
//! ([`ModelChoice`]): `inherit` (also no value), one of the aliases `fast`,
//! `balanced` and `strong`, or a model id. The word is resolved here, against
//! the parent's model, **before** a loop config is built from the agent, so
//! the window, the cost check and every record read the id that will answer;
//! no word ever reaches the router as a model id.
//!
//! - **The lead** ([`lead_choice`], [`resolve`] with no parent): `inherit` is
//!   the router's effective default, the model a request naming none reaches
//!   (its parent, the main loop, requests none); a borrowed lead — a template
//!   without `orchestration` running as one — follows the shipped lead's
//!   choice instead (M-1a); an alias resolves against the effective default.
//! - **A subagent** ([`parent_of_child`], [`resolve`]): its parent's model is
//!   the one that answered its lead, read at spawn time from the lead's
//!   [`AnsweringModel`] slot, or the lead's own resolved model where the slot
//!   is empty or names a model this router cannot route (a provider can
//!   answer under a dated snapshot id its catalogue does not hold).
//! - **A per-call model** ([`per_call`]): resolved when the call runs and
//!   refused, with what can be chosen, when it names nothing routable —
//!   never substituted (M-7).
//!
//! An alias never leaves its parent's provider by itself
//! ([`LlmRouter::resolve_alias`], M-4).

use openalpaca_llm::{LlmRouter, ModelAlias, ModelChoice};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// The model that answered a lead's last round: written by its loop where
/// it records the round's model (`LoopConfig::answering_model`), read by its
/// spawn tool when a subagent on `inherit` starts (M-1).
pub type AnsweringModel = Arc<RwLock<Option<String>>>;

/// An empty slot, for a lead about to run.
pub fn answering_model() -> AnsweringModel {
    Arc::new(RwLock::new(None))
}

/// What the slot holds now.
pub fn read_answering(slot: &AnsweringModel) -> Option<String> {
    slot.read()
        .unwrap_or_else(|poisoned| {
            tracing::warn!("answering-model slot was poisoned; reading it anyway");
            poisoned.into_inner()
        })
        .clone()
}

/// Record the model that answered a round.
pub fn write_answering(slot: &AnsweringModel, model: &str) {
    *slot.write().unwrap_or_else(|poisoned| {
        tracing::warn!("answering-model slot was poisoned; writing it anyway");
        poisoned.into_inner()
    }) = Some(model.to_string());
}

/// The template the dispatcher leads with, whose choice a borrowed lead's
/// `inherit` follows (M-1a).
pub const SHIPPED_LEAD_TEMPLATE: &str = "lead_agent";

/// A template's `model:` (or an agent's `llm_config.model`) as a choice: no
/// value is `inherit` (M-1b).
pub fn template_choice(model: Option<&str>) -> ModelChoice {
    ModelChoice::parse(model.unwrap_or(""))
}

/// The word or id a choice is written as, for a record and a reply.
pub fn choice_word(choice: &ModelChoice) -> String {
    match choice {
        ModelChoice::Inherit => "inherit".to_string(),
        ModelChoice::Alias(alias) => alias.as_str().to_string(),
        ModelChoice::Id(id) => id.clone(),
    }
}

/// Whether a choice follows a parent or a table rather than naming an id:
/// such a choice resolves to a routable model, so its template's
/// `fallback_models` are not a request chain for it (M-6e).
pub fn is_word(choice: &ModelChoice) -> bool {
    !matches!(choice, ModelChoice::Id(_))
}

/// The lead's choice: its own template's word, but for a borrowed lead —
/// a template without the `orchestration` capability, run as a lead because
/// the singleton `lead_agent` is busy or absent (G-1) — whose `inherit`
/// follows the shipped lead's choice (M-1a), so concurrent workflows, and
/// every child that inherits from them, run on one model. That choice is
/// what a run of `lead_agent` gets — its template with the set stored over
/// it (AGENT-SETS), as `spawn_instance` builds it — never the template's
/// bare line. An explicit alias or id on a borrowed lead is kept.
pub fn lead_choice(
    registry: &crate::agent::AgentRegistry,
    lead: &crate::agent::subagent::SubAgent,
) -> ModelChoice {
    let own = template_choice(lead.llm_config.model.as_deref());
    let borrowed = !lead
        .capabilities
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case("orchestration"));
    if own != ModelChoice::Inherit || !borrowed || lead.template_id == SHIPPED_LEAD_TEMPLATE {
        return own;
    }
    registry
        .run_model(SHIPPED_LEAD_TEMPLATE)
        .map(|model| template_choice(model.as_deref()))
        .unwrap_or(own)
}

/// The `model:` a run of each resolved template gets, its stored set over
/// it (AGENT-SETS) — what M-6f's start line checks a `[fallback_chains]`
/// key against, so a key a set still names is not called stale.
pub fn template_models(registry: &crate::agent::AgentRegistry) -> Vec<Option<String>> {
    registry
        .list_templates()
        .into_iter()
        .filter_map(|t| registry.run_model(&t.frontmatter.id))
        .collect()
}

/// The model `choice` names for a run whose parent runs on `parent` (`None`:
/// the effective default, the lead's case). `inherit` is the parent's model
/// — the effective default when there is none — an alias the router's
/// resolution against it (M-4), an id itself, as written: an id that is not
/// routable walks the ladder at the call, announced, as it always has
/// (M-6d). `None` only when nothing at all is routable, which the call then
/// reports as `NoRoutableModel`.
pub fn resolve(router: &LlmRouter, choice: &ModelChoice, parent: Option<&str>) -> Option<String> {
    let parent = parent.map(str::trim).filter(|p| !p.is_empty());
    match choice {
        ModelChoice::Inherit => parent
            .map(str::to_string)
            .or_else(|| router.effective_default_model()),
        ModelChoice::Alias(alias) => router.resolve_alias(*alias, parent).map(|r| r.model),
        ModelChoice::Id(id) => Some(id.clone()),
    }
}

/// A subagent's parent model: the model that answered its lead, when the
/// router can route it; else the lead's own resolved model (M-1, M-6e).
pub fn parent_of_child(
    router: &LlmRouter,
    answering: Option<&AnsweringModel>,
    lead_model: Option<&str>,
) -> Option<String> {
    answering
        .and_then(read_answering)
        .filter(|m| router.is_routable(m))
        .or_else(|| lead_model.map(str::to_string))
}

/// The routable ids a refusal and the `model` parameter list: the first
/// [`MAX_LISTED_MODELS`], sorted, then how many more.
pub const MAX_LISTED_MODELS: usize = 30;

/// `ids` as the refusal and the parameter print them.
pub fn listed(ids: &[String]) -> String {
    if ids.is_empty() {
        return "none".to_string();
    }
    let shown = ids
        .iter()
        .take(MAX_LISTED_MODELS)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match ids.len().saturating_sub(MAX_LISTED_MODELS) {
        0 => shown,
        more => format!("{shown}, and {more} more"),
    }
}

/// What each alias names for a run whose parent runs on `parent`, in the
/// order the docs list them; an alias nothing routable answers is left out.
/// A listing, not a run's resolution: it announces nothing
/// ([`LlmRouter::peek_alias`]), so M-5's lines name only an alias a run used.
pub fn alias_models(router: &LlmRouter, parent: Option<&str>) -> Vec<(ModelAlias, String)> {
    ModelAlias::ALL
        .into_iter()
        .filter_map(|alias| router.peek_alias(alias, parent).map(|r| (alias, r.model)))
        .collect()
}

/// A spawn call's `model`, resolved when the call runs (M-7): the call's
/// value rank first, before the template's `model:`. `Ok` with the choice
/// and the routable model it names; `Err` with the refusal — the call's id
/// not routable, or nothing routable for a word — naming what can be chosen
/// now, so the lead can choose again. Never a substitution.
pub fn per_call(
    router: &LlmRouter,
    value: &str,
    parent: Option<&str>,
) -> Result<(ModelChoice, String), String> {
    let choice = ModelChoice::parse(value);
    let resolved = match &choice {
        ModelChoice::Id(id) => router.is_routable(id).then(|| id.clone()),
        word => resolve(router, word, parent),
    };
    match resolved {
        Some(model) => Ok((choice, model)),
        None => Err(not_routable(router, value.trim(), parent)),
    }
}

/// M-7's refusal text.
pub fn not_routable(router: &LlmRouter, value: &str, parent: Option<&str>) -> String {
    let aliases = alias_models(router, parent)
        .into_iter()
        .map(|(alias, model)| format!("{alias} → {model}"))
        .collect::<Vec<_>>();
    let aliases = if aliases.is_empty() {
        "none".to_string()
    } else {
        aliases.join(", ")
    };
    format!(
        "model '{value}' is not routable on this install. Routable now: {}. Aliases now: \
         {aliases}. Leave model out to use the template's.",
        listed(&router.routable_models())
    )
}

/// What a lead's spawn tools tell it it can choose (M-8): each alias's
/// model here, the routable ids and the lead's own model, read when the
/// lead's run builds its definitions. The default is the menu of no install:
/// the words alone, for a definition built outside a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelMenu {
    pub aliases: Vec<(ModelAlias, String)>,
    pub routable: Vec<String>,
    pub lead_model: Option<String>,
}

impl ModelMenu {
    /// The menu of a lead that will answer on `lead_model`: the model its
    /// resolved id reaches (an edited lead's unroutable id: the ladder's
    /// model, [`LlmRouter::would_answer`]), which its children inherit and
    /// its aliases resolve against.
    pub fn for_lead(router: &LlmRouter, lead_model: Option<&str>) -> Self {
        Self {
            aliases: alias_models(router, lead_model),
            routable: router.routable_models(),
            lead_model: lead_model.map(str::to_string),
        }
    }

    /// The `model` parameter's description on both spawn tools.
    pub fn description(&self) -> String {
        let tail = "The template's cost cap still applies.";
        if self.aliases.is_empty() && self.routable.is_empty() {
            return format!(
                "Run this subagent on another model than its template's: fast, balanced or \
                 strong (resolved on this install), or a routable model id. Omit it to use \
                 the template's model. {tail}"
            );
        }
        let distinct: std::collections::BTreeSet<&str> =
            self.aliases.iter().map(|(_, m)| m.as_str()).collect();
        let aliases = if self.aliases.len() == ModelAlias::ALL.len() && distinct.len() == 1 {
            format!(
                "Here fast, balanced and strong are all {}",
                self.aliases[0].1
            )
        } else {
            let parts: Vec<String> = self
                .aliases
                .iter()
                .enumerate()
                .map(|(i, (alias, model))| {
                    if i == 0 {
                        format!("{alias} is {model}")
                    } else {
                        format!("{alias} {model}")
                    }
                })
                .collect();
            format!("Here {}", parts.join(", "))
        };
        let lead = match &self.lead_model {
            Some(model) => format!(" — most templates inherit yours, {model}"),
            None => String::new(),
        };
        format!(
            "Run this subagent on another model than its template's. {aliases}; or one of: \
             {}. Omit it to use the template's model{lead}. {tail}",
            listed(&self.routable)
        )
    }
}

/// The three Claude ids an earlier bundle's templates pinned (M-6f): a
/// `[fallback_chains]` key naming one was written to steer them.
pub const EARLIER_BUNDLE_PINS: [&str; 3] = [
    "claude-sonnet-4-6",
    "claude-haiku-4-5-20251001",
    "claude-opus-4-6",
];

/// M-6f's start lines: one per key of `llm.toml`'s own `[fallback_chains]`
/// table (the parsed file's, never the router's merged map, which also
/// holds `[orchestrator] fallback_models` under the main model's id) that is
/// one of [`EARLIER_BUNDLE_PINS`], is not routable, and no resolved
/// template's `model:` names. Such an entry was written to route the shipped
/// templates on an install without Anthropic; since C8 it reaches nothing.
pub fn stale_fallback_chain_lines(
    chains: Option<&HashMap<String, Vec<String>>>,
    router: Option<&LlmRouter>,
    template_models: &[Option<String>],
) -> Vec<String> {
    let Some(chains) = chains else {
        return Vec::new();
    };
    let mut keys: Vec<&String> = chains
        .keys()
        .filter(|key| EARLIER_BUNDLE_PINS.contains(&key.as_str()))
        .filter(|key| !router.is_some_and(|r| r.is_routable(key)))
        .filter(|key| {
            !template_models
                .iter()
                .any(|m| m.as_deref().map(str::trim) == Some(key.as_str()))
        })
        .collect();
    keys.sort();
    keys.into_iter()
        .map(|key| {
            format!(
                "[fallback_chains].\"{key}\" no longer steers the built-in agents: they name \
                 inherit or a model alias since this build. Pin their models under \
                 [providers.<name>.model_aliases]."
            )
        })
        .collect()
}

#[cfg(test)]
#[path = "model_choice_tests.rs"]
mod tests;
