# Context Budget & Sub-Agent Distillation Design

**Date:** 2026-03-11
**Status:** Draft
**Scope:** Unified context window management for the orchestrator and sub-agent prompt distillation

---

## 1. Problem Statement

The current prompt and context management has several issues:

1. **Ad-hoc budgets**: Memory is hardcoded at 2000 chars, skills catalog at 500 chars. Identity and user profile have configurable defaults (300/1000 chars via `PromptBudgetsConfig`). None of these adapt to the model's context window or actual content sizes.
2. **Pipeline/DAG agents get thin prompts**: Sub-agents receive only a few-sentence persona + task description. No user preferences, no conversation context, no relevant memories.
3. **Lossy context compression**: The agentic loop's `compress_context()` (`runner/agentic_loop/context.rs`) preserves the system prompt, initial query, and recent N rounds, then replaces middle messages with a structured summary (tool calls + results). However, it does not extract durable information (user preferences, decisions, facts) into persistent memory before discarding — valuable context is permanently lost.
4. **Inconsistent context injection**: Simple queries get summary + memory + history, but pipeline agents get none of that. Skill invocations get conversation context but not skill-specific memories.
5. **No unified token accounting**: Each section renders independently with no awareness of total budget consumed.

## 2. Design Goals

- Treat the entire context window as a managed resource with explicit accounting
- Only messages are compressible — all other sections are fixed
- Compaction extracts durable information into memory before discarding
- Sub-agents receive minimum necessary context, distilled by the orchestrator
- Leverage Claude's `context_management` API for server-side tool/thinking clearing

## 3. Context Window Model

The context window has two zones plus an autocompact buffer.

### 3.1 Fixed Zone

Always present, non-compressible. Sections fill in priority order:

| Section | Source | Presence | Notes |
|---|---|---|---|
| System prompt | SOUL.md + Identity | Always | Cached with 30s TTL (existing) |
| User profile | USER.md | When available | Trusted system message |
| Tools | Tool registry | When tools active | API schema + guidance block |
| Agent catalog | Agent registry | Orchestrator only | Available agents summary |
| Skills catalog | Skill catalog | When skills loaded | Available skills summary |
| Memory retrieval | Hybrid FTS+vector | When relevant | Re-fetched per turn |

Each section renderer returns a `RenderedSection { content: String, token_estimate: usize }`. The `ContextBudgetManager` sums the fixed zone to know exactly how much free space remains.

### 3.2 Free Zone

Everything not in the fixed zone or autocompact buffer is available for messages:

```
free_zone = model_context_window - fixed_zone_tokens - autocompact_buffer
```

Messages grow into the free zone naturally. No pre-allocation, no waste.

### 3.3 Autocompact Buffer

A reserved buffer at the end of the context window. The LLM can generate output into this space during a turn (long tool-use chains, thinking), but if the **user's next input** would push the total past the buffer boundary, compaction triggers **before** the API call.

For a 200K model with `autocompact_buffer_ratio = 0.165`:
```
autocompact_buffer = 200K * 0.165 = ~33K tokens
compaction_trigger = model_context_window - autocompact_buffer
                   = 200K - 33K = 167K
```

The user never sees a "context full" error — compaction always fires preemptively.

### 3.4 Migration from `context_threshold`

The existing `LoopConfig` (`runner/agentic_loop/config.rs`) has two related fields:

- `max_context_tokens: u32` — hard cap set via `with_context_window(model_window)`
- `context_threshold: f64` — defaults to `0.6`, triggers `compress_context()` when estimated tokens exceed `model_window * 0.6`

The new `autocompact_buffer_ratio = 0.165` implies a trigger at 83.5% of the window — significantly later than the current 60%. This is intentional: the current 60% threshold is conservative because the heuristic compressor is lossy. The new 3-phase pipeline preserves more information, so the trigger can fire later.

**Migration:**

- **Phase A** (token accounting only): `LoopConfig.context_threshold` and `max_context_tokens` continue to work as-is. `ContextBudgetManager` computes but does not enforce.
- **Phase B** (compaction pipeline): `ContextBudgetManager` takes over compaction triggering. `LoopConfig.context_threshold` is **deprecated** — the autocompact buffer ratio replaces it. `LoopConfig.max_context_tokens` continues to be set by `with_context_window()` but is read from `ContextBudgetManager` instead.
- Note: `context_threshold` is a fill-level trigger (fire when filled to X%), while `autocompact_buffer_ratio` is a reserve-from-top fraction (keep X% empty). They are conceptual inverses: `context_threshold = 0.6` ≈ `autocompact_buffer_ratio = 0.4`.
- If both old and new thresholds are configured during migration, `ContextBudgetManager` wins (it fires first, at the autocompact boundary). The old `compress_context()` trigger becomes a safety net that should never fire.

### 3.5 Token Accounting

- Use actual token counts from API responses (`usage.input_tokens`) when available
- Pre-flight estimation: `tokens ~ bytes / 4` (existing heuristic), refined with per-section actuals after first API round
- Tools token estimate computed once at loop start (existing pattern)
- Integrate Claude's `count_tokens` API for accurate pre-flight when needed

### 3.6 Context Editing API Integration

For multi-round agentic loops, additionally leverage Claude's native `context_management` beta:

```json
{
  "context_management": {
    "edits": [
      {
        "type": "clear_thinking_20251015",
        "keep": { "type": "thinking_turns", "value": 2 }
      },
      {
        "type": "clear_tool_uses_20250919",
        "trigger": { "type": "input_tokens", "value": "<compaction_trigger>" },
        "keep": { "type": "tool_uses", "value": 5 }
      }
    ]
  }
}
```

This handles server-side tool/thinking bloat clearing. Our application-level `CompactionPipeline` handles message-level compaction client-side. Both work together.

## 4. Compaction Pipeline

When user input would breach the autocompact buffer, the 3-phase compaction pipeline runs before the API call.

### 4.1 Phase 1: Memory Extraction

Scan older messages (before the semantic boundary) for durable information worth persisting:

| Extract Type | Example | Storage |
|---|---|---|
| User preferences | "I prefer TypeScript over JavaScript" | Memory store (user_preference kind) |
| Decisions made | "We agreed to use PostgreSQL" | Memory store (decision kind) |
| Facts discovered | "The API rate limit is 100/min" | Memory store (fact kind) |
| Task outcomes | "Deployed v2.3 to staging" | Memory store (event kind) |

This is an LLM call using the compaction model (e.g., `claude-haiku-4-5-20251001`) — cheap and fast. Only the messages being compacted are processed, not the full context. Max `max_extractions_per_compaction` entries extracted per cycle.

Extracted entries are written to the persistent memory store and become retrievable in future turns via hybrid FTS+vector search.

### 4.2 Phase 2: Discard

Remove messages with near-zero information value:

- Pure social exchanges ("ok", "thanks", "got it")
- Acknowledgments without substance
- Redundant content already captured by Phase 1 extraction

Heuristic-first: the existing `is_social_message()` method lives on `IntentParser` (`orchestrator/intent/pre_screen.rs:166`) and checks against the `SOCIAL_PHRASES` constant. To make this reusable by `CompactionPipeline` (which lives outside the orchestrator module), extract `SOCIAL_PHRASES` and the matching logic into a shared utility function in `crate::utils::social` (or similar), then have both `IntentParser` and `CompactionPipeline` call it. The extraction LLM can optionally flag additional "low-value" segments in Phase 1's response.

### 4.3 Phase 3: Summarize at Semantic Boundaries

Remaining older messages are condensed into a structured summary. The summary respects semantic boundaries — it does not cut in the middle of a reasoning chain or task:

```
[Context Summary]
Topic: Implementing user authentication
- Explored OAuth2 vs JWT, chose JWT for simplicity
- Implemented token generation in auth/token.rs
- Tests passing for happy path, edge cases pending

Topic: Database schema discussion
- Added users table with email, password_hash, created_at
- Decided against soft deletes per user preference
```

The summary replaces the older messages in the context. Recent messages (after the semantic boundary) stay intact.

### 4.4 Compaction Target

Compact until total context is at ~50% of free zone capacity (`compaction_target_ratio = 0.50`), giving maximum breathing room before the next compaction cycle.

### 4.5 Compaction Failure Handling

Compaction Phases 1 and 3 require LLM calls (extraction and summarization). These can fail due to:

- Compaction model unavailable or rate-limited
- LLM response timeout
- Malformed LLM response (unparseable JSON in extraction)

**Fallback strategy:** If any compaction LLM call fails, fall back to the existing `compress_context()` function (`runner/agentic_loop/context.rs`), which uses heuristic compression without LLM calls. This preserves the system prompt, initial query, and recent rounds — it just skips memory extraction (Phase 1) and semantic summarization (Phase 3).

```
CompactionPipeline.compact()
  |
  +- Phase 1 (LLM extraction) -> on error: skip, log warning
  +- Phase 2 (heuristic discard) -> always succeeds (no LLM)
  +- Phase 3 (LLM summarization) -> on error: fall back to compress_context()
  |
  Result: either 3-phase compaction OR heuristic-only fallback
```

The compaction is never a hard failure — the conversation always continues, just with less optimal compression.

### 4.6 When NOT to Compact

- If the fixed zone alone exceeds 50% of the context window: this is a configuration problem (too many tools/skills loaded). Log a warning, do not compact.
- During the first few messages of a conversation: nothing to compact yet, let the buffer absorb naturally.

## 5. Sub-Agent Context Distillation

When the orchestrator dispatches to a pipeline/DAG agent, it assembles a `ContextPackage` — a minimal, curated context tailored to the specific sub-task. The orchestrator is the curator; sub-agents never see the raw conversation.

### 5.1 ContextPackage Structure

```rust
struct ContextPackage {
    // Always present
    task_description: String,

    // Orchestrator-distilled (included based on task relevance)
    conversation_summary: Option<String>,
    relevant_memories: Vec<MemoryEntry>,
    user_context: Option<UserContext>,
    workspace_artifacts: Vec<Artifact>,

    // Agent-declared constraints (from template)
    max_context_tokens: usize,
    denied_sections: Vec<String>,
}
```

### 5.2 Minimum Exposure Principle

The orchestrator decides what to include based on the task plan and agent template constraints:

| Section | Included When | Not Included |
|---|---|---|
| `task_description` | Always | — |
| `conversation_summary` | Task references conversation context | Pure computation tasks |
| `relevant_memories` | Memory store has entries relevant to the sub-task | No relevant results |
| `user_context` | Task produces user-facing output | Internal analysis tasks |
| `workspace_artifacts` | Pipeline step depends on prior step output | First step in pipeline |
| SOUL.md values | Never | Sub-agents have their own persona |
| Full message history | Never | Only the distilled summary |
| Tool definitions | Always (agent's skill-resolved tools) | — |

### 5.3 Distillation Flow

```
Orchestrator (full context)
|
+- Task Plan: "Agent A: analyze logs, Agent B: write fix"
|
+- For Agent A:
|   1. task_description = plan.agents[0].assignment
|   2. Query memory store: search("error logs deploy") -> relevant entries
|   3. conversation_summary = summarize(recent_messages, focus="error context")
|   4. user_context = None (not user-facing)
|   5. workspace_artifacts = [] (first in pipeline)
|   -> ContextPackage { task, memories, summary }
|
+- For Agent B:
    1. task_description = plan.agents[1].assignment
    2. Query memory store: search("code fix patterns") -> entries
    3. conversation_summary = None (Agent A output provides context)
    4. user_context = Some(prefs) (user will review the code)
    5. workspace_artifacts = [Agent A's analysis output]
    -> ContextPackage { task, memories, user_context, artifacts }
```

### 5.4 Sub-Agent Prompt Assembly

Sub-agents use the same fixed-zone + free-zone model, scoped to their own model and constraints:

```
Sub-Agent Context Window:
+---------------------------------------------+
| FIXED ZONE                                   |
|  Agent Persona (template ## Persona)         |
|  Assignment (task_description + role)         |
|  Tools (skill-resolved)                       |
|  [optional] User Context                      |
|  [optional] Relevant Memories                 |
|  [optional] Conversation Summary              |
+---------------------------------------------+
| FREE ZONE                                     |
|  Workspace artifacts (from prior steps)       |
|  Agent's own message history (multi-round)    |
+---------------------------------------------+
| AUTOCOMPACT BUFFER                            |
|  (scaled to agent's model context window)     |
+---------------------------------------------+
```

Sub-agents get their own `ContextBudgetManager` instance. If a sub-agent runs a multi-round loop, its own messages compact independently.

### 5.5 Security

Agent templates can declare `denied_sections` to enforce information barriers:
- Agents handling external output should NOT get internal system details
- Agents with restricted capabilities should NOT see tools they can't use
- Full raw conversation history is never passed — only distilled summary

This extends the existing `denied_capabilities` / `allowed_capabilities` pattern in `AgentConstraints` (`agent/subagent/mod.rs:103`). The struct diff:

```rust
pub struct AgentConstraints {
    // ... existing 10 fields (max_tool_calls, timeout_seconds, max_cost_per_task,
    //     max_rounds, require_confirmation_for, allowed_capabilities,
    //     denied_capabilities, allowed_models, denied_models, auto_approve)

    // NEW: context distillation constraints
    #[serde(default)]
    pub denied_sections: Vec<String>,       // e.g. ["conversation_summary", "user_context"]
    #[serde(default)]
    pub max_context_tokens: Option<usize>,  // overrides model default if set
}
```

The `normalize()` method already lowercases all list fields — `denied_sections` follows the same pattern.

**`max_context_tokens` derivation:** When not explicitly set on the agent template, defaults to the agent's model context window (from `ModelRegistry`) minus its autocompact buffer. When explicitly set, it caps the total context the orchestrator will assemble for that sub-agent, regardless of model capacity. This lets constrained agents (e.g., a summarizer that needs only 8K tokens) avoid wasting orchestrator time on unnecessary distillation.

## 6. Integration with Existing Architecture

### 6.1 New Components

**`ContextBudgetManager`** (`crates/openalpaca_core/src/context_budget/budget.rs`):
- Track token usage per section (fixed zone accounting)
- Compute free zone capacity for a given model
- Determine when compaction is needed
- Hold autocompact buffer size

**`CompactionPipeline`** (`crates/openalpaca_core/src/context_budget/compaction.rs`):
- Phase 1: Memory extraction via triage model LLM call
- Phase 2: Social message discard via heuristic + LLM flags
- Phase 3: Summarization at semantic boundaries via LLM call

**`ContextPackage`** (`crates/openalpaca_core/src/context_budget/package.rs`):
- Built by orchestrator in dispatcher
- Consumed by runner (agentic_loop, pipeline, dag_executor)

### 6.2 Changes to Existing Code

| File | Current | After |
|---|---|---|
| `query_handler/mod.rs` | `build_base_system_prompt()` returns String, hardcoded budgets | Returns `RenderedSection` with token count. `ContextBudgetManager` tracks totals |
| `query_handler/simple_query_handler.rs` | Message list with ad-hoc char limits | Sections render within budget from `ContextBudgetManager`. Compaction check before API call |
| `runner/agentic_loop/context.rs` | `compress_context()` preserves system/initial/recent, replaces middle with tool-call summaries | Replaced by `CompactionPipeline` for memory-extracting compaction. Existing `compress_context()` kept as fast fallback when compaction LLM is unavailable |
| `runner/agentic_loop/mod.rs` | Manual token tracking | Uses `ContextBudgetManager`. Adds `context_management` to API request |
| `dispatcher/pipeline_step.rs` | Thin prompt: persona + assignment + scope | Builds `ContextPackage`, assembles richer prompt |
| `dispatcher/dag.rs` + `runner/dag_executor/node_runner.rs` | Same thin prompt | Same `ContextPackage` treatment |
| `runner/lead_agent/prompt.rs` | Custom hardcoded prompt | Uses `ContextBudgetManager` for its own window |
| `middleware/prompt.rs` | Returns String | Returns `RenderedSection { content, token_estimate }` |
| `daemon_config/orchestrator.rs` | `PromptBudgetsConfig` with `identity_budget` (300 chars) + `user_profile_budget` (1000 chars) | Kept as-is for per-section char limits. New `ContextBudgetConfig` in `daemon_config/execution.rs` is **additive** — handles autocompact/compaction settings. In Phase A, `RenderedSection` token estimates replace char budgets for token accounting; the char limits remain as content-truncation guards |
| `middleware/prompt.rs` | `format_tool_guidance()` returns `String` | Returns `RenderedSection { content, token_estimate }`. **6 production call sites** need updating: `runner/lead_agent/mod.rs`, `runner/lead_agent/tools.rs`, `runner/dag_executor/node_runner.rs`, `dispatcher/pipeline_step.rs`, `orchestrator/skill/invocation.rs`, `query_handler/simple_query_handler.rs` (+ 3 tests in `middleware/prompt.rs`) |

### 6.3 What Stays the Same

- SOUL.md / IDENTITY.md / USER.md parsers (just report token sizes)
- Memory retrieval (hybrid FTS + vector)
- Tool registry and resolution
- Cached base prompt with 30s TTL (stores token count alongside rendered string)
- `wrap_untrusted_context()` for injection safety
- `LoopConfig` and `from_agent()` (ContextBudgetManager wraps it)

### 6.4 New Config Section

Deserialized as `ContextBudgetConfig` (new struct in `daemon_config/execution.rs`):

```toml
[execution.context]
# Autocompact buffer as fraction of model context window
autocompact_buffer_ratio = 0.165
# Compaction target: compress until free zone is this % utilized
compaction_target_ratio = 0.50
# Model for compaction LLM calls (extraction + summarization)
compaction_model = "claude-haiku-4-5-20251001"
# Max memories to extract per compaction cycle
max_extractions_per_compaction = 10
# Recent messages to always keep intact (minimum)
min_recent_messages = 4
```

### 6.5 Module Structure

> **Note:** Named `context_budget/` (not `context/`) because `crate::context` already exists and contains `SharedContext` (`context/shared/mod.rs`). The `context_budget` name is unambiguous and describes the module's purpose.

```
crates/openalpaca_core/src/context_budget/
  mod.rs              -- pub exports
  budget.rs           -- ContextBudgetManager, RenderedSection
  compaction.rs       -- CompactionPipeline (extract, discard, summarize)
  package.rs          -- ContextPackage (sub-agent distillation)
  tests.rs            -- Unit tests for budget math, compaction logic
```

### 6.6 Migration Path

Layered, shippable incrementally:

1. **Phase A**: `ContextBudgetManager` + `RenderedSection` — token accounting only, no behavior change. Sections report sizes.
2. **Phase B**: Autocompact buffer + `CompactionPipeline` — replaces heuristic-only `compress_context()` with 3-phase pipeline (memory extraction + discard + summarize). Existing compressor kept as fallback. Biggest behavioral change.
3. **Phase C**: `ContextPackage` + sub-agent distillation — enriches pipeline/DAG agent prompts. Independent of A/B.
4. **Phase D**: Claude `context_management` API integration — server-side tool/thinking clearing. Independent of A/B/C.

## 7. Observability

### 7.1 New SystemEvent Variants

| Event | When | Payload |
|---|---|---|
| `ContextBudgetComputed` | After fixed zone assembled | request_id, model, window_size, fixed_zone_tokens, free_zone_tokens, buffer_size, section_breakdown |
| `CompactionTriggered` | User input would breach buffer | request_id, utilization_pct, messages_before, messages_after, memories_extracted, messages_discarded, summary_tokens |
| `CompactionPhaseCompleted` | Each phase finishes | request_id, phase, duration_ms, items_processed |
| `ContextPackageBuilt` | Sub-agent dispatch | request_id, agent_id, sections_included, total_tokens, memories_count |

### 7.2 Telemetry Table

```sql
CREATE TABLE context_compaction_log (
    id INTEGER PRIMARY KEY,
    request_id TEXT NOT NULL,
    lane_key TEXT NOT NULL,
    trigger_utilization_pct REAL,
    messages_before INTEGER,
    messages_after INTEGER,
    memories_extracted INTEGER,
    messages_discarded INTEGER,
    summary_tokens INTEGER,
    extract_ms INTEGER,
    discard_ms INTEGER,
    summarize_ms INTEGER,
    total_ms INTEGER,
    compaction_model TEXT,
    timestamp TEXT DEFAULT (datetime('now'))
);
```

Compaction telemetry logging follows the existing `TelemetryConfig` pattern (`DaemonConfig.telemetry`). The `store_query_preview` flag gates whether message content appears in the log. Compaction logging is always enabled when the compaction pipeline runs (it's an operational concern, not opt-in analytics).

### 7.3 Logging

- `tracing::info!` on every compaction trigger (before/after token counts)
- `tracing::debug!` for per-section token breakdown per request
- `tracing::warn!` if fixed zone exceeds 50% of context window

## 8. Testing Strategy

### 8.1 Unit Tests

| Test | Verifies |
|---|---|
| `test_budget_computation_basic` | Fixed + free + buffer sum to model window |
| `test_budget_various_models` | Correct scaling for 8K, 128K, 200K windows |
| `test_compaction_trigger_threshold` | Triggers exactly when user input breaches buffer |
| `test_compaction_not_triggered_below_threshold` | No compaction within free zone |
| `test_section_token_reporting` | `RenderedSection` reports accurate estimate |
| `test_autocompact_buffer_ratio_config` | Buffer scales with ratio |
| `test_fixed_zone_overflow_warning` | Warning when fixed zone > 50% |
| `test_context_package_minimum_exposure` | Package includes only declared sections |
| `test_context_package_denied_sections` | Denied sections never in package |
| `test_context_package_always_has_task` | task_description always present |

### 8.2 Compaction Pipeline Tests (mock LLM)

| Test | Verifies |
|---|---|
| `test_extraction_stores_memories` | Phase 1 writes to memory store |
| `test_discard_removes_social` | Phase 2 removes social messages |
| `test_summarize_respects_semantic_boundary` | Phase 3 doesn't split mid-topic |
| `test_compaction_reaches_target` | Post-compaction utilization <= target |
| `test_compaction_preserves_recent` | Recent N messages survive intact |
| `test_compaction_roundtrip` | Compact -> continue -> compact again works |

### 8.3 Integration Tests

| Test | Verifies |
|---|---|
| `test_long_conversation_compacts_gracefully` | 50+ messages triggers compaction, continues |
| `test_sub_agent_receives_distilled_context` | Pipeline agent gets ContextPackage, not raw history |
| `test_extracted_memories_retrievable` | Compaction memories appear in future searches |
