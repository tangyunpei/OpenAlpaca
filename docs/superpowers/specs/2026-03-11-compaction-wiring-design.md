# Compaction Pipeline Production Wiring

**Date:** 2026-03-11
**Status:** Draft
**Scope:** Wire Phase B `CompactionPipeline` into the agentic loop's production path, improve heuristic fallback, wire `ContextBudgetManager` into sub-agent execution

**Depends on:**
- Phase A — `ContextBudgetManager` (token accounting, `RenderedSection`, budget computation)
- Phase B — `CompactionPipeline` types (`CompactionResult`, `MemoryExtractor`, `Summarizer` traits, `CompactionTriggered`/`CompactionPhaseCompleted` events)

---

## 1. Goal

Replace the heuristic `compress_context()` with LLM-based 3-phase compaction in the production agentic loop. Improve the heuristic fallback for cases where no compaction model is available. Wire `ContextBudgetManager` into `pipeline_step.rs` and `node_runner.rs` so sub-agents get the same budget-aware compaction as the main agent.

## 2. Design Decisions

### 2.1 Compaction Model

Use a dedicated (potentially cheaper/local) model for compaction via `ContextBudgetConfig.compaction_model` (field already exists, currently unused). This routes through the full `LlmRouter`, so users can configure any provider — `ollama/llama3` for local inference, `claude-haiku` for cheap cloud, etc.

Falls back to the default model if `compaction_model` is `None`.

### 2.2 Trait Implementations on LlmBackend

`LlmBackend` (in `agentic_loop/backend.rs`) implements both `MemoryExtractor` and `Summarizer` traits.

**Where `compaction_model` lives:**
- `compaction_model: Option<String>` is added to `LoopConfig` (Section 2.8).
- `compaction_model: Option<String>` is added to the `LlmBackend::Router` enum variant (not `LlmBackend::Direct` — direct mode has no router and cannot make compaction LLM calls).
- When constructing `LlmBackend::Router` in `mod.rs` (lines 143–158), populate `compaction_model` from `config.compaction_model.clone()`.
- The trait impls on `LlmBackend` access `self.compaction_model` on the `Router` variant to set the model override in `RouterRequest`. For `LlmBackend::Direct`, the trait methods return `Err(...)` (no compaction model available), which triggers the heuristic fallback.

**No signature change** to `run_agentic_loop_routed` — `LlmBackend` is constructed inside the function from existing parameters.

### 2.3 Extracted Memories: Log-Only

Extracted memories are emitted via `tracing::info!` (kind + first 100 chars of content). The count is reported in the `CompactionTriggered` event's `memories_extracted` field.

No DB storage, no context injection. This validates extraction quality first; persistence is wired separately once quality is confirmed.

### 2.4 Improved Heuristic `compress_context()`

When no LLM compressor is available (no `compaction_model`, or `LlmBackend::Direct` mode), the fallback heuristic is improved with the following changes:

1. **Social discard first** — call `CompactionPipeline::discard_social()` before building the summary. If this alone gets under budget, stop.
2. **Token-aware boundary** — walk backwards from the end counting tokens until `compaction_target_tokens` is reached; compress everything before that boundary. Replaces the fixed `tail_keep * 3` calculation.
3. **Include user messages** — fix the current bug where user content in the compressed range is silently dropped.
4. **Round-grouped summary format** — group by conversation round (user -> agent -> tool results) instead of flat list.
5. **Summary token cap** — truncate the summary if it exceeds the budget target.
6. **Add `budget: Option<&ContextBudgetManager>` parameter** — for token-aware boundary calculation. When `budget` is `Some`, use `budget.min_recent_messages()` for `min_recent`. When `budget` is `None`, use `config.context_tail_keep * 3` (legacy behavior preserved).
7. **Signature:** `compress_context(messages: &mut Vec<ChatMessage>, tail_keep: usize, budget: Option<&ContextBudgetManager>)`. Existing call sites pass `None` for backward compatibility.

### 2.5 Compaction Flow in the Loop

**Arc ownership:** Messages in the loop are `Arc<Vec<ChatMessage>>`. `CompactionPipeline::compact()` takes `Vec<ChatMessage>` by value. At the compaction call site:
1. Extract: `let owned = Arc::try_unwrap(messages).unwrap_or_else(|arc| (*arc).clone());`
2. Run: `let result = CompactionPipeline::compact(owned, min_recent, &backend, &backend).await;`
3. Reassign: `messages = Arc::new(result.compacted_messages);`

```
if budget.should_compact(msg_tokens):
    if backend supports LLM compaction (Router mode + compaction_model set):
        extract messages from Arc (try_unwrap or clone)
        result = CompactionPipeline::compact(msgs, budget.min_recent_messages(), &backend, &backend)
        emit CompactionTriggered + CompactionPhaseCompleted events
        log extracted memories (count + previews)
        reassign messages = Arc::new(result.compacted_messages)
    else:
        compress_context(Arc::make_mut(&mut messages), config.context_tail_keep, Some(budget))
```

**Fallback chain:**

| Failure | Behavior |
|---|---|
| LLM Phase 1 fails (extraction) | Skip extraction, continue to Phase 2/3 |
| LLM Phase 3 fails (summarization) | `CompactionPipeline` internally falls back to `compress_context()` (passes `None` for budget) |
| No `compaction_model` configured | Use improved heuristic directly |
| `LlmBackend::Direct` mode | Use improved heuristic directly (trait methods return Err) |

All paths preserve original behavior as a safety net. The conversation never fails due to compaction errors.

### 2.6 Sub-Agent Wiring

`ContextBudgetManager` is instantiated in `pipeline_step.rs` and `node_runner.rs`, then passed to `run_agentic_loop_routed()` (currently `None`).

**Model context window resolution:** `ContextBudgetManager::new()` requires `model_context_window: usize`. Resolve via:
```rust
let model_id = agent.llm_config.model.as_deref()
    .unwrap_or(router.default_model());
let context_window = router.model_registry()
    .get_model_info(model_id)
    .map(|info| info.context_window as usize)
    .unwrap_or(200_000);  // safe fallback
```

Both `pipeline_step.rs` and `node_runner.rs` already have `router` and `agent` available.

### 2.7 Telemetry

Reuse Phase B events — no new `SystemEvent` variants needed:

- `CompactionTriggered` — emitted when compaction runs (LLM or heuristic)
- `CompactionPhaseCompleted` — emitted per phase

Phase values: `"extraction"`, `"social_discard"`, `"summarization"`, `"heuristic_fallback"`.

**Heuristic-only field values:** When using the heuristic fallback (no LLM):
- `memories_extracted: 0`
- `summary_tokens`: estimated from the summary message length (`len / 4`)

### 2.8 LoopConfig Addition

Add `compaction_model: Option<String>` field to `LoopConfig`:

- Populated from `ContextBudgetConfig.compaction_model` in `LoopConfig::from_agent()` and `LoopConfig::from_lead_agent()`.
- Both delegate to `LoopConfig::from_defaults()` — add `compaction_model: Option<String>` parameter to `from_defaults()`, or set the field after construction in `from_agent`/`from_lead_agent`. Prefer the latter to minimize signature churn.
- **Manual `Clone` and `Debug` impls:** `LoopConfig` has hand-written `Clone` (lines 59–78) and `Debug` (lines 81–97) impls. Both must be updated to include `compaction_model`.

### 2.9 Cost Attribution

Compaction LLM calls (extraction + summarization) route through `LlmRouter::complete()`, which records usage via `cost_tracker.record_usage()`. These calls are attributed to the current agent/task — compaction token usage counts toward the loop's `max_cost` budget.

This is accepted for now. If compaction cost becomes significant (e.g. with expensive compaction models), a future change can introduce a separate `RequestContext` with a `"compaction:"` agent_id prefix to exclude compaction from the task budget.

## 3. Files Modified

No new files. All changes are to existing modules:

| File | Changes |
|---|---|
| `agentic_loop/backend.rs` | Add `compaction_model: Option<String>` to `LlmBackend::Router` variant. Implement `MemoryExtractor` and `Summarizer` traits on `LlmBackend`. `Direct` variant returns `Err` for both traits. |
| `agentic_loop/context.rs` | Rewrite `compress_context()` with token-aware heuristic: social discard, token boundary, user message inclusion, round-grouped summaries, summary cap. Add `budget: Option<&ContextBudgetManager>` parameter. |
| `agentic_loop/config.rs` | Add `compaction_model: Option<String>` to `LoopConfig`. Update manual `Clone` and `Debug` impls. |
| `agentic_loop/mod.rs` | Replace `compress_context()` call site with `CompactionPipeline::compact()` when compaction model is available. Handle `Arc` ownership for messages. Emit `CompactionTriggered` and `CompactionPhaseCompleted` events. Log extracted memories. Fall back to improved heuristic otherwise. |
| `context_budget/compaction.rs` | Types now used in production (dead code resolved). Update Phase 3 fallback call to pass `None` for `budget` parameter on `compress_context()`. |
| `dispatcher/pipeline_step.rs` | Instantiate `ContextBudgetManager` from model registry + `DaemonConfig`, pass to `run_agentic_loop_routed()`. |
| `dag_executor/node_runner.rs` | Same as `pipeline_step.rs`. |
| `daemon_config/execution.rs` | No changes needed — config fields already exist. |
