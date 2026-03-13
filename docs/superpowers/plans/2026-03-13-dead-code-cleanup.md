# Dead Code Cleanup — Approach A

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove false-positive `#[allow(dead_code)]` annotations, delete unused stub modules, and wire `CompactionResult` fields into the `CompactionTriggered` telemetry event.

**Architecture:** Three independent changes: (1) annotation cleanup, (2) stub module deletion, (3) telemetry wiring. The telemetry wiring connects the `CompactionResult` struct (which populates `messages_before`, `messages_after`, `extracted_memories`, `error`) to the `CompactionTriggered` event that already exists but is never emitted. The event bus is threaded into the agentic loop via an `event_bus` field on `LoopConfig`.

**Tech Stack:** Rust, tokio, EventBus (broadcast channel)

---

## Chunk 1: Annotation Cleanup + Stub Removal

### Task 1: Remove false-positive `#[allow(dead_code)]` annotations

These 4 sites suppress warnings on code that IS actively used. The compiler can't trace usage through serde deserialization or feature-gated builder methods.

**Files:**
- Modify: `crates/openalpaca_core/src/orchestrator/skill/handler.rs:13`
- Modify: `crates/openalpaca_connectors/src/telegram/delivery.rs:51`
- Modify: `crates/openalpaca_connectors/src/lib.rs:73,75`
- Modify: `apps/openalpacad/src/routes/settings_types.rs:27`

- [ ] **Step 1: Remove annotation from `SkillInvocationResult`**

In `crates/openalpaca_core/src/orchestrator/skill/handler.rs`, remove line 13:
```rust
// BEFORE (lines 12-14):
/// Result of a skill invocation, carrying LLM metadata alongside the output content.
#[allow(dead_code)]
pub(crate) struct SkillInvocationResult {

// AFTER:
/// Result of a skill invocation, carrying LLM metadata alongside the output content.
pub(crate) struct SkillInvocationResult {
```

- [ ] **Step 2: Remove annotation from `escape_markdown_v2`**

In `crates/openalpaca_connectors/src/telegram/delivery.rs`, remove line 51:
```rust
// BEFORE (lines 51-52):
#[allow(dead_code)]
pub(super) fn escape_markdown_v2(text: &str) -> String {

// AFTER:
pub(super) fn escape_markdown_v2(text: &str) -> String {
```

- [ ] **Step 3: Remove annotations from `ConnectorBuilder` fields**

In `crates/openalpaca_connectors/src/lib.rs`, remove lines 73 and 75:
```rust
// BEFORE (lines 72-76):
pub struct ConnectorBuilder {
    #[allow(dead_code)]
    db: Arc<Database>,
    #[allow(dead_code)]
    bus: Arc<EventBus>,

// AFTER:
pub struct ConnectorBuilder {
    db: Arc<Database>,
    bus: Arc<EventBus>,
```

- [ ] **Step 4: Remove annotation from `LlmUsageDailyQuery`**

In `apps/openalpacad/src/routes/settings_types.rs`, remove line 27:
```rust
// BEFORE (lines 26-28):
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct LlmUsageDailyQuery {

// AFTER:
#[derive(Debug, Deserialize)]
pub struct LlmUsageDailyQuery {
```

- [ ] **Step 5: Build to verify no new warnings**

Run: `cargo check --all-targets 2>&1 | grep "warning"`
Expected: No new `dead_code` warnings from the removed annotations (these are all actively used code).

- [ ] **Step 6: Commit**

```bash
git add crates/openalpaca_core/src/orchestrator/skill/handler.rs \
       crates/openalpaca_connectors/src/telegram/delivery.rs \
       crates/openalpaca_connectors/src/lib.rs \
       apps/openalpacad/src/routes/settings_types.rs
git commit -m "fix: remove false-positive #[allow(dead_code)] annotations

Four sites suppressed warnings on code that is actively used:
- SkillInvocationResult: consumed in handler.rs telemetry persistence
- escape_markdown_v2: called in telegram unit tests
- ConnectorBuilder.db/.bus: used in connector builder methods
- LlmUsageDailyQuery: deserialized via axum Query extractor"
```

---

### Task 2: Delete unused stub tool modules

The `summarize` and `text_generate` modules are stubs that always return "not implemented". They are never registered in `builtin_tools()` or `builtin_tools_with_persona_context()`.

**Files:**
- Delete: `crates/openalpaca_core/src/tools/builtins/summarize.rs`
- Delete: `crates/openalpaca_core/src/tools/builtins/text_generate.rs`
- Modify: `crates/openalpaca_core/src/tools/builtins/mod.rs:6-11`

- [ ] **Step 1: Remove module declarations from `mod.rs`**

In `crates/openalpaca_core/src/tools/builtins/mod.rs`, remove lines 6-11:
```rust
// REMOVE these 6 lines:
// Stub tools — not registered (always returned "not implemented").
// Kept for potential future implementation.
#[allow(dead_code)]
mod summarize;
#[allow(dead_code)]
mod text_generate;
```

- [ ] **Step 2: Delete the stub files**

```bash
rm crates/openalpaca_core/src/tools/builtins/summarize.rs
rm crates/openalpaca_core/src/tools/builtins/text_generate.rs
```

- [ ] **Step 3: Build and run existing tests**

Run: `cargo test -p openalpaca_core -- tools::builtins`
Expected: All existing builtin tool tests pass. The deleted stub tests (`test_summarize_tool`, `test_text_generate_tool`) are gone.

- [ ] **Step 4: Commit**

```bash
git add -A crates/openalpaca_core/src/tools/builtins/
git commit -m "refactor: remove unused summarize and text_generate stub tools

These modules were never registered in builtin_tools() and always
returned 'not implemented'. Git history preserves them if needed."
```

---

## Chunk 2: Wire CompactionResult into CompactionTriggered Telemetry

### Task 3: Extend `CompactionReport` with telemetry fields

The `GraduatedCompactor` returns a `CompactionReport` but it lacks data from the `CompactionResult` produced by the `LlmSummary` tier. Extend `CompactionReport` to carry `memories_extracted`, `messages_discarded`, `messages_before`, `messages_after`, and `compaction_error`.

**Files:**
- Modify: `crates/openalpaca_core/src/prompt_ctx/compaction/graduated.rs`
- Modify: `crates/openalpaca_core/src/context_budget/compaction.rs:30-38`
- Test: `crates/openalpaca_core/src/prompt_ctx/compaction/graduated.rs` (existing tests + new test)

- [ ] **Step 1: Write failing test for CompactionReport carrying CompactionResult data**

Add to `crates/openalpaca_core/src/prompt_ctx/compaction/graduated.rs` tests module:

```rust
#[tokio::test]
async fn test_compaction_report_carries_result_data() {
    use crate::context_budget::compaction::{ExtractedMemory, MemoryExtractor, Summarizer};
    use crate::context_budget::{ContextBudgetManager, CompactionTier};
    use crate::daemon_config::ContextBudgetConfig;
    use openalpaca_llm::ChatMessage;

    struct TestExtractor;
    #[async_trait::async_trait]
    impl MemoryExtractor for TestExtractor {
        async fn extract(&self, _: &[ChatMessage]) -> Result<Vec<ExtractedMemory>, String> {
            Ok(vec![ExtractedMemory {
                kind: "fact".into(),
                content: "test memory".into(),
            }])
        }
    }

    struct TestSummarizer;
    #[async_trait::async_trait]
    impl Summarizer for TestSummarizer {
        async fn summarize(&self, _: &[ChatMessage]) -> Result<String, String> {
            Ok("[Summary]".into())
        }
    }

    // Build a budget that triggers LlmSummary tier
    let cfg = ContextBudgetConfig {
        autocompact_buffer_ratio: 0.15,
        ..ContextBudgetConfig::default()
    };
    let budget = ContextBudgetManager::new(100, &cfg); // tiny window to force compaction

    // Build enough messages to trigger LlmSummary
    let mut messages = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("initial"),
    ];
    for i in 0..20 {
        messages.push(ChatMessage::user(&format!("msg {i}")));
        messages.push(ChatMessage::assistant(&format!("resp {i}")));
    }
    messages.push(ChatMessage::user("recent"));
    messages.push(ChatMessage::assistant("recent resp"));

    let compactor = GraduatedCompactor::new(&budget, &TestExtractor, &TestSummarizer);
    let report = compactor.compact(&mut messages, 2).await;

    assert!(report.memories_extracted > 0, "should carry extracted memory count");
    assert!(report.messages_before > 0, "should carry messages_before");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p openalpaca_core -- compaction::graduated::tests::test_compaction_report_carries_result_data`
Expected: FAIL — `CompactionReport` doesn't have `memories_extracted`, `messages_before` fields.

- [ ] **Step 3: Extend `CompactionReport` struct**

In `crates/openalpaca_core/src/prompt_ctx/compaction/graduated.rs`, update the struct:

```rust
/// Report of compaction actions taken.
#[derive(Debug, Default)]
pub struct CompactionReport {
    pub tiers_applied: Vec<CompactionTier>,
    pub initial_tokens: usize,
    pub final_tokens: usize,
    /// Number of memories extracted during LlmSummary phase.
    pub memories_extracted: usize,
    /// Number of messages discarded during compaction.
    pub messages_discarded: usize,
    /// Message count before compaction started.
    pub messages_before: usize,
    /// Message count after compaction completed.
    pub messages_after: usize,
    /// Error from LlmSummary phase (None = success or phase not reached).
    pub compaction_error: Option<String>,
}
```

- [ ] **Step 4: Populate new fields in `GraduatedCompactor::compact()`**

Two locations need changes:

**4a. Set `messages_before` in the initial `CompactionReport` literal (before the loop):**

```rust
// BEFORE (lines 39-42):
let mut report = CompactionReport {
    initial_tokens: crate::runner::estimate_messages_tokens(messages) as usize,
    ..Default::default()
};

// AFTER:
let mut report = CompactionReport {
    initial_tokens: crate::runner::estimate_messages_tokens(messages) as usize,
    messages_before: messages.len(),
    ..Default::default()
};
```

**4b. Populate LlmSummary data in the `CompactionTier::LlmSummary` arm (lines 66-78):**

```rust
// BEFORE:
CompactionTier::LlmSummary => {
    let min_recent = self.budget.min_recent_messages();
    let owned = std::mem::take(messages);
    let result = crate::context_budget::compaction::CompactionPipeline::compact(
        owned,
        min_recent,
        self.extractor,
        self.summarizer,
    )
    .await;
    *messages = result.compacted_messages;
}

// AFTER:
CompactionTier::LlmSummary => {
    let min_recent = self.budget.min_recent_messages();
    let owned = std::mem::take(messages);
    let result = crate::context_budget::compaction::CompactionPipeline::compact(
        owned,
        min_recent,
        self.extractor,
        self.summarizer,
    )
    .await;
    report.memories_extracted = result.extracted_memories.len();
    report.messages_discarded += result.messages_discarded;
    report.compaction_error = result.error.clone();
    *messages = result.compacted_messages;
}
```

**4c. Set `messages_after` at the end of `compact()` (before returning):**

```rust
// BEFORE (lines 99-101):
report.final_tokens =
    crate::runner::estimate_messages_tokens(messages) as usize;
report

// AFTER:
report.final_tokens =
    crate::runner::estimate_messages_tokens(messages) as usize;
report.messages_after = messages.len();
report
```

- [ ] **Step 5: Remove `#[allow(dead_code)]` from `CompactionResult` fields**

In `crates/openalpaca_core/src/context_budget/compaction.rs`, the `messages_before` and `messages_after` fields are now read by `GraduatedCompactor`. Remove their annotations (lines 34-37):

```rust
// BEFORE:
pub struct CompactionResult {
    pub compacted_messages: Vec<ChatMessage>,
    pub extracted_memories: Vec<ExtractedMemory>,
    pub messages_discarded: usize,
    #[allow(dead_code)]
    pub messages_before: usize,
    #[allow(dead_code)]
    pub messages_after: usize,
    pub error: Option<String>,
}

// AFTER:
pub struct CompactionResult {
    pub compacted_messages: Vec<ChatMessage>,
    pub extracted_memories: Vec<ExtractedMemory>,
    pub messages_discarded: usize,
    pub messages_before: usize,
    pub messages_after: usize,
    pub error: Option<String>,
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p openalpaca_core -- compaction::graduated::tests::test_compaction_report_carries_result_data`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/openalpaca_core/src/prompt_ctx/compaction/graduated.rs \
       crates/openalpaca_core/src/context_budget/compaction.rs
git commit -m "feat: extend CompactionReport with telemetry fields from CompactionResult

CompactionReport now carries memories_extracted, messages_discarded,
messages_before, messages_after, and compaction_error from the
LlmSummary phase. Removes #[allow(dead_code)] from CompactionResult
fields that are now consumed."
```

---

### Task 4: Add `event_bus` to `LoopConfig` and emit `CompactionTriggered`

Thread the `EventBus` into the agentic loop via `LoopConfig` so compaction can emit telemetry. This approach avoids changing the function signatures of `run_agentic_loop`, `run_agentic_loop_routed`, and `run_agentic_loop_inner` (which have 8+14 = 22 call sites).

**Key insight:** All `LoopConfig` struct literal sites use `..Default::default()` or `..self.loop_config.clone()`, so adding a new field with `Default` → `None` propagates automatically. Only callers that want to SET the bus need changes.

**Files:**
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/config.rs:17-125` (struct + Clone + Debug + Default)
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/mod.rs:280-315` (emit event)
- Modify: `crates/openalpaca_core/src/orchestrator/mod.rs:281` (set bus on loop_config)
- Modify: `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs` (set bus on loop_config)
- Modify: `crates/openalpaca_core/src/runner/dag_executor/node_runner.rs` (set bus on loop_config)
- Modify: `crates/openalpaca_core/src/runner/lead_agent/mod.rs` (set bus on loop_config)
- Modify: `crates/openalpaca_core/src/runner/lead_agent/tools.rs` (set bus on loop_config)
- Modify: `crates/openalpaca_core/src/orchestrator/skill/invoke_executor.rs` (set bus on loop_config)

**NOT modified** (inherit `None` via `..Default::default()` or `..self.loop_config.clone()`):
- `runner/agentic_loop/tests.rs` — 14 call sites, all use `..Default::default()`, get `event_bus: None` automatically
- `orchestrator/query_handler/simple_query_handler.rs` — 2 call sites use `..self.loop_config.clone()`, inherit the bus from `Orchestrator.loop_config`
- `orchestrator/skill/invocation.rs` — 1 call site uses `..self.loop_config.clone()`, inherits the bus

- [ ] **Step 1: Add `event_bus` field to `LoopConfig`**

In `crates/openalpaca_core/src/runner/agentic_loop/config.rs`:

**1a. Add import at top:**
```rust
use crate::bus::EventBus;
```

**1b. Add field to struct (line ~59, after `compaction_model`):**
```rust
    /// Optional event bus for emitting compaction telemetry.
    /// When set, the agentic loop publishes `CompactionTriggered` events.
    pub event_bus: Option<EventBus>,
```

**1c. Add to manual `Clone` impl (line ~80, after `compaction_model`):**
```rust
            event_bus: self.event_bus.clone(),
```

**1d. Add to manual `Debug` impl (line ~99, after `compaction_model`):**
```rust
            .field("event_bus", &self.event_bus.is_some())
```

**1e. Add to `Default` impl (line ~122, after `compaction_model`):**
```rust
            event_bus: None,
```

**1f. Add to `from_defaults` struct literal (line ~180, after `compaction_model: None,`):**
```rust
            event_bus: None,
```
This literal enumerates every field explicitly (no `..Default::default()`), so it will fail to compile without this addition.

- [ ] **Step 2: Emit `CompactionTriggered` from `run_agentic_loop_inner`**

In `crates/openalpaca_core/src/runner/agentic_loop/mod.rs`, after the existing `tracing::info!` block for "Graduated compaction completed" (around line 312), add:

```rust
                // Emit CompactionTriggered telemetry
                if let Some(ref bus) = config.event_bus {
                    bus.publish(crate::events::SystemEvent::CompactionTriggered {
                        request_id: uuid::Uuid::new_v4(),
                        utilization_pct: report.initial_tokens as f64
                            / budget.model_context_window() as f64
                            * 100.0,
                        messages_before: report.messages_before,
                        messages_after: report.messages_after,
                        memories_extracted: report.memories_extracted,
                        messages_discarded: report.messages_discarded,
                        summary_tokens: report.initial_tokens
                            .saturating_sub(report.final_tokens),
                        timestamp: chrono::Utc::now(),
                    });
                }
```

Note: neither `uuid` nor `chrono` is imported in this file. Add both imports near the top:
```rust
use chrono::Utc;
use uuid::Uuid;
```

- [ ] **Step 3: Set `event_bus` on `Orchestrator.loop_config`**

In `crates/openalpaca_core/src/orchestrator/mod.rs`, after the existing `loop_config` is stored (line 281), set the bus. The simplest approach: set it on the config before storing:

```rust
// BEFORE (around line 281):
            loop_config,

// AFTER:
            loop_config: {
                let mut lc = loop_config;
                lc.event_bus = Some(bus.clone());
                lc
            },
```

This ensures all `self.loop_config.clone()` calls in query handlers and skill invocation inherit the bus.

- [ ] **Step 4: Set `event_bus` on LoopConfig at 4 non-orchestrator call sites**

These callers construct `LoopConfig` via `LoopConfig::from_agent()` / `LoopConfig::from_lead_agent()` which defaults `event_bus` to `None`. They need to set it explicitly:

**4a. `pipeline_step.rs`** — has `bus: EventBus` parameter. After `loop_config` construction (around line 187-196):
```rust
loop_config.event_bus = Some(bus.clone());
```

**4b. `node_runner.rs`** — has `bus: EventBus` parameter. After `loop_config` construction (around line 42-48):
```rust
loop_config.event_bus = Some(bus.clone());
```

**4c. `lead_agent/mod.rs`** — has `bus: EventBus` parameter. After `loop_config` construction (around line 299-302):
```rust
loop_config.event_bus = Some(bus.clone());
```

**4d. `lead_agent/tools.rs`** — has `self.bus: EventBus` field. After `LoopConfig::from_agent()` call (around line 247):
```rust
loop_config.event_bus = Some(self.bus.clone());
```

**4e. `skill/invoke_executor.rs`** — has `self.bus: EventBus` field. After `LoopConfig { ... }` literal (around line 218-221):
```rust
// The literal uses ..LoopConfig::default(), so event_bus is None.
// Set it after construction:
config.event_bus = Some(self.bus.clone());
```
Note: the `config` binding may need `let mut config = ...`.

- [ ] **Step 5: Build and run all tests**

```bash
cargo check --all-targets
cargo test -p openalpaca_core
```

Expected: All tests pass. The 14 test call sites use `..Default::default()` so they automatically get `event_bus: None` — no test changes needed.

- [ ] **Step 6: Commit**

```bash
git add crates/openalpaca_core/src/runner/agentic_loop/config.rs \
       crates/openalpaca_core/src/runner/agentic_loop/mod.rs \
       crates/openalpaca_core/src/orchestrator/mod.rs \
       crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs \
       crates/openalpaca_core/src/runner/dag_executor/node_runner.rs \
       crates/openalpaca_core/src/runner/lead_agent/mod.rs \
       crates/openalpaca_core/src/runner/lead_agent/tools.rs \
       crates/openalpaca_core/src/orchestrator/skill/invoke_executor.rs
git commit -m "feat: emit CompactionTriggered event from agentic loop

The CompactionTriggered event was defined but never published.
Add event_bus field to LoopConfig so the agentic loop can emit
CompactionTriggered after graduated compaction completes, carrying
utilization percentage, message counts, memory extraction count,
and summary token delta."
```

---

## Final Verification

- [ ] **Full workspace build**: `cargo check --all-targets`
- [ ] **All tests pass**: `cargo test`
- [ ] **No new warnings**: `cargo clippy --all-targets 2>&1 | grep "dead_code"` should return nothing
- [ ] **Only pre-existing warnings remain**: `cargo clippy --all-targets 2>&1 | grep "warning"` count should be ≤ previous count minus the 1 dead_code warning we fixed
