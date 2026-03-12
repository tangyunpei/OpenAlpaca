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

`LlmBackend` (in `agentic_loop/backend.rs`) implements both `MemoryExtractor` and `Summarizer` traits:

- Add `compaction_model: Option<String>` field to the `LlmBackend` enum (set during construction)
- Trait methods use the `compaction_model` override in `RouterRequest`, falling back to the default model
- No signature change to `run_agentic_loop_routed` — `LlmBackend` is already constructed inside the loop

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
6. **Add `budget: Option<&ContextBudgetManager>` parameter** — for token-aware boundary calculation. Falls back to legacy `tail_keep` behavior when `None`.

### 2.5 Compaction Flow in the Loop

```
if budget.should_compact(msg_tokens):
    if backend has compaction model (Router mode + compaction_model set):
        result = CompactionPipeline::compact(msgs, min_recent, &backend, &backend)
        emit CompactionTriggered + CompactionPhaseCompleted events
        log extracted memories (count + previews)
        replace messages with result.compacted_messages
    else:
        improved_compress_context(msgs, budget)  // token-aware heuristic
```

**Fallback chain:**

| Failure | Behavior |
|---|---|
| LLM Phase 1 fails (extraction) | Skip extraction, continue to Phase 2/3 |
| LLM Phase 3 fails (summarization) | Fall back to improved heuristic |
| No `compaction_model` configured | Use improved heuristic directly |

All paths preserve original behavior as a safety net. The conversation never fails due to compaction errors.

### 2.6 Sub-Agent Wiring

`ContextBudgetManager` is instantiated in `pipeline_step.rs` and `node_runner.rs` from `DaemonConfig`. It is passed to `run_agentic_loop_routed()` (currently `None`). Sub-agents get the same budget-aware compaction as the main agent.

### 2.7 Telemetry

Reuse Phase B events — no new `SystemEvent` variants needed:

- `CompactionTriggered` — emitted when compaction runs (LLM or heuristic)
- `CompactionPhaseCompleted` — emitted per phase

Phase values: `"extraction"`, `"social_discard"`, `"summarization"`, `"heuristic_fallback"`.

### 2.8 LoopConfig Addition

Add `compaction_model: Option<String>` field to `LoopConfig`:

- Populated from `ContextBudgetConfig.compaction_model` in `LoopConfig::from_agent()` and `LoopConfig::from_lead_agent()`
- Passed to `LlmBackend` during construction

## 3. Files Modified

No new files. All changes are to existing modules:

| File | Changes |
|---|---|
| `agentic_loop/backend.rs` | Add `compaction_model` field to `LlmBackend` enum. Implement `MemoryExtractor` and `Summarizer` traits. |
| `agentic_loop/context.rs` | Rewrite `compress_context()` with token-aware heuristic: social discard, token boundary, user message inclusion, round-grouped summaries, summary cap. Add `budget` parameter. |
| `agentic_loop/config.rs` | Add `compaction_model: Option<String>` to `LoopConfig`. |
| `agentic_loop/mod.rs` | Replace `compress_context()` call site with `CompactionPipeline::compact()` when compaction model is available. Emit `CompactionTriggered` and `CompactionPhaseCompleted` events. Log extracted memories. Fall back to improved heuristic otherwise. |
| `context_budget/compaction.rs` | Types now used in production (dead code resolved). |
| `dispatcher/pipeline_step.rs` | Instantiate `ContextBudgetManager` from `DaemonConfig`, pass to `run_agentic_loop_routed()`. |
| `dag_executor/node_runner.rs` | Same as `pipeline_step.rs`. |
| `daemon_config/execution.rs` | No changes needed — config fields already exist. |
