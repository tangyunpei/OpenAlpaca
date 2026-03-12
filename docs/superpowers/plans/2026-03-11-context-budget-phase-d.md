# Context Budget Phase D: Context Management API Integration

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate Claude's `context_management` beta API for server-side tool-use and thinking clearing in multi-round agentic loops.

**Architecture:** Add `context_management` field to `ChatRequest` and `RouterRequest`. Serialize in Anthropic provider's `build_request_body`. Wire into agentic loop to pass thresholds from `ContextBudgetManager`.

**Tech Stack:** Rust, serde_json, openalpaca_llm types.

**Spec:** `docs/superpowers/specs/2026-03-11-context-budget-design.md` — Section 3.6.

**Depends on:** Phase A (ContextBudgetManager for trigger values). Independent of Phase B/C.

---

## File Structure

| Action | Path | Purpose |
|--------|------|---------|
| Create | `crates/openalpaca_llm/src/context_management.rs` | Types for `ContextManagement`, `ContextEdit` |
| Modify | `crates/openalpaca_llm/src/lib.rs` | Add `pub mod context_management;` |
| Modify | `crates/openalpaca_llm/src/types.rs` | Add `context_management` field to `ChatRequest` |
| Modify | `crates/openalpaca_llm/src/routing/router/types.rs` | Add `context_management` field to `RouterRequest` |
| Modify | `crates/openalpaca_llm/src/routing/router/completion.rs` | Propagate field from `RouterRequest` → `ChatRequest` |
| Modify | `crates/openalpaca_llm/src/providers/anthropic/request.rs` | Serialize `context_management` in `build_request_body` |
| Modify | `crates/openalpaca_llm/src/providers/anthropic/mod.rs` | Add `anthropic-beta` header when context_management is present |
| Modify | `crates/openalpaca_core/src/runner/agentic_loop/backend.rs` | Thread context_management through `LlmBackend::complete()` |
| Modify | `crates/openalpaca_core/src/runner/agentic_loop/mod.rs` | Build context_management from budget manager, pass to backend |
| Modify | All `ChatRequest` + `RouterRequest` construction sites | Add `context_management: None` (~29 sites total) |

---

### Task 1: Define `ContextManagement` types

**Files:**
- Create: `crates/openalpaca_llm/src/context_management.rs`
- Modify: `crates/openalpaca_llm/src/lib.rs`

- [ ] **Step 1: Write tests**

```rust
// crates/openalpaca_llm/src/context_management.rs (inline tests)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clear_tool_uses_serialization() {
        let edit = ContextEdit::ClearToolUses {
            trigger_tokens: 100_000,
            keep_tool_uses: 5,
        };
        let val = edit.to_json();
        assert_eq!(val["type"], "clear_tool_uses_20250919");
        assert_eq!(val["trigger"]["type"], "input_tokens");
        assert_eq!(val["trigger"]["value"], 100_000);
        assert_eq!(val["keep"]["type"], "tool_uses");
        assert_eq!(val["keep"]["value"], 5);
    }

    #[test]
    fn test_clear_thinking_serialization() {
        let edit = ContextEdit::ClearThinking {
            keep_thinking_turns: 2,
        };
        let val = edit.to_json();
        assert_eq!(val["type"], "clear_thinking_20251015");
        assert_eq!(val["keep"]["type"], "thinking_turns");
        assert_eq!(val["keep"]["value"], 2);
    }

    #[test]
    fn test_context_management_serialization() {
        let mgmt = ContextManagement {
            edits: vec![
                ContextEdit::ClearThinking { keep_thinking_turns: 2 },
                ContextEdit::ClearToolUses { trigger_tokens: 100_000, keep_tool_uses: 5 },
            ],
        };
        let val = mgmt.to_json();
        assert!(val["edits"].is_array());
        assert_eq!(val["edits"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_from_budget_builds_correct_edits() {
        let mgmt = ContextManagement::from_budget(167_000, 5, 2);
        assert_eq!(mgmt.edits.len(), 2);
    }

    #[test]
    fn test_empty_context_management() {
        let mgmt = ContextManagement { edits: vec![] };
        let val = mgmt.to_json();
        assert!(val["edits"].as_array().unwrap().is_empty());
    }
}
```

- [ ] **Step 2: Implement types**

```rust
// crates/openalpaca_llm/src/context_management.rs

/// Claude API context_management configuration for server-side context editing.
///
/// Reference: Anthropic API docs — context_management beta feature.
#[derive(Debug, Clone)]
pub struct ContextManagement {
    pub edits: Vec<ContextEdit>,
}

/// A single context edit instruction.
#[derive(Debug, Clone)]
pub enum ContextEdit {
    /// Clear old tool-use blocks when input tokens exceed trigger.
    ClearToolUses {
        trigger_tokens: usize,
        keep_tool_uses: usize,
    },
    /// Clear old extended-thinking blocks, keeping N most recent.
    ClearThinking {
        keep_thinking_turns: usize,
    },
}

impl ContextEdit {
    /// Serialize to the Anthropic API JSON format.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            ContextEdit::ClearToolUses { trigger_tokens, keep_tool_uses } => {
                serde_json::json!({
                    "type": "clear_tool_uses_20250919",
                    "trigger": {
                        "type": "input_tokens",
                        "value": trigger_tokens
                    },
                    "keep": {
                        "type": "tool_uses",
                        "value": keep_tool_uses
                    }
                })
            }
            ContextEdit::ClearThinking { keep_thinking_turns } => {
                serde_json::json!({
                    "type": "clear_thinking_20251015",
                    "keep": {
                        "type": "thinking_turns",
                        "value": keep_thinking_turns
                    }
                })
            }
        }
    }
}

impl ContextManagement {
    /// Build from budget manager parameters.
    ///
    /// - `compaction_trigger`: input token threshold (from ContextBudgetManager)
    /// - `keep_tool_uses`: number of recent tool-use blocks to keep
    /// - `keep_thinking_turns`: number of recent thinking turns to keep
    pub fn from_budget(
        compaction_trigger: usize,
        keep_tool_uses: usize,
        keep_thinking_turns: usize,
    ) -> Self {
        Self {
            edits: vec![
                ContextEdit::ClearThinking { keep_thinking_turns },
                ContextEdit::ClearToolUses {
                    trigger_tokens: compaction_trigger,
                    keep_tool_uses,
                },
            ],
        }
    }

    /// Serialize to the Anthropic API JSON format.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "edits": self.edits.iter().map(|e| e.to_json()).collect::<Vec<_>>()
        })
    }
}
```

Add to `crates/openalpaca_llm/src/lib.rs`:
```rust
pub mod context_management;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p openalpaca_llm -- context_management --nocapture`
Expected: 5 tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_llm/src/context_management.rs crates/openalpaca_llm/src/lib.rs
git commit -m "feat(context_management): define ContextManagement + ContextEdit types (Phase D.1)"
```

---

### Task 2: Add `context_management` to `ChatRequest` and `RouterRequest`

**Files:**
- Modify: `crates/openalpaca_llm/src/types.rs`
- Modify: `crates/openalpaca_llm/src/routing/router/types.rs`

**CRITICAL:** Neither `ChatRequest` nor `RouterRequest` derives `Serialize`. They're runtime-only structs. The `context_management` field is `Option<ContextManagement>` — no serde attributes needed.

- [ ] **Step 1: Add field to `ChatRequest`**

In `crates/openalpaca_llm/src/types.rs`, add after `thinking: Option<ThinkingConfig>` (the last field, line ~294):

```rust
    /// Context management configuration (Anthropic only). Other providers ignore this.
    pub context_management: Option<crate::context_management::ContextManagement>,
```

- [ ] **Step 2: Add field to `RouterRequest`**

In `crates/openalpaca_llm/src/routing/router/types.rs`, add after `thinking: Option<ThinkingConfig>` (the last field, line ~31):

```rust
    /// Context management configuration (Anthropic only).
    pub context_management: Option<crate::context_management::ContextManagement>,
```

- [ ] **Step 3: Fix ALL compilation errors**

Adding a new field to these structs will break every construction site. Search for all places that construct `ChatRequest` and `RouterRequest` and add `context_management: None`.

**Find all construction sites:**

```bash
# ChatRequest construction sites
cargo check -p openalpaca_llm 2>&1 | grep "missing field"

# RouterRequest construction sites
cargo check --all-targets 2>&1 | grep "missing field"
```

**Approach:** Run `cargo check --all-targets`, fix each "missing field" error by adding `context_management: None`. This is safer than guessing — the compiler tells you exactly which sites need updating.

**Expect ~29 construction sites.** Complete enumeration:

**ChatRequest** (~7 production + ~19 test):
- `crates/openalpaca_llm/src/routing/router/completion.rs:50` — `complete_streaming` conversion
- `crates/openalpaca_llm/src/routing/router/retry.rs:58` — `execute_with_retry`
- `crates/openalpaca_llm/src/routing/router/fallback.rs:30` — CLI fallback path
- `crates/openalpaca_core/src/runner/agentic_loop/backend.rs:41` — `LlmBackend::Direct` path
- `crates/openalpaca_llm/src/providers/anthropic/tests.rs` — ~12 test constructions
- `crates/openalpaca_llm/src/providers/openai/tests.rs` — ~7 test constructions

**RouterRequest** (~9 production + ~1 test):
- `crates/openalpaca_core/src/runner/agentic_loop/backend.rs:56` — streaming RouterRequest
- `crates/openalpaca_core/src/runner/agentic_loop/backend.rs:123` — non-streaming RouterRequest
- `crates/openalpaca_core/src/orchestrator/summary.rs:67` — summary generation
- `crates/openalpaca_core/src/orchestrator/extraction.rs:110` — user trait extraction
- `crates/openalpaca_core/src/orchestrator/replanner/mod.rs:84` — replanner
- `crates/openalpaca_core/src/orchestrator/task_planner/response_parser.rs:248` — task planner retry
- `crates/openalpaca_core/src/orchestrator/task_planner/response_parser.rs:401` — intent triage
- `crates/openalpaca_core/src/memory/task_extraction/mod.rs:117` — task output extraction
- `crates/openalpaca_llm/src/routing/router/tests.rs:67` — `make_request()` test helper

**Note:** `simple_query_handler.rs` does NOT construct RouterRequest directly — it calls `run_agentic_loop_routed()` which delegates to `backend.rs`.

- [ ] **Step 4: Verify build**

Run: `cargo check --all-targets`
Expected: Clean build

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_llm/src/types.rs crates/openalpaca_llm/src/routing/
git commit -m "feat(context_management): add context_management to ChatRequest + RouterRequest (Phase D.2)"
```

---

### Task 3: Propagate through router to Anthropic provider

**Files:**
- Modify: `crates/openalpaca_llm/src/routing/router/completion.rs`
- Modify: `crates/openalpaca_llm/src/providers/anthropic/request.rs`

- [ ] **Step 1: Thread through router conversion**

In `completion.rs`, where `RouterRequest` is converted to `ChatRequest` (the conversion that passes to providers), ensure `context_management` is propagated:

```rust
// In the ChatRequest construction from RouterRequest:
context_management: request.context_management.clone(),
```

This may already be handled in Step 2 above when fixing compilation. Verify it's present.

- [ ] **Step 2: Serialize in Anthropic `build_request_body`**

In `crates/openalpaca_llm/src/providers/anthropic/request.rs`, in the `build_request_body` function, after the thinking config block (~line 246) and before the final return:

```rust
// Context management (server-side tool/thinking clearing)
if let Some(ref ctx_mgmt) = request.context_management {
    body["context_management"] = ctx_mgmt.to_json();
}
```

**Note:** Non-Anthropic providers (OpenAI, Ollama) should silently ignore this field — they never read `request.context_management`, so no changes needed there.

- [ ] **Step 2.5: Add `anthropic-beta` header for context_management**

The `context_management` API is a beta feature requiring the `anthropic-beta` header. In `crates/openalpaca_llm/src/providers/anthropic/mod.rs`, locate the `chat_with_key` and `chat_streaming_with_key` methods where HTTP headers are set.

Add conditional beta header when `context_management` is present:

```rust
// After existing headers (.header("anthropic-version", API_VERSION))
if request.context_management.is_some() {
    req_builder = req_builder.header("anthropic-beta", "interleaved-thinking-2025-05-14,context-management-2025-01-15");
}
```

**Note:** Check the current Anthropic API docs for the exact beta flag name. The `interleaved-thinking` beta may already be sent — if so, append the context-management flag to the existing comma-separated beta string.

- [ ] **Step 3: Write integration test**

Add to `crates/openalpaca_llm/src/providers/anthropic/tests.rs` (tests MUST be inside the `providers::anthropic` module because `build_request_body` is `pub(super)`):

```rust
#[test]
fn test_build_request_body_with_context_management() {
    use crate::context_management::{ContextManagement, ContextEdit};

    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("hello")]),
        tools: Arc::new(vec![]),
        model: Some("claude-sonnet-4-20250514".to_string()),
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: Some(ContextManagement {
            edits: vec![
                ContextEdit::ClearThinking { keep_thinking_turns: 2 },
                ContextEdit::ClearToolUses { trigger_tokens: 100_000, keep_tool_uses: 5 },
            ],
        }),
    };

    let body = build_request_body("claude-sonnet-4-20250514", 4096, &request);
    assert!(body.get("context_management").is_some());
    let edits = body["context_management"]["edits"].as_array().unwrap();
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0]["type"], "clear_thinking_20251015");
    assert_eq!(edits[1]["type"], "clear_tool_uses_20250919");
}

#[test]
fn test_build_request_body_without_context_management() {
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("hello")]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
    };

    let body = build_request_body("claude-sonnet-4-20250514", 4096, &request);
    assert!(body.get("context_management").is_none());
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p openalpaca_llm -- anthropic --nocapture`

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_llm/src/routing/router/completion.rs crates/openalpaca_llm/src/providers/anthropic/
git commit -m "feat(context_management): serialize in Anthropic provider (Phase D.3)"
```

---

### Task 4: Wire into agentic loop via `backend.rs`

**IMPORTANT:** `RouterRequest` is NOT constructed in `agentic_loop/mod.rs`. It is constructed in `agentic_loop/backend.rs` inside the `LlmBackend::complete()` method (lines 56 and 123 — streaming and non-streaming paths). The `mod.rs` calls `backend.complete(...)` with individual parameters.

**Files:**
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/backend.rs`
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/mod.rs`

- [ ] **Step 1: Read both files**

Read `backend.rs` fully. Identify:
- `LlmBackend::complete()` method signature (what parameters it takes)
- Both `RouterRequest { ... }` construction sites (streaming at ~line 56, non-streaming at ~line 123)
- The `LlmBackend::Direct` path where `ChatRequest` is constructed (~line 41)

Read `mod.rs`. Identify where `backend.complete(...)` is called in the loop.

- [ ] **Step 2: Extend `LlmBackend::complete()` signature**

Add a `context_management` parameter to `LlmBackend::complete()`:

```rust
pub async fn complete(
    &self,
    // ... existing parameters ...
    context_management: Option<openalpaca_llm::context_management::ContextManagement>,
) -> Result<...> {
```

Thread it into both `RouterRequest` constructions:
```rust
RouterRequest {
    // ... existing fields ...
    context_management: context_management.clone(),
}
```

And for the `LlmBackend::Direct` path's `ChatRequest`:
```rust
ChatRequest {
    // ... existing fields ...
    context_management: context_management.clone(),
}
```

- [ ] **Step 3: Compute `ContextManagement` in `mod.rs` and pass to backend**

In `agentic_loop/mod.rs`, before the loop's `backend.complete(...)` call, compute the context_management value:

```rust
// Build context_management from budget manager (Phase D)
let context_management = context_budget.map(|budget| {
    openalpaca_llm::context_management::ContextManagement::from_budget(
        budget.compaction_trigger(),
        5,  // keep 5 recent tool-use blocks
        2,  // keep 2 recent thinking turns
    )
});
```

If Phase B has not been implemented yet (no `context_budget` parameter on the loop), use a standalone approach:

```rust
// Phase D standalone: use full model window for trigger calculation
// Note: config.max_context_tokens is already model_window * context_threshold,
// so use it directly as the trigger (do NOT multiply by context_threshold again)
let context_management = if config.max_context_tokens > 0 {
    Some(openalpaca_llm::context_management::ContextManagement::from_budget(
        config.max_context_tokens as usize, 5, 2,
    ))
} else {
    None
};
```

Pass to `backend.complete(...)`:
```rust
let response = backend.complete(
    // ... existing args ...
    context_management.clone(),
).await?;
```

- [ ] **Step 4: Verify build + ALL tests**

Run: `cargo check --all-targets && cargo test -p openalpaca_core -- agentic_loop --nocapture`

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/runner/agentic_loop/
git commit -m "feat(context_management): wire into agentic loop via backend (Phase D.4)"
```

---

### Task 5: Phase D verification

- [ ] **Step 1:** `cargo check --all-targets`
- [ ] **Step 2:** `cargo test -p openalpaca_llm`
- [ ] **Step 3:** `cargo test -p openalpaca_core`
- [ ] **Step 4:** `cargo clippy -p openalpaca_llm -- -D warnings`
- [ ] **Step 5:** `cargo clippy -p openalpaca_core -- -D warnings`
- [ ] **Step 6:** `cargo check -p openalpacad`
