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
mod persona;
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

#[derive(Debug, Clone)]
pub enum GlobalCacheValue {
    Persona(Arc<PersonaOutput>),
    StaticPrompt(Arc<StaticPromptOutput>),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissReason {
    FirstBuild,
    PersonaChanged,
    AgentConfigChanged,
    ToolsChanged,
    SkillsChanged,
    BootstrapChanged,
    QueryChanged,
    MemoryChanged,
    LaneTipAdvanced,
}

/// The compose engine. Holds the tier-1 global cache for Layer 1 (persona)
/// and Layer 2 (static prompt) outputs. Tier-2 per-lane cache lives on
/// `ConversationLane.caches` — this struct does not own those slots.
pub struct ComposeEngine {
    global_cache: Arc<Mutex<LruCache<GlobalCacheKey, Arc<GlobalCacheValue>>>>,
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
    pub fn global_cache(&self) -> &Arc<Mutex<LruCache<GlobalCacheKey, Arc<GlobalCacheValue>>>> {
        &self.global_cache
    }

    /// Global-cache lookup for Layer 1 (Persona) outputs. Computes the cache
    /// key from the input's fingerprint; on hit, returns an `Arc`-clone of
    /// the cached output. On miss, runs `persona::compute` and inserts.
    ///
    /// `lane_id` is accepted for symmetry with the event-emission code path
    /// but is ignored here (the persona layer has no lane-scoped state).
    pub fn lookup_or_build_persona(
        &self,
        input: &PersonaInput,
        _lane_id: Option<&str>,
    ) -> CacheLookup<PersonaOutput> {
        let fingerprint = persona::compute_fingerprint(input);
        let key = GlobalCacheKey::Persona(fingerprint);

        {
            let mut cache = self
                .global_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(value) = cache.get(&key)
                && let GlobalCacheValue::Persona(arc) = value.as_ref()
            {
                return CacheLookup {
                    output: arc.clone(),
                    hit: true,
                };
            }
        }

        // Miss: compute and insert.
        let built = persona::compute(input);
        let arc = Arc::new(built);
        {
            let mut cache = self
                .global_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cache.put(key, Arc::new(GlobalCacheValue::Persona(arc.clone())));
        }

        CacheLookup {
            output: arc,
            hit: false,
        }
    }

    /// Phase 1 stub: runs all five layers with stub outputs and returns a
    /// valid `ComposedRequest`. Phase 2 and Phase 3 replace the layer calls
    /// with real work plus cache plumbing.
    ///
    /// The `&ComposeRequest` argument is retained (unused in Phase 1) so the
    /// signature is stable once Phase 3 adds routing logic that consults the
    /// variant. The four layer inputs are passed in explicitly because the
    /// loader layer (a later phase) will build them from `ComposeRequest` +
    /// live orchestrator state.
    #[allow(clippy::too_many_arguments)]
    pub fn compose(
        &self,
        _request: &ComposeRequest,
        persona_input: PersonaInput,
        static_prompt_input: StaticPromptInput,
        dynamic_context_input: DynamicContextInput,
        history_input: HistoryInput,
        model_window: u32,
        tools: Arc<Vec<openalpaca_llm::ToolDefinition>>,
    ) -> ComposedRequest {
        let persona_out = persona::compute(&persona_input);
        let static_out = static_prompt::compute(&static_prompt_input);
        let dyn_out = dynamic_context::compute(&dynamic_context_input);
        let hist_out = history::compute(&history_input);

        let layer_trace = LayerTrace {
            persona_mode: persona_input.mode,
            static_prompt_mode: static_prompt_input.mode,
            dynamic_context_mode: dynamic_context_input.mode,
            history_mode: history_input.mode.clone(),
            memo_hits: LayerMemoHits::default(),
        };

        assembly::compose(assembly::AssemblyInput {
            persona: &persona_out,
            static_prompt: &static_out,
            dynamic_context: &dyn_out,
            history: &hist_out,
            tools,
            model_window,
            layer_trace,
        })
    }
}

impl Default for ComposeEngine {
    fn default() -> Self {
        Self::new(256)
    }
}
