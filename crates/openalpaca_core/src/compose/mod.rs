//! Layered context + prompt composition engine (spec sections Components 1-5).
//!
//! Entry point: [`ComposeEngine::compose`].
//!
//! Phase 1 scaffolding: all types compile; stub layers produce minimal-but-valid
//! outputs; `ComposeEngine::compose` runs all five layers end-to-end and
//! returns a typed `ComposedRequest`. Phases 2 and 3 fill in the real layer
//! logic and the two-tier memoization.

mod assembly;
pub mod fingerprint;
pub mod persona;
mod static_prompt;
mod dynamic_context;
mod history;
pub mod types;

#[cfg(test)]
mod tests;

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use lru::LruCache;

pub use types::*;

/// The two kinds of entries in the tier-1 global cache (persona outputs and
/// static-prompt outputs are cached separately).
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum GlobalCacheKey {
    Persona(PersonaFingerprint),
    StaticPrompt(StaticPromptFingerprint),
}

/// Identifies which compose-engine layer emitted a telemetry event
/// (spec section Component 4 Events).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerId {
    Persona,
    StaticPrompt,
    DynamicContext,
    History,
}

/// Reason a compose-engine layer rebuilt its output (cache miss).
///
/// Structured attribution (spec section Component 3): Layer 1 + Layer 2 cache
/// misses are attributed to a specific sub-field of the input by diffing
/// the new sub-fingerprints against the most-recently-used existing entry.
/// Layer 3 + Layer 4 report `FirstBuild` on miss (per-lane attribution out
/// of scope for this cycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissReason {
    FirstBuild,
    PersonaChanged,
    AgentConfigChanged,
    ToolsChanged,
    SkillsChanged,
    SkillBlockChanged,
    BootstrapChanged,
    ConnectorStatusChanged,
    RawBlocksChanged,
    SendToolContextChanged,
    MessageSourceChanged,
    ModeChanged,
    ModelWindowChanged,
    IdentityBudgetChanged,
    UserBudgetChanged,
    AgentPersonaChanged,
    QueryChanged,
    MemoryChanged,
    LaneTipAdvanced,
    Unknown,
}

/// Per-sub-field fingerprints for Layer 1 (Persona) cache entries. Stored
/// alongside each cached output so `attribute_persona_miss` can diff against
/// the previous entry and return a specific `MissReason`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaSubFingerprints {
    pub persona_version: u64,
    pub mode_tag: u8,
    /// 9-byte encoding: [tag(0/1), u64_le(value)]. Zero when None.
    pub identity_budget: [u8; 9],
    pub user_budget: [u8; 9],
}

/// Per-sub-field fingerprints for Layer 2 (StaticPrompt) cache entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticPromptSubFingerprints {
    pub persona_fp: [u8; 32],
    pub agent_config_fp: [u8; 32],
    pub agent_persona_fp: [u8; 32],
    pub tools_fp: [u8; 32],
    pub skills_catalog_fp: [u8; 32],
    pub skill_block_fp: [u8; 32],
    pub bootstrap_fp: [u8; 32],
    pub connector_status_fp: [u8; 32],
    pub raw_blocks_fp: [u8; 32],
    pub send_tool_context_fp: [u8; 32],
    pub message_source_fp: [u8; 32],
    pub mode_tag: u8,
    pub model_window: u32,
}

/// Tagged cache value that pairs each output with its per-sub-field
/// fingerprints. Replaces the previous `GlobalCacheValue` shape.
#[derive(Debug, Clone)]
pub enum CachedEntry {
    Persona {
        output: Arc<PersonaOutput>,
        subs: PersonaSubFingerprints,
    },
    StaticPrompt {
        output: Arc<StaticPromptOutput>,
        subs: StaticPromptSubFingerprints,
    },
}

/// The compose engine. Holds the tier-1 global cache for Layer 1 (persona)
/// and Layer 2 (static prompt) outputs. Tier-2 per-lane cache lives on
/// `ConversationLane.caches` — this struct does not own those slots.
pub struct ComposeEngine {
    global_cache: Arc<Mutex<LruCache<GlobalCacheKey, Arc<CachedEntry>>>>,
}

impl std::fmt::Debug for ComposeEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposeEngine").finish_non_exhaustive()
    }
}

/// Result of a global-cache lookup. Carries both the `Arc`-shared output and
/// a `hit` bit the caller uses to (a) populate `LayerTrace::memo_hits` and
/// (b) decide whether to emit `ComposeLayerCacheHit` or `Miss`.
#[derive(Debug, Clone)]
pub struct CacheLookup<T> {
    pub output: Arc<T>,
    pub hit: bool,
    /// Populated when `hit == false` so the caller emits the right
    /// `ComposeLayerCacheMiss { reason, ... }` variant. On `hit == true`,
    /// this is always `None`.
    pub miss_reason: Option<MissReason>,
}

/// Compare two `PersonaSubFingerprints` and return the first differing field
/// as a `MissReason`. Priority order: persona_version → identity_budget →
/// user_budget → mode_tag. Returns `MissReason::Unknown` if all fields match
/// (defensive; should never fire if caller verified the composite fingerprints
/// differ).
fn attribute_persona_miss(
    old: &PersonaSubFingerprints,
    new: &PersonaSubFingerprints,
) -> MissReason {
    if old.persona_version != new.persona_version {
        return MissReason::PersonaChanged;
    }
    if old.identity_budget != new.identity_budget {
        return MissReason::IdentityBudgetChanged;
    }
    if old.user_budget != new.user_budget {
        return MissReason::UserBudgetChanged;
    }
    if old.mode_tag != new.mode_tag {
        return MissReason::ModeChanged;
    }
    MissReason::Unknown
}

/// Compare two `StaticPromptSubFingerprints` and return the first differing
/// field as a `MissReason`. Priority order follows declaration order; first
/// mismatch wins.
fn attribute_static_prompt_miss(
    old: &StaticPromptSubFingerprints,
    new: &StaticPromptSubFingerprints,
) -> MissReason {
    if old.persona_fp != new.persona_fp {
        return MissReason::PersonaChanged;
    }
    if old.agent_config_fp != new.agent_config_fp {
        return MissReason::AgentConfigChanged;
    }
    if old.agent_persona_fp != new.agent_persona_fp {
        return MissReason::AgentPersonaChanged;
    }
    if old.tools_fp != new.tools_fp {
        return MissReason::ToolsChanged;
    }
    if old.skills_catalog_fp != new.skills_catalog_fp {
        return MissReason::SkillsChanged;
    }
    if old.skill_block_fp != new.skill_block_fp {
        return MissReason::SkillBlockChanged;
    }
    if old.bootstrap_fp != new.bootstrap_fp {
        return MissReason::BootstrapChanged;
    }
    if old.connector_status_fp != new.connector_status_fp {
        return MissReason::ConnectorStatusChanged;
    }
    if old.raw_blocks_fp != new.raw_blocks_fp {
        return MissReason::RawBlocksChanged;
    }
    if old.send_tool_context_fp != new.send_tool_context_fp {
        return MissReason::SendToolContextChanged;
    }
    if old.message_source_fp != new.message_source_fp {
        return MissReason::MessageSourceChanged;
    }
    if old.mode_tag != new.mode_tag {
        return MissReason::ModeChanged;
    }
    if old.model_window != new.model_window {
        return MissReason::ModelWindowChanged;
    }
    MissReason::Unknown
}

impl ComposeEngine {
    pub fn new(global_cache_capacity: usize) -> Self {
        let capacity = NonZeroUsize::new(global_cache_capacity.max(1)).unwrap();
        Self {
            global_cache: Arc::new(Mutex::new(LruCache::new(capacity))),
        }
    }

    /// Access to the global cache. Phase 2 wires this up inside the layer
    /// computations; Phase 1 exposes it for tests and future inspection.
    pub fn global_cache(&self) -> &Arc<Mutex<LruCache<GlobalCacheKey, Arc<CachedEntry>>>> {
        &self.global_cache
    }

    /// Global-cache lookup for Layer 1 (Persona) outputs. Computes the cache
    /// key from the input's fingerprint; on hit, returns an `Arc`-clone of
    /// the cached output. On miss, runs `persona::compute` and inserts.
    ///
    /// On miss, diffs the new sub-fingerprints against the most-recently-used
    /// existing Persona entry to attribute the miss to a specific
    /// `MissReason`. Cold-cache misses (no prior entry) report
    /// `MissReason::FirstBuild`.
    ///
    /// `lane_id` is accepted for symmetry with the event-emission code path
    /// but is ignored here (the persona layer has no lane-scoped state).
    pub fn lookup_or_build_persona(
        &self,
        input: &PersonaInput,
        _lane_id: Option<&str>,
    ) -> CacheLookup<PersonaOutput> {
        let fingerprint = persona::compute_fingerprint(input);
        let new_subs = persona::compute_sub_fingerprints(input);
        let key = GlobalCacheKey::Persona(fingerprint);

        // Try hit path (separate lock scope from miss-path attribution scan).
        {
            let mut cache = self
                .global_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = cache.get(&key)
                && let CachedEntry::Persona { output, .. } = entry.as_ref()
            {
                return CacheLookup {
                    output: output.clone(),
                    hit: true,
                    miss_reason: None,
                };
            }
        }

        // Miss: attribute against the most-recently-used existing Persona
        // entry. `iter()` yields entries in LRU order (most-recently-used
        // first), so `find_map` picks the MRU Persona variant.
        let reason = {
            let cache = self
                .global_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cache
                .iter()
                .find_map(|(_k, v)| match v.as_ref() {
                    CachedEntry::Persona { subs, .. } => {
                        Some(attribute_persona_miss(subs, &new_subs))
                    }
                    _ => None,
                })
                .unwrap_or(MissReason::FirstBuild)
        };

        // Build + insert.
        let built = persona::compute(input);
        let arc = Arc::new(built);
        {
            let mut cache = self
                .global_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cache.put(
                key,
                Arc::new(CachedEntry::Persona {
                    output: arc.clone(),
                    subs: new_subs,
                }),
            );
        }

        CacheLookup {
            output: arc,
            hit: false,
            miss_reason: Some(reason),
        }
    }

    /// Global-cache lookup for Layer 2 (Static Prompt) outputs. Mirrors
    /// `lookup_or_build_persona` but keys against the static-prompt
    /// fingerprint variant. On miss, attribution diffs the new sub-fingerprints
    /// against the most-recently-used existing StaticPrompt entry.
    pub fn lookup_or_build_static_prompt(
        &self,
        input: &StaticPromptInput,
        _lane_id: Option<&str>,
    ) -> CacheLookup<StaticPromptOutput> {
        let fingerprint = static_prompt::compute_fingerprint(input);
        let new_subs = static_prompt::compute_sub_fingerprints(input);
        let key = GlobalCacheKey::StaticPrompt(fingerprint);

        {
            let mut cache = self
                .global_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = cache.get(&key)
                && let CachedEntry::StaticPrompt { output, .. } = entry.as_ref()
            {
                return CacheLookup {
                    output: output.clone(),
                    hit: true,
                    miss_reason: None,
                };
            }
        }

        let reason = {
            let cache = self
                .global_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cache
                .iter()
                .find_map(|(_k, v)| match v.as_ref() {
                    CachedEntry::StaticPrompt { subs, .. } => {
                        Some(attribute_static_prompt_miss(subs, &new_subs))
                    }
                    _ => None,
                })
                .unwrap_or(MissReason::FirstBuild)
        };

        let built = static_prompt::compute(input);
        let arc = Arc::new(built);
        {
            let mut cache = self
                .global_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cache.put(
                key,
                Arc::new(CachedEntry::StaticPrompt {
                    output: arc.clone(),
                    subs: new_subs,
                }),
            );
        }

        CacheLookup {
            output: arc,
            hit: false,
            miss_reason: Some(reason),
        }
    }

    /// Per-lane cache lookup for Layer 3 (Dynamic Context). Tier-2 cache lives
    /// on `ConversationLane.caches.dynamic_context` — a single-slot
    /// `Mutex<Option<(Fingerprint, Arc<DynamicContextOutput>)>>` that holds the
    /// most recent output for that lane. When `lane` is `None`, the layer is
    /// computed fresh each call and never cached.
    pub fn lookup_or_build_dynamic_context(
        &self,
        input: &DynamicContextInput,
        lane: Option<&crate::lane::ConversationLane>,
    ) -> CacheLookup<DynamicContextOutput> {
        let fingerprint = dynamic_context::compute_fingerprint(input);

        if let Some(lane) = lane {
            let guard = lane
                .caches
                .dynamic_context
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some((cached_fp, cached_arc)) = guard.as_ref()
                && *cached_fp == fingerprint
            {
                return CacheLookup {
                    output: cached_arc.clone(),
                    hit: true,
                    miss_reason: None,
                };
            }
        }

        let built = dynamic_context::compute(input);
        let arc = Arc::new(built);
        if let Some(lane) = lane {
            let mut guard = lane
                .caches
                .dynamic_context
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *guard = Some((fingerprint, arc.clone()));
        }

        // Per-lane cache attribution is out of scope for this cycle (spec
        // Future Phases). Report `FirstBuild` on every miss.
        CacheLookup {
            output: arc,
            hit: false,
            miss_reason: Some(MissReason::FirstBuild),
        }
    }

    /// Per-lane cache lookup for Layer 4 (Conversation History). Tier-2 cache
    /// lives on `ConversationLane.caches.history`. See
    /// `lookup_or_build_dynamic_context` for the flow shape — the two helpers
    /// are intentionally symmetric.
    pub fn lookup_or_build_history(
        &self,
        input: &HistoryInput,
        lane: Option<&crate::lane::ConversationLane>,
    ) -> CacheLookup<HistoryOutput> {
        let fingerprint = history::compute_fingerprint(input);

        if let Some(lane) = lane {
            let guard = lane
                .caches
                .history
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some((cached_fp, cached_arc)) = guard.as_ref()
                && *cached_fp == fingerprint
            {
                return CacheLookup {
                    output: cached_arc.clone(),
                    hit: true,
                    miss_reason: None,
                };
            }
        }

        let built = history::compute(input);
        let arc = Arc::new(built);
        if let Some(lane) = lane {
            let mut guard = lane
                .caches
                .history
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *guard = Some((fingerprint, arc.clone()));
        }

        // Per-lane cache attribution is out of scope for this cycle (spec
        // Future Phases). Report `FirstBuild` on every miss.
        CacheLookup {
            output: arc,
            hit: false,
            miss_reason: Some(MissReason::FirstBuild),
        }
    }

    /// Runs all five layers and returns a typed `ComposedRequest`.
    ///
    /// Phase 3: all four layer lookups are wired through their caches —
    /// Layers 1+2 via the global LRU, Layers 3+4 via the per-lane caches on
    /// `ConversationLane.caches`. Four `ComposeLayerCacheHit`/`Miss` events
    /// are emitted when `event_bus` is `Some` (one per layer).
    ///
    /// When `lane` is `None`, Layers 3+4 are computed fresh each call and
    /// never cached. Pre-existing tests from Phases 1+2 pass `None` for
    /// this arg.
    ///
    /// The `&ComposeRequest` argument is retained for the routing layer
    /// that Phase 4+ migrations will consume; today it is read only for the
    /// event `lane_id` field via `lane_key()`. The four layer inputs are
    /// passed in explicitly because the loader layer (a later phase) will
    /// build them from `ComposeRequest` + live orchestrator state.
    #[allow(clippy::too_many_arguments)]
    pub fn compose(
        &self,
        request: &ComposeRequest,
        persona_input: PersonaInput,
        static_prompt_input: StaticPromptInput,
        dynamic_context_input: DynamicContextInput,
        history_input: HistoryInput,
        model_window: u32,
        tools: Arc<Vec<openalpaca_llm::ToolDefinition>>,
        event_bus: Option<&crate::bus::EventBus>,
        lane: Option<&crate::lane::ConversationLane>,
    ) -> ComposedRequest {
        let lane_id = request.lane_key().map(|s| s.to_string());

        // Capture modes up-front (the inputs are partially moved below).
        let persona_mode = persona_input.mode;
        let static_prompt_mode = static_prompt_input.mode;
        let dynamic_context_mode = dynamic_context_input.mode;
        let history_mode = history_input.mode.clone();

        // Layer 1 — Persona.
        let persona_result = self.lookup_or_build_persona(&persona_input, lane_id.as_deref());
        emit_cache_event(
            event_bus,
            LayerId::Persona,
            persona_result.output.fingerprint,
            persona_result.hit,
            persona_result.miss_reason.unwrap_or(MissReason::FirstBuild),
            lane_id.clone(),
        );

        // Layer 2 — Static Prompt. Override the caller-supplied persona_output
        // with the lookup result (may be pointer-equal on cache hit, so this
        // is a no-op in the common case — but is necessary when the loader
        // didn't pre-populate it).
        let mut sp_input = static_prompt_input;
        sp_input.persona_output = persona_result.output.clone();
        let static_result = self.lookup_or_build_static_prompt(&sp_input, lane_id.as_deref());
        emit_cache_event(
            event_bus,
            LayerId::StaticPrompt,
            static_result.output.fingerprint,
            static_result.hit,
            static_result.miss_reason.unwrap_or(MissReason::FirstBuild),
            lane_id.clone(),
        );

        // Layer 3 — Dynamic Context (per-lane cache).
        let dynamic_result = self.lookup_or_build_dynamic_context(&dynamic_context_input, lane);
        emit_cache_event(
            event_bus,
            LayerId::DynamicContext,
            dynamic_result.output.fingerprint,
            dynamic_result.hit,
            dynamic_result.miss_reason.unwrap_or(MissReason::FirstBuild),
            lane_id.clone(),
        );

        // Layer 4 — Conversation History (per-lane cache).
        let history_result = self.lookup_or_build_history(&history_input, lane);
        emit_cache_event(
            event_bus,
            LayerId::History,
            history_result.output.fingerprint,
            history_result.hit,
            history_result.miss_reason.unwrap_or(MissReason::FirstBuild),
            lane_id.clone(),
        );

        let layer_trace = LayerTrace {
            persona_mode,
            static_prompt_mode,
            dynamic_context_mode,
            history_mode,
            memo_hits: LayerMemoHits {
                persona: persona_result.hit,
                static_prompt: static_result.hit,
                dynamic_context: dynamic_result.hit,
                history: history_result.hit,
            },
        };

        assembly::compose(assembly::AssemblyInput {
            persona: &persona_result.output,
            static_prompt: &static_result.output,
            dynamic_context: &dynamic_result.output,
            history: &history_result.output,
            tools,
            model_window,
            layer_trace,
        })
    }
}

/// Publish a `ComposeLayerCacheHit` / `Miss` event to the bus if one is
/// provided. No-op when `event_bus` is `None`.
fn emit_cache_event(
    event_bus: Option<&crate::bus::EventBus>,
    layer: LayerId,
    fingerprint: [u8; 32],
    hit: bool,
    reason: MissReason,
    lane_id: Option<String>,
) {
    let Some(bus) = event_bus else {
        return;
    };
    let timestamp = chrono::Utc::now();
    let event = if hit {
        crate::events::SystemEvent::ComposeLayerCacheHit {
            layer,
            fingerprint,
            lane_id,
            timestamp,
        }
    } else {
        crate::events::SystemEvent::ComposeLayerCacheMiss {
            layer,
            fingerprint,
            reason,
            lane_id,
            timestamp,
        }
    };
    let _ = bus.publish(event);
}

impl Default for ComposeEngine {
    fn default() -> Self {
        Self::new(256)
    }
}
