# Context Budget Phase B: Compaction Pipeline

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace heuristic-only `compress_context()` with a 3-phase compaction pipeline (memory extraction + social discard + semantic summarization). Existing compressor kept as fallback.

**Architecture:** `CompactionPipeline` in `context_budget/compaction.rs` with trait-based LLM abstraction (`Summarizer`, `MemoryExtractor`) for testability. Social phrase detection extracted to shared `utils::social` module. Wired into agentic loop via `ContextBudgetManager` parameter.

**Tech Stack:** Rust, async_trait (already in openalpaca_core deps), openalpaca_llm ChatMessage, tokio.

**Spec:** `docs/superpowers/specs/2026-03-11-context-budget-design.md` — Sections 4.1–4.6, 6.2, 7.1–7.2.

**Depends on:** Phase A (context_budget module, ContextBudgetManager, ContextBudgetConfig must exist).

---

## File Structure

| Action | Path | Purpose |
|--------|------|---------|
| Create | `crates/openalpaca_core/src/utils/mod.rs` | New utils module |
| Create | `crates/openalpaca_core/src/utils/social.rs` | Shared `SOCIAL_PHRASES` + `is_social_phrase()` |
| Create | `crates/openalpaca_core/src/context_budget/compaction.rs` | 3-phase `CompactionPipeline` |
| Create | `crates/openalpaca_storage/src/migrations/032_context_compaction_log.sql` | Telemetry table |
| Modify | `crates/openalpaca_core/src/lib.rs` | Add `pub mod utils;` |
| Modify | `crates/openalpaca_core/src/orchestrator/intent/pre_screen.rs` | Delegate to shared social util |
| Modify | `crates/openalpaca_core/src/context_budget/mod.rs` | Add compaction submodule |
| Modify | `crates/openalpaca_core/src/context_budget/tests.rs` | Compaction tests |
| Modify | `crates/openalpaca_core/src/runner/agentic_loop/context.rs` | Change `compress_context` to `pub(crate)` |
| Modify | `crates/openalpaca_core/src/runner/mod.rs` | Re-export `compress_context` as `pub(crate)` |
| Modify | `crates/openalpaca_core/src/runner/agentic_loop/mod.rs` | Add `context_budget` param to loop |
| Modify | `crates/openalpaca_core/src/events.rs` | Add compaction event variants |
| Modify | `apps/openalpacad/src/event_bridge.rs` | Add match arms for new events |
| Modify | All call sites of `run_agentic_loop`/`run_agentic_loop_routed` | Add `None` param |

---

### Task 1: Extract `SOCIAL_PHRASES` to shared utility

**Files:**
- Create: `crates/openalpaca_core/src/utils/mod.rs`
- Create: `crates/openalpaca_core/src/utils/social.rs`
- Modify: `crates/openalpaca_core/src/lib.rs`
- Modify: `crates/openalpaca_core/src/orchestrator/intent/pre_screen.rs`

- [ ] **Step 1: Create shared utility with tests**

```rust
// crates/openalpaca_core/src/utils/social.rs

/// Short social/acknowledgement phrases that never require planning or tool use.
pub const SOCIAL_PHRASES: &[&str] = &[
    "thanks", "thank you", "ok", "okay", "got it", "sounds good",
    "yes", "no", "sure", "right",
    "好的", "没问题", "谢谢", "嗯", "明白", "收到", "对", "是的", "不是", "不用",
];

/// Check if a message is a pure social/acknowledgement phrase.
pub fn is_social_phrase(content: &str) -> bool {
    let trimmed = content.trim().to_lowercase();
    SOCIAL_PHRASES.contains(&trimmed.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_social_phrases() {
        assert!(is_social_phrase("thanks"));
        assert!(is_social_phrase("  OK  "));
        assert!(is_social_phrase("好的"));
        assert!(!is_social_phrase("write me a function"));
        assert!(!is_social_phrase(""));
    }
}
```

```rust
// crates/openalpaca_core/src/utils/mod.rs
pub mod social;
```

Add to `lib.rs`: `pub mod utils;`

- [ ] **Step 2: Update `pre_screen.rs` to delegate**

In `crates/openalpaca_core/src/orchestrator/intent/pre_screen.rs`:
- Remove `pub(super) const SOCIAL_PHRASES` (lines 8-12)
- Add import: `use crate::utils::social::{SOCIAL_PHRASES, is_social_phrase};`
- Update `is_social_message()` (line 166) body to: `is_social_phrase(content)`
- Update `is_enhanced_simple_query()` at line ~80 where `SOCIAL_PHRASES.contains(&trimmed_lower)` is used — this still works because `SOCIAL_PHRASES` is now imported. Verify the `trimmed_lower` variable is already `&str` so the `.contains()` call compiles.

- [ ] **Step 3: Verify build + tests**

Run: `cargo test -p openalpaca_core -- pre_screen --nocapture && cargo test -p openalpaca_core -- utils::social --nocapture`

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/utils/ crates/openalpaca_core/src/lib.rs crates/openalpaca_core/src/orchestrator/intent/pre_screen.rs
git commit -m "refactor: extract SOCIAL_PHRASES to shared utils::social (Phase B prep)"
```

---

### Task 2: Implement `CompactionPipeline` Phase 2 — heuristic discard

**Files:**
- Create: `crates/openalpaca_core/src/context_budget/compaction.rs`
- Modify: `crates/openalpaca_core/src/context_budget/mod.rs`
- Modify: `crates/openalpaca_core/src/context_budget/tests.rs`

- [ ] **Step 1: Write failing tests**

```rust
// Add to tests.rs
use super::compaction::CompactionPipeline;
use openalpaca_llm::ChatMessage;

#[test]
fn test_discard_removes_social() {
    let messages = vec![
        ChatMessage::system("system prompt"),
        ChatMessage::user("initial query"),
        ChatMessage::user("thanks"),
        ChatMessage::assistant("You're welcome!"),
        ChatMessage::user("ok"),
        ChatMessage::assistant("Anything else?"),
        ChatMessage::user("What's the weather?"),
        ChatMessage::assistant("It's sunny."),
    ];
    let result = CompactionPipeline::discard_social(&messages, 2);
    assert!(result.len() < messages.len());
    assert_eq!(result[0].role, openalpaca_llm::Role::System);
    assert!(result.last().unwrap().content.contains("sunny"));
}

#[test]
fn test_discard_preserves_substantive() {
    let messages = vec![
        ChatMessage::system("system prompt"),
        ChatMessage::user("initial query"),
        ChatMessage::user("Write a sort function"),
        ChatMessage::assistant("Here's the implementation..."),
    ];
    let result = CompactionPipeline::discard_social(&messages, 2);
    assert_eq!(result.len(), messages.len());
}
```

- [ ] **Step 2: Implement Phase 2**

```rust
// crates/openalpaca_core/src/context_budget/compaction.rs

use crate::utils::social::is_social_phrase;
use async_trait::async_trait;
use openalpaca_llm::{ChatMessage, Role};

/// 3-phase compaction pipeline for context window management.
pub struct CompactionPipeline;

impl CompactionPipeline {
    /// Phase 2: Discard social/low-value message pairs.
    ///
    /// Preserves: message 0 (system), message 1 (initial query),
    /// last `min_recent` messages. Removes social user messages
    /// and their immediately following assistant responses.
    pub fn discard_social(messages: &[ChatMessage], min_recent: usize) -> Vec<ChatMessage> {
        if messages.len() <= 2 + min_recent {
            return messages.to_vec();
        }

        let preserve_from = messages.len().saturating_sub(min_recent);
        let mut result = Vec::with_capacity(messages.len());

        // Always keep system + initial query
        if !messages.is_empty() {
            result.push(messages[0].clone());
        }
        if messages.len() > 1 {
            result.push(messages[1].clone());
        }

        let mut skip_next_assistant = false;
        for (i, msg) in messages.iter().enumerate().skip(2) {
            if i >= preserve_from {
                result.push(msg.clone());
                continue;
            }

            if skip_next_assistant {
                skip_next_assistant = false;
                if msg.role == Role::Assistant {
                    continue;
                }
            }

            if msg.role == Role::User && is_social_phrase(&msg.content) {
                skip_next_assistant = true;
                continue;
            }

            result.push(msg.clone());
        }

        result
    }
}
```

Add to `context_budget/mod.rs`: `pub(crate) mod compaction;`

- [ ] **Step 3: Run tests**

Run: `cargo test -p openalpaca_core -- context_budget::tests --nocapture`

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/context_budget/
git commit -m "feat(context_budget): CompactionPipeline Phase 2 — heuristic discard (Phase B.1)"
```

---

### Task 3: Implement Phase 1 — memory extraction trait + Phase 3 — summarization trait

**Files:**
- Modify: `crates/openalpaca_core/src/context_budget/compaction.rs`
- Modify: `crates/openalpaca_core/src/context_budget/tests.rs`

- [ ] **Step 1: Write failing tests**

```rust
use super::compaction::{ExtractedMemory, MemoryExtractor, Summarizer};
use async_trait::async_trait;

struct MockExtractor(Vec<ExtractedMemory>);

#[async_trait]
impl MemoryExtractor for MockExtractor {
    async fn extract(&self, _messages: &[ChatMessage]) -> Result<Vec<ExtractedMemory>, String> {
        Ok(self.0.clone())
    }
}

struct MockSummarizer(String);

#[async_trait]
impl Summarizer for MockSummarizer {
    async fn summarize(&self, _messages: &[ChatMessage]) -> Result<String, String> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn test_extraction_returns_memories() {
    let messages = vec![
        ChatMessage::user("I prefer TypeScript over JavaScript"),
        ChatMessage::assistant("Noted, I'll use TypeScript."),
    ];
    let extractor = MockExtractor(vec![ExtractedMemory {
        kind: "user_preference".to_string(),
        content: "User prefers TypeScript".to_string(),
    }]);
    let extracted = extractor.extract(&messages).await.unwrap();
    assert_eq!(extracted.len(), 1);
    assert_eq!(extracted[0].kind, "user_preference");
}

#[tokio::test]
async fn test_summarize_replaces_older_messages() {
    let messages = vec![
        ChatMessage::system("system prompt"),
        ChatMessage::user("initial query"),
        ChatMessage::user("Tell me about Rust"),
        ChatMessage::assistant("Rust is a systems language..."),
        ChatMessage::user("What about ownership?"),
        ChatMessage::assistant("Ownership is Rust's core..."),
        ChatMessage::user("How do lifetimes work?"),
        ChatMessage::assistant("Lifetimes ensure references are valid..."),
    ];
    let summarizer = MockSummarizer("[Summary: discussed Rust]".to_string());
    let result = CompactionPipeline::summarize_older(messages.clone(), 2, &summarizer)
        .await
        .unwrap();
    // System + initial + summary + last 2 messages
    assert!(result.len() <= 5);
    assert!(result.iter().any(|m| m.content.contains("[Summary:")));
    assert_eq!(result.last().unwrap().content, "Lifetimes ensure references are valid...");
}

#[tokio::test]
async fn test_summarize_preserves_recent_when_nothing_to_summarize() {
    let messages = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("init"),
        ChatMessage::user("recent"),
        ChatMessage::assistant("answer"),
    ];
    let summarizer = MockSummarizer("summary".to_string());
    let result = CompactionPipeline::summarize_older(messages.clone(), 4, &summarizer)
        .await
        .unwrap();
    assert_eq!(result.len(), messages.len()); // no change
}
```

- [ ] **Step 2: Implement traits + Phase 3**

Add to `compaction.rs`:

```rust
/// A memory entry extracted during compaction Phase 1.
#[derive(Debug, Clone)]
pub struct ExtractedMemory {
    pub kind: String,
    pub content: String,
}

/// Trait for LLM-based memory extraction (mockable in tests).
#[async_trait]
pub trait MemoryExtractor: Send + Sync {
    async fn extract(&self, messages: &[ChatMessage]) -> Result<Vec<ExtractedMemory>, String>;
}

/// Trait for LLM-based message summarization (mockable in tests).
#[async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize(&self, messages: &[ChatMessage]) -> Result<String, String>;
}

impl CompactionPipeline {
    /// Phase 3: Summarize older messages using an LLM.
    ///
    /// Replaces messages[2..boundary] with a single summary message.
    /// boundary = messages.len() - min_recent.
    pub async fn summarize_older(
        messages: Vec<ChatMessage>,
        min_recent: usize,
        summarizer: &dyn Summarizer,
    ) -> Result<Vec<ChatMessage>, String> {
        if messages.len() <= 2 + min_recent {
            return Ok(messages);
        }

        let boundary = messages.len().saturating_sub(min_recent);
        if boundary <= 2 {
            return Ok(messages);
        }

        let older = &messages[2..boundary];
        let summary_text = summarizer.summarize(older).await?;

        let mut result = Vec::with_capacity(2 + 1 + min_recent);
        result.push(messages[0].clone()); // system
        result.push(messages[1].clone()); // initial query
        result.push(ChatMessage::user(&summary_text)); // summary
        result.extend_from_slice(&messages[boundary..]); // recent

        Ok(result)
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p openalpaca_core -- context_budget::tests --nocapture`

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/context_budget/
git commit -m "feat(context_budget): add extraction trait + summarization Phase 3 (Phase B.2)"
```

---

### Task 4: Make `compress_context` accessible as fallback

**Files:**
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/context.rs` (visibility)
- Modify: `crates/openalpaca_core/src/runner/mod.rs` (re-export)

- [ ] **Step 1: Change visibility**

In `crates/openalpaca_core/src/runner/agentic_loop/context.rs` line 56, change:
```rust
pub(super) fn compress_context(
```
to:
```rust
pub(crate) fn compress_context(
```

Also change `estimate_messages_tokens` at line 27 to `pub(crate)`.

- [ ] **Step 2: Re-export from `runner/mod.rs`**

In `crates/openalpaca_core/src/runner/mod.rs`, add:
```rust
pub(crate) use agentic_loop::context::{compress_context, estimate_messages_tokens};
```

**Note:** `runner/mod.rs` currently declares `mod agentic_loop;` (private). For the `pub(crate) use` to work, the `context` module within `agentic_loop` must be accessible. Since `context.rs` is `mod context;` (private) inside `agentic_loop/mod.rs`, the re-export path needs to go through the agentic_loop module. Change the agentic_loop's `mod.rs` to add:
```rust
pub(crate) use context::{compress_context, estimate_messages_tokens};
```

Then in `runner/mod.rs`:
```rust
pub(crate) use agentic_loop::{compress_context, estimate_messages_tokens};
```

- [ ] **Step 3: Verify build**

Run: `cargo check -p openalpaca_core --all-targets`

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/runner/
git commit -m "refactor: expose compress_context as pub(crate) for fallback (Phase B.3)"
```

---

### Task 5: Implement full `CompactionPipeline::compact()` with fallback

**Files:**
- Modify: `crates/openalpaca_core/src/context_budget/compaction.rs`
- Modify: `crates/openalpaca_core/src/context_budget/tests.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[tokio::test]
async fn test_compaction_full_pipeline() {
    let messages = vec![
        ChatMessage::system("system prompt"),
        ChatMessage::user("initial query"),
        ChatMessage::user("thanks"),
        ChatMessage::assistant("You're welcome!"),
        ChatMessage::user("Tell me about Rust"),
        ChatMessage::assistant("Rust is a systems language..."),
        ChatMessage::user("What about ownership?"),
        ChatMessage::assistant("Ownership is the core..."),
        ChatMessage::user("How do lifetimes work?"),
        ChatMessage::assistant("Lifetimes ensure..."),
    ];

    let extractor = MockExtractor(vec![ExtractedMemory {
        kind: "fact".to_string(),
        content: "Discussed Rust ownership".to_string(),
    }]);
    let summarizer = MockSummarizer("[Summary: Rust discussion]".to_string());

    let result = CompactionPipeline::compact(messages, 2, &extractor, &summarizer).await;
    assert!(result.compacted_messages.len() < 10);
    assert_eq!(result.extracted_memories.len(), 1);
    assert!(result.messages_discarded > 0);
    assert!(result.error.is_none());
}

#[tokio::test]
async fn test_compaction_fallback_on_summarizer_failure() {
    struct FailingSummarizer;

    #[async_trait]
    impl Summarizer for FailingSummarizer {
        async fn summarize(&self, _: &[ChatMessage]) -> Result<String, String> {
            Err("model unavailable".to_string())
        }
    }

    let messages = vec![
        ChatMessage::system("system prompt"),
        ChatMessage::user("initial query"),
        ChatMessage::user("message 1"),
        ChatMessage::assistant("response 1"),
        ChatMessage::user("message 2"),
        ChatMessage::assistant("response 2"),
        ChatMessage::user("message 3"),
        ChatMessage::assistant("response 3"),
        ChatMessage::user("recent"),
        ChatMessage::assistant("recent response"),
    ];

    let result = CompactionPipeline::compact(messages, 2, &MockExtractor(vec![]), &FailingSummarizer).await;
    assert!(result.compacted_messages.len() < 10);
    assert!(result.error.is_some());
    assert!(result.error.unwrap().contains("model unavailable"));
}

#[tokio::test]
async fn test_compaction_preserves_recent() {
    let messages = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("init"),
        ChatMessage::user("old1"),
        ChatMessage::assistant("old_resp1"),
        ChatMessage::user("recent_q"),
        ChatMessage::assistant("recent_a"),
    ];
    let result = CompactionPipeline::compact(
        messages, 2, &MockExtractor(vec![]), &MockSummarizer("summary".to_string()),
    ).await;
    let last = result.compacted_messages.last().unwrap();
    assert_eq!(last.content, "recent_a");
}
```

- [ ] **Step 2: Implement `compact()` + `CompactionResult`**

```rust
/// Result of a full compaction pipeline run.
#[derive(Debug)]
pub struct CompactionResult {
    pub compacted_messages: Vec<ChatMessage>,
    pub extracted_memories: Vec<ExtractedMemory>,
    pub messages_discarded: usize,
    pub messages_before: usize,
    pub messages_after: usize,
    pub error: Option<String>,
}

impl CompactionPipeline {
    /// Run the full 3-phase compaction pipeline.
    ///
    /// Phase 1: Extract memories (LLM). On error: skip, log.
    /// Phase 2: Discard social messages (heuristic). Always succeeds.
    /// Phase 3: Summarize older messages (LLM). On error: fall back to
    ///          existing `compress_context()` heuristic.
    pub async fn compact(
        messages: Vec<ChatMessage>,
        min_recent: usize,
        extractor: &dyn MemoryExtractor,
        summarizer: &dyn Summarizer,
    ) -> CompactionResult {
        let messages_before = messages.len();

        // Phase 1: Memory extraction (best-effort)
        let boundary = messages.len().saturating_sub(min_recent).max(2);
        let older = if boundary > 2 { &messages[2..boundary] } else { &[] as &[ChatMessage] };

        let extracted_memories = match extractor.extract(older).await {
            Ok(memories) => {
                tracing::info!(count = memories.len(), "Compaction Phase 1: extracted memories");
                memories
            }
            Err(e) => {
                tracing::warn!("Compaction Phase 1 (extraction) failed, skipping: {e}");
                vec![]
            }
        };

        // Phase 2: Discard social messages (always succeeds)
        let after_discard = Self::discard_social(&messages, min_recent);
        let messages_discarded = messages_before - after_discard.len();
        tracing::info!(discarded = messages_discarded, "Compaction Phase 2: social discard");

        // Phase 3: Summarize older messages (with fallback)
        match Self::summarize_older(after_discard.clone(), min_recent, summarizer).await {
            Ok(compacted) => {
                let messages_after = compacted.len();
                CompactionResult {
                    compacted_messages: compacted,
                    extracted_memories,
                    messages_discarded,
                    messages_before,
                    messages_after,
                    error: None,
                }
            }
            Err(e) => {
                tracing::warn!("Compaction Phase 3 (summarize) failed, heuristic fallback: {e}");
                let mut fallback = after_discard;
                // compress_context expects tail_keep in "rounds" (×3 internally)
                // Convert min_recent (message count) to rounds via ceiling division
                let tail_keep = ((min_recent + 2) / 3).max(1);
                crate::runner::compress_context(&mut fallback, tail_keep);
                let messages_after = fallback.len();
                CompactionResult {
                    compacted_messages: fallback,
                    extracted_memories,
                    messages_discarded,
                    messages_before,
                    messages_after,
                    error: Some(format!("Phase 3 failed: {e}")),
                }
            }
        }
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p openalpaca_core -- context_budget::tests --nocapture`

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/context_budget/
git commit -m "feat(context_budget): full CompactionPipeline with 3-phase + fallback (Phase B.4)"
```

---

### Task 6: Add compaction events + telemetry migration

**Files:**
- Modify: `crates/openalpaca_core/src/events.rs`
- Modify: `apps/openalpacad/src/event_bridge.rs`
- Create: `crates/openalpaca_storage/src/migrations/032_context_compaction_log.sql`
- Modify: `crates/openalpaca_storage/src/migrations/mod.rs`

- [ ] **Step 1: Add event variants**

In `events.rs` after `ContextBudgetComputed`:

```rust
    /// Context compaction was triggered and completed
    CompactionTriggered {
        request_id: Uuid,
        utilization_pct: f64,
        messages_before: usize,
        messages_after: usize,
        memories_extracted: usize,
        messages_discarded: usize,
        summary_tokens: usize,
        timestamp: DateTime<Utc>,
    },
    /// A single compaction phase completed
    CompactionPhaseCompleted {
        request_id: Uuid,
        phase: String,
        duration_ms: u64,
        items_processed: usize,
        timestamp: DateTime<Utc>,
    },
```

- [ ] **Step 2: Add match arms in `event_bridge.rs`**

```rust
openalpaca_core::events::SystemEvent::CompactionTriggered {
    request_id, messages_before, messages_after, memories_extracted, ..
} => {
    tracing::info!(
        %request_id, messages_before, messages_after, memories_extracted,
        "Context compaction triggered"
    );
}
openalpaca_core::events::SystemEvent::CompactionPhaseCompleted {
    request_id, ref phase, duration_ms, items_processed, ..
} => {
    tracing::debug!(
        %request_id, %phase, duration_ms, items_processed,
        "Compaction phase completed"
    );
}
```

- [ ] **Step 3: Create migration**

```sql
-- crates/openalpaca_storage/src/migrations/032_context_compaction_log.sql
CREATE TABLE IF NOT EXISTS context_compaction_log (
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
    fallback_used INTEGER DEFAULT 0,
    timestamp TEXT DEFAULT (datetime('now'))
);
```

Register in `migrations/mod.rs`. Update schema_version assertions from `31` to `32` in **5 places**:
- `crates/openalpaca_storage/src/database/tests.rs` — `db.schema_version()` test
- `crates/openalpaca_storage/src/database/tests.rs` — `db2.schema_version()` test (separate assertion!)
- `crates/openalpaca_storage/src/repository/dispatch_decision/tests.rs`
- `crates/openalpaca_storage/src/repository/llm_usage/tests.rs`

- [ ] **Step 4: Verify build + migration**

Run: `cargo check --all-targets && cargo test -p openalpaca_storage -- migrations --nocapture`

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/events.rs apps/openalpacad/src/event_bridge.rs crates/openalpaca_storage/src/migrations/
git commit -m "feat(context_budget): compaction events + telemetry migration 032 (Phase B.5)"
```

---

### Task 7: Wire `ContextBudgetManager` into agentic loop

**IMPORTANT:** This changes the signature of `run_agentic_loop()` and `run_agentic_loop_routed()`. All **14 call sites** in `tests.rs` + all production call sites must be updated.

**Files:**
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/mod.rs`
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/tests.rs` (14 call sites)
- Modify: `crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs`
- Modify: `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs`
- Modify: `crates/openalpaca_core/src/runner/dag_executor/node_runner.rs`
- Modify: `crates/openalpaca_core/src/runner/lead_agent/mod.rs`
- Modify: `crates/openalpaca_core/src/runner/lead_agent/tools.rs`
- Modify: `crates/openalpaca_core/src/orchestrator/skill/invocation.rs`

- [ ] **Step 1: Add `context_budget` parameter to public functions**

In `run_agentic_loop()` and `run_agentic_loop_routed()`, add parameter:
```rust
context_budget: Option<&crate::context_budget::ContextBudgetManager>,
```

Thread it through to `run_agentic_loop_inner()`.

- [ ] **Step 2: Update compression trigger**

Replace existing compression block (~lines 260-276) with:

```rust
// Context compression (budget-aware)
if let Some(budget) = context_budget {
    let msg_tokens = estimate_messages_tokens(&messages) as usize;
    if budget.should_compact(msg_tokens) {
        tracing::info!(msg_tokens, trigger = budget.compaction_trigger(), "Budget compaction triggered");
        compress_context(Arc::make_mut(&mut messages), config.context_tail_keep);
        known_token_count = estimate_messages_tokens(&messages);
    }
} else if config.max_context_tokens > 0 && known_token_count > config.max_context_tokens {
    // Legacy fallback (deprecated — Phase B migration)
    tracing::debug!(tokens = known_token_count, max = config.max_context_tokens, "Legacy compression");
    compress_context(Arc::make_mut(&mut messages), config.context_tail_keep);
    known_token_count = estimate_messages_tokens(&messages);
}
```

- [ ] **Step 3: Update ALL call sites to pass `None`**

Production call sites (add `None` as the `context_budget` argument):
1. `simple_query_handler.rs` — `run_agentic_loop_routed()` calls (2 locations: main path + social fast path)
2. `pipeline_step.rs` — `run_agentic_loop_routed()` call
3. `node_runner.rs` — `run_agentic_loop_routed()` call
4. `runner/lead_agent/mod.rs` — `run_agentic_loop_routed()` call
5. `runner/lead_agent/tools.rs` — `run_agentic_loop_routed()` call
6. `orchestrator/skill/invocation.rs` — `run_agentic_loop_routed()` call

Test call sites — all 14 in `runner/agentic_loop/tests.rs` (lines 109, 139, 168, 189, 217, 244, 305, 365, 428, 454, 526, 606, 646, 713):
```rust
// Each call: add `None,` before the cancel_token parameter
let result = run_agentic_loop(
    &provider,
    initial_messages,
    tools,
    &config,
    None, // sandbox
    "test_agent",
    None, // sandbox_policy
    None, // context_budget  <-- ADD THIS
    None, // cancel_token
)
.await;
```

- [ ] **Step 4: Verify build + ALL tests**

Run: `cargo check --all-targets && cargo test -p openalpaca_core`

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/runner/ crates/openalpaca_core/src/orchestrator/
git commit -m "feat(context_budget): wire budget manager into agentic loop (Phase B.6)"
```

---

### Task 8: Phase B verification

- [ ] **Step 1:** `cargo check --all-targets`
- [ ] **Step 2:** `cargo test -p openalpaca_core`
- [ ] **Step 3:** `cargo test -p openalpaca_storage`
- [ ] **Step 4:** `cargo clippy -p openalpaca_core -- -D warnings`
- [ ] **Step 5:** `cargo check -p openalpacad`
