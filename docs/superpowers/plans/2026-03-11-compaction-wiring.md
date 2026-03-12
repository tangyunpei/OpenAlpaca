# Compaction Pipeline Wiring Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the Phase B `CompactionPipeline` into the production agentic loop, implement LLM-based compaction via `LlmBackend` traits, improve the heuristic fallback, and wire `ContextBudgetManager` into all three execution paths.

**Architecture:** `LlmBackend` implements `MemoryExtractor` and `Summarizer` traits using a dedicated compaction model routed through `LlmRouter`. The agentic loop's compression block is replaced with `CompactionPipeline::compact()` when an LLM compactor is available, falling back to an improved token-aware heuristic. `ContextBudgetManager` is instantiated in pipeline_step, node_runner, and lead_agent, and passed to the loop.

**Tech Stack:** Rust, async_trait, openalpaca_llm (LlmRouter, RouterRequest, ChatMessage), openalpaca_core (ContextBudgetManager, CompactionPipeline)

**Spec:** `docs/superpowers/specs/2026-03-11-compaction-wiring-design.md`

---

## File Structure

| Action | Path | Purpose |
|--------|------|---------|
| Modify | `crates/openalpaca_core/src/runner/agentic_loop/config.rs` | Add `compaction_model` to LoopConfig |
| Modify | `crates/openalpaca_core/src/runner/agentic_loop/backend.rs` | Add `compaction_model` to Router variant, impl MemoryExtractor + Summarizer |
| Modify | `crates/openalpaca_core/src/runner/agentic_loop/context.rs` | Rewrite `compress_context()` with token-aware heuristic |
| Modify | `crates/openalpaca_core/src/runner/agentic_loop/mod.rs` | Replace compression block with CompactionPipeline + events |
| Modify | `crates/openalpaca_core/src/context_budget/compaction.rs` | Update Phase 3 fallback call signature |
| Verify | `crates/openalpaca_core/src/context_budget/mod.rs` | Confirm compaction module is `pub(crate)` (sufficient for intra-crate access) |
| Modify | `crates/openalpaca_core/src/context_budget/tests.rs` | Add tests for improved heuristic + LlmBackend trait impls |
| Modify | `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs` | Instantiate ContextBudgetManager, set compaction_model, pass to loop |
| Modify | `crates/openalpaca_core/src/runner/dag_executor/node_runner.rs` | Same as pipeline_step |
| Modify | `crates/openalpaca_core/src/runner/lead_agent/mod.rs` | Same as pipeline_step (from_lead_agent path) |

---

## Chunk 1: Core Infrastructure

### Task 1: Add `compaction_model` to LoopConfig

**Files:**
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/config.rs:17-97`

This task adds the `compaction_model` field to `LoopConfig` so the agentic loop knows which model to use for LLM-based compaction.

- [ ] **Step 1: Add field to LoopConfig struct**

In `crates/openalpaca_core/src/runner/agentic_loop/config.rs`, add after `max_stream_duration` (line 56):

```rust
    /// Model to use for LLM-based context compaction (extraction + summarization).
    /// When `None`, falls back to heuristic-only compaction.
    pub compaction_model: Option<String>,
```

- [ ] **Step 2: Update Default impl**

In the `Default` impl (around line 107), add after `max_stream_duration`:

```rust
            compaction_model: None,
```

- [ ] **Step 3: Update `from_defaults()` return value**

In `from_defaults()` (around line 166), add after `max_stream_duration` in the `Self { ... }` block:

```rust
            compaction_model: None,
```

- [ ] **Step 4: Update manual Clone impl**

In the `Clone` impl (lines 59-79), add after `max_stream_duration: self.max_stream_duration,`:

```rust
            compaction_model: self.compaction_model.clone(),
```

- [ ] **Step 5: Update manual Debug impl**

In the `Debug` impl (lines 81-97), add after `.field("max_stream_duration", &self.max_stream_duration)`:

```rust
            .field("compaction_model", &self.compaction_model)
```

- [ ] **Step 6: Verify build**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: Clean build (no errors)

- [ ] **Step 7: Commit**

```bash
git add crates/openalpaca_core/src/runner/agentic_loop/config.rs
git commit -m "feat(compaction): add compaction_model to LoopConfig"
```

---

### Task 2: Implement MemoryExtractor + Summarizer traits on LlmBackend

**Files:**
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/backend.rs:12-20`
- Modify: `crates/openalpaca_core/src/context_budget/mod.rs`
- Modify: `crates/openalpaca_core/src/context_budget/tests.rs`

This task makes `LlmBackend` capable of LLM-based compaction by implementing both traits. The `Router` variant uses a dedicated compaction model; the `Direct` variant returns `Err` (triggering heuristic fallback).

- [ ] **Step 1: Verify compaction module visibility**

In `crates/openalpaca_core/src/context_budget/mod.rs`, verify `compaction` is declared as `pub(crate) mod compaction;`. This is sufficient — `backend.rs` (in the `runner` module) is in the same crate and can access `pub(crate)` items. No change needed here.

- [ ] **Step 2: Add `compaction_model` to `LlmBackend::Router` variant**

In `crates/openalpaca_core/src/runner/agentic_loop/backend.rs`, change the `Router` variant (line 16-19):

```rust
    Router {
        router: &'a LlmRouter,
        context: RequestContext,
    },
```

to:

```rust
    Router {
        router: &'a LlmRouter,
        context: RequestContext,
        compaction_model: Option<String>,
    },
```

- [ ] **Step 3: Fix the `LlmBackend::Router` construction site in mod.rs**

In `crates/openalpaca_core/src/runner/agentic_loop/mod.rs`, find where `LlmBackend::Router` is constructed inline (line 148, inside `run_agentic_loop_routed`). It currently looks like:

```rust
    let context = RequestContext {
        agent_id: Some(agent_id.to_string()),
        task_id: task_id.map(|s| s.to_string()),
    };
    run_agentic_loop_inner(
        LlmBackend::Router { router, context },
        ...
```

Add the `compaction_model` field to the inline construction:

```rust
    let context = RequestContext {
        agent_id: Some(agent_id.to_string()),
        task_id: task_id.map(|s| s.to_string()),
    };
    run_agentic_loop_inner(
        LlmBackend::Router { router, context, compaction_model: config.compaction_model.clone() },
        ...
```

- [ ] **Step 4: Fix the `complete()` method's pattern match**

In `backend.rs`, the `complete()` method matches on `Self::Router { router, context }`. Update it to:

```rust
Self::Router { router, context, .. }
```

(The `compaction_model` field is not used in `complete()` — it's only used by the trait impls.)

Also check `supports_retry()` and `task_cost()` methods for pattern matches on the `Router` variant and add `..` if needed.

- [ ] **Step 5: Implement MemoryExtractor on LlmBackend**

Add to the bottom of `backend.rs`:

```rust
#[async_trait::async_trait]
impl<'a> crate::context_budget::compaction::MemoryExtractor for LlmBackend<'a> {
    async fn extract(
        &self,
        messages: &[openalpaca_llm::ChatMessage],
    ) -> Result<Vec<crate::context_budget::compaction::ExtractedMemory>, String> {
        let (router, compaction_model, req_context) = match self {
            Self::Router { router, compaction_model, context } => {
                (*router, compaction_model.clone(), context.clone())
            }
            Self::Direct { .. } => return Err("No router available for LLM extraction".into()),
        };

        // Build extraction prompt
        let mut extract_messages = Vec::with_capacity(messages.len() + 1);
        extract_messages.push(openalpaca_llm::ChatMessage::system(
            "You are a memory extraction assistant. Extract key facts, decisions, and important \
             information from the following conversation messages. Return each memory as a line \
             in the format:\n\
             KIND: content\n\n\
             Valid KINDs: fact, decision, preference, instruction, context\n\n\
             Only extract genuinely important information. Be concise. Maximum 10 entries.",
        ));
        let combined: String = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    openalpaca_llm::Role::User => "User",
                    openalpaca_llm::Role::Assistant => "Assistant",
                    openalpaca_llm::Role::System => "System",
                    openalpaca_llm::Role::Tool => "Tool",
                };
                format!("[{role}]: {}", m.content)
            })
            .collect::<Vec<_>>()
            .join("\n");
        extract_messages.push(openalpaca_llm::ChatMessage::user(&combined));

        let request = openalpaca_llm::routing::RouterRequest {
            messages: std::sync::Arc::new(extract_messages),
            tools: std::sync::Arc::new(vec![]),
            model: compaction_model,
            temperature: Some(0.0),
            max_tokens: Some(1024),
            context: req_context,
            tool_choice: None,
            tools_token_estimate: None,
            enable_caching: false,
            thinking: None,
            context_management: None,
        };

        let response = router.complete(request).await.map_err(|e| e.to_string())?;

        // Parse response lines into ExtractedMemory
        let memories = response
            .content
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() {
                    return None;
                }
                if let Some((kind, content)) = line.split_once(':') {
                    let kind = kind.trim().to_lowercase();
                    let content = content.trim().to_string();
                    if !content.is_empty() {
                        return Some(crate::context_budget::compaction::ExtractedMemory {
                            kind,
                            content,
                        });
                    }
                }
                None
            })
            .take(10)
            .collect();

        Ok(memories)
    }
}
```

- [ ] **Step 6: Implement Summarizer on LlmBackend**

Add after the `MemoryExtractor` impl:

```rust
#[async_trait::async_trait]
impl<'a> crate::context_budget::compaction::Summarizer for LlmBackend<'a> {
    async fn summarize(
        &self,
        messages: &[openalpaca_llm::ChatMessage],
    ) -> Result<String, String> {
        let (router, compaction_model, req_context) = match self {
            Self::Router { router, compaction_model, context } => {
                (*router, compaction_model.clone(), context.clone())
            }
            Self::Direct { .. } => {
                return Err("No router available for LLM summarization".into())
            }
        };

        // Build summarization prompt
        let mut sum_messages = Vec::with_capacity(2);
        sum_messages.push(openalpaca_llm::ChatMessage::system(
            "You are a conversation summarizer. Summarize the following conversation messages \
             into a concise paragraph that captures the key points, decisions made, and important \
             context. Focus on information that would be needed to continue the conversation. \
             Be concise but complete.",
        ));
        let combined: String = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    openalpaca_llm::Role::User => "User",
                    openalpaca_llm::Role::Assistant => "Assistant",
                    openalpaca_llm::Role::System => "System",
                    openalpaca_llm::Role::Tool => "Tool",
                };
                format!("[{role}]: {}", m.content)
            })
            .collect::<Vec<_>>()
            .join("\n");
        sum_messages.push(openalpaca_llm::ChatMessage::user(&combined));

        let request = openalpaca_llm::routing::RouterRequest {
            messages: std::sync::Arc::new(sum_messages),
            tools: std::sync::Arc::new(vec![]),
            model: compaction_model,
            temperature: Some(0.0),
            max_tokens: Some(2048),
            context: req_context,
            tool_choice: None,
            tools_token_estimate: None,
            enable_caching: false,
            thinking: None,
            context_management: None,
        };

        let response = router.complete(request).await.map_err(|e| e.to_string())?;
        Ok(response.content)
    }
}
```

- [ ] **Step 7: Verify build**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: Clean build. The `dead_code` warnings for `ExtractedMemory`, `MemoryExtractor`, `Summarizer` should now be gone because `LlmBackend` uses them.

- [ ] **Step 8: Commit**

```bash
git add crates/openalpaca_core/src/runner/agentic_loop/backend.rs \
        crates/openalpaca_core/src/runner/agentic_loop/mod.rs
git commit -m "feat(compaction): implement MemoryExtractor + Summarizer on LlmBackend"
```

---

### Task 3: Improve heuristic `compress_context()`

**Files:**
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/context.rs:56-146`
- Modify: `crates/openalpaca_core/src/context_budget/compaction.rs:93`
- Modify: `crates/openalpaca_core/src/context_budget/tests.rs`

This task rewrites the `compress_context()` heuristic with: social discard, token-aware boundary, user message inclusion, round-grouped summaries, and budget-awareness.

- [ ] **Step 1: Write tests for the improved heuristic**

In `crates/openalpaca_core/src/context_budget/tests.rs`, add:

```rust
// ── Improved compress_context tests ─────────────────────────
#[test]
fn test_compress_context_includes_user_messages() {
    use openalpaca_llm::ChatMessage;
    let mut msgs = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("initial query"),
        ChatMessage::user("tell me about X"),
        ChatMessage::assistant("X is a concept that..."),
        ChatMessage::user("and what about Y?"),
        ChatMessage::assistant("Y relates to..."),
        // recent (tail_keep=1 → keep last 3)
        ChatMessage::user("final question"),
        ChatMessage::assistant("final answer"),
        ChatMessage::user("follow-up"),
    ];
    crate::runner::compress_context(&mut msgs, 1, None);
    // The summary should mention user messages "tell me about X" and "and what about Y?"
    let summary = &msgs[2].content;
    assert!(
        summary.contains("User") || summary.contains("tell me"),
        "Summary should include user messages: {summary}"
    );
}

#[test]
fn test_compress_context_social_discard_with_budget() {
    use openalpaca_llm::ChatMessage;
    use crate::daemon_config::ContextBudgetConfig;

    let budget = crate::context_budget::ContextBudgetManager::new(200_000, &ContextBudgetConfig::default());

    let mut msgs = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("initial query"),
        ChatMessage::user("thanks"),       // social
        ChatMessage::assistant("You're welcome!"), // paired with social
        ChatMessage::user("real question"),
        ChatMessage::assistant("real answer"),
        // recent
        ChatMessage::user("latest"),
        ChatMessage::assistant("latest response"),
        ChatMessage::user("more"),
    ];
    let before = msgs.len();
    crate::runner::compress_context(&mut msgs, 1, Some(&budget));
    // Social pair should be discarded, reducing message count
    assert!(msgs.len() < before, "Social messages should be discarded");
}

#[test]
fn test_compress_context_no_budget_legacy_behavior() {
    use openalpaca_llm::ChatMessage;
    let mut msgs = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("initial"),
        ChatMessage::user("q1"),
        ChatMessage::assistant("a1"),
        ChatMessage::user("q2"),
        ChatMessage::assistant("a2"),
        ChatMessage::user("q3"),
        ChatMessage::assistant("a3"),
        ChatMessage::user("recent"),
    ];
    crate::runner::compress_context(&mut msgs, 1, None);
    // Should still work with None budget (legacy path)
    assert!(msgs.len() < 9);
    assert_eq!(msgs[0].content, "sys");
    assert_eq!(msgs[1].content, "initial");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p openalpaca_core -- compress_context 2>&1 | tail -20`
Expected: Compilation error — `compress_context` doesn't accept 3 arguments yet.

- [ ] **Step 3: Rewrite `compress_context()`**

Replace the entire `compress_context` function in `crates/openalpaca_core/src/runner/agentic_loop/context.rs` (lines 56-146) with:

```rust
/// Compress context by replacing older rounds with a compact summary.
///
/// When `budget` is provided:
///   1. Discard social message pairs first (may be sufficient alone)
///   2. Use token-aware boundary to determine what to compress
///   3. Include user messages in summary (fixes previous omission)
///   4. Group summary by conversation rounds
///
/// When `budget` is `None`, uses legacy `tail_keep × 3` boundary.
pub(crate) fn compress_context(
    messages: &mut Vec<ChatMessage>,
    tail_keep: usize,
    budget: Option<&crate::context_budget::ContextBudgetManager>,
) {
    let min_recent = budget
        .map(|b| b.min_recent_messages())
        .unwrap_or(tail_keep * 3);

    // Phase 1: Social discard (always applied when budget is present)
    if budget.is_some() && messages.len() > 2 + min_recent {
        let cleaned =
            crate::context_budget::compaction::CompactionPipeline::discard_social(messages, min_recent);
        if cleaned.len() < messages.len() {
            tracing::debug!(
                discarded = messages.len() - cleaned.len(),
                "Heuristic: social messages discarded"
            );
            *messages = cleaned;
        }

        // Check if social discard alone was sufficient
        if let Some(b) = budget {
            let tokens_after = estimate_messages_tokens(messages) as usize;
            if !b.should_compact(tokens_after) {
                return; // Social discard was enough
            }
        }
    }

    // Phase 2: Determine compression boundary
    let keep_tail = if let Some(b) = budget {
        // Token-aware: walk backwards counting tokens until we hit the target
        let target = b.compaction_target_tokens();
        let mut tail_tokens = 0usize;
        let mut boundary = messages.len();
        for (i, msg) in messages.iter().enumerate().rev() {
            if i <= 1 {
                break; // Never compress system + initial query
            }
            let msg_tokens = if let Some(ref parts) = msg.parts {
                parts.iter().map(|p| estimate_part_tokens(p) as usize).sum()
            } else {
                msg.content.len() / 4
            };
            if tail_tokens + msg_tokens > target && boundary < messages.len() {
                break;
            }
            tail_tokens += msg_tokens;
            boundary = i;
        }
        messages.len() - boundary
    } else {
        // Legacy: fixed tail_keep × 3
        tail_keep * 3
    };

    if messages.len() <= 2 + keep_tail {
        return; // Nothing to compress
    }

    let compress_end = messages.len() - keep_tail;

    // Phase 3: Build round-grouped summary from messages[2..compress_end]
    let mut summary_parts = Vec::new();
    let mut round = 1u32;
    let mut current_round_parts: Vec<String> = Vec::new();

    for msg in &messages[2..compress_end] {
        // Handle multimodal parts
        if let Some(ref parts) = msg.parts {
            let role_label = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::System => "System",
                Role::Tool => "Tool",
            };
            for part in parts {
                let desc = match part {
                    ContentPart::Image { .. } => format!("{role_label}: [sent an image]"),
                    ContentPart::Audio { .. } => format!("{role_label}: [sent audio]"),
                    ContentPart::Document { filename, extracted_text, .. } => {
                        let excerpt = extracted_text
                            .as_ref()
                            .map(|t| truncate_for_summary(t, 150))
                            .unwrap_or_default();
                        format!("{role_label}: [attached: {filename}] {excerpt}")
                    }
                    ContentPart::FileRef { filename, .. } => {
                        format!("{role_label}: [attached: {filename}]")
                    }
                    ContentPart::Text { text } if !text.is_empty() => {
                        format!("{role_label}: {}", truncate_for_summary(text, 150))
                    }
                    _ => continue,
                };
                current_round_parts.push(format!("  {desc}"));
            }
            continue;
        }

        match msg.role {
            Role::User => {
                // Start a new round when we see a user message (except the first)
                if !current_round_parts.is_empty() {
                    summary_parts.push(format!("Round {round}:"));
                    summary_parts.extend(current_round_parts.drain(..));
                    round += 1;
                }
                current_round_parts.push(format!(
                    "  User: {}",
                    truncate_for_summary(&msg.content, 150)
                ));
            }
            Role::Assistant => {
                if !msg.content.is_empty() {
                    current_round_parts.push(format!(
                        "  Assistant: {}",
                        truncate_for_summary(&msg.content, 150)
                    ));
                }
                if let Some(ref tcs) = msg.tool_calls {
                    for tc in tcs {
                        current_round_parts.push(format!("  Called: {}", tc.name));
                    }
                }
            }
            Role::Tool => {
                current_round_parts.push(format!(
                    "  Result: {}",
                    truncate_for_summary(&msg.content, 100)
                ));
            }
            Role::System => {
                // Include system messages in summary (previously dropped)
                current_round_parts.push(format!(
                    "  System: {}",
                    truncate_for_summary(&msg.content, 100)
                ));
            }
        }
    }

    // Flush last round
    if !current_round_parts.is_empty() {
        summary_parts.push(format!("Round {round}:"));
        summary_parts.extend(current_round_parts);
    }

    let mut summary = format!(
        "[Context compressed: {} earlier messages in {} rounds]\n{}",
        compress_end - 2,
        round,
        summary_parts.join("\n")
    );

    // Cap summary size if budget is available
    if let Some(b) = budget {
        let max_summary_chars = b.compaction_target_tokens() * 4; // rough chars estimate
        if summary.len() > max_summary_chars {
            let end = summary.floor_char_boundary(max_summary_chars);
            summary.truncate(end);
            summary.push_str("\n[...summary truncated]");
        }
    }

    // Replace messages[2..compress_end] with the summary
    messages.splice(
        2..compress_end,
        std::iter::once(ChatMessage::user(&summary)),
    );
}
```

- [ ] **Step 4: Update the re-export in runner/mod.rs**

The function signature changed. Check `crates/openalpaca_core/src/runner/mod.rs` — it re-exports `compress_context`. The re-export is a `pub(crate) use` which forwards the new signature automatically. No change needed.

- [ ] **Step 5: Update the call in compaction.rs Phase 3 fallback**

In `crates/openalpaca_core/src/context_budget/compaction.rs`, line 93, change:

```rust
                crate::runner::compress_context(&mut fallback, tail_keep);
```

to:

```rust
                crate::runner::compress_context(&mut fallback, tail_keep, None);
```

- [ ] **Step 6: Update the call sites in agentic_loop/mod.rs (temporary — Task 4 overwrites)**

In `crates/openalpaca_core/src/runner/agentic_loop/mod.rs`, find the two `compress_context()` calls (around lines 285 and 302). Add the third argument to satisfy the new signature. Task 4 will replace this entire block, so these are just build-fixing placeholders:

Line ~285 (budget-aware block):
```rust
compress_context(Arc::make_mut(&mut messages), config.context_tail_keep, context_budget);
```

Line ~302 (legacy fallback):
```rust
compress_context(Arc::make_mut(&mut messages), config.context_tail_keep, None);
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p openalpaca_core -- compress_context --nocapture`
Expected: All 3 new tests + existing tests pass.

Run: `cargo check --all-targets`
Expected: Clean build.

- [ ] **Step 8: Commit**

```bash
git add crates/openalpaca_core/src/runner/agentic_loop/context.rs \
        crates/openalpaca_core/src/runner/agentic_loop/mod.rs \
        crates/openalpaca_core/src/context_budget/compaction.rs \
        crates/openalpaca_core/src/context_budget/tests.rs
git commit -m "feat(compaction): improve compress_context with social discard, token-aware boundary, user messages"
```

---

## Chunk 2: Pipeline Wiring + Sub-Agent Integration

### Task 4: Wire CompactionPipeline into the agentic loop

**Files:**
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/mod.rs:274-304`

This task replaces the compression block in the agentic loop with `CompactionPipeline::compact()` when LLM compaction is available, falling back to the improved heuristic.

**Note:** The spec requires `CompactionTriggered`/`CompactionPhaseCompleted` event emission, but the agentic loop does not have `EventBus` access (it's not in the function signature). Adding EventBus would be a cross-cutting signature change. For now, telemetry is emitted via `tracing::info!` which is captured by the tracing subscriber. Event bus integration is deferred to a follow-up task that threads `EventBus` into the loop.

- [ ] **Step 1: Replace the budget-aware compression block**

In `crates/openalpaca_core/src/runner/agentic_loop/mod.rs`, replace the entire budget-aware compression block (around lines 274-304) with:

```rust
            // ── 4. Context compression (budget-aware) ──────────────────
            if let Some(budget) = context_budget {
                let msg_tokens = estimate_messages_tokens(&messages) as usize;
                if budget.should_compact(msg_tokens) {
                    let messages_before = messages.len();

                    // Try LLM-based compaction if compaction model is available
                    let can_llm_compact = matches!(&backend, LlmBackend::Router { compaction_model: Some(_), .. });

                    if can_llm_compact {
                        tracing::info!(
                            agent_id = agent_id,
                            msg_tokens,
                            trigger = budget.compaction_trigger(),
                            messages_before,
                            "LLM compaction triggered"
                        );

                        // Extract messages from Arc for CompactionPipeline (takes Vec by value)
                        let owned = Arc::try_unwrap(messages)
                            .unwrap_or_else(|arc| (*arc).clone());

                        let result = crate::context_budget::compaction::CompactionPipeline::compact(
                            owned,
                            budget.min_recent_messages(),
                            &backend,
                            &backend,
                        )
                        .await;

                        // Log extracted memories (telemetry only — no DB storage)
                        for mem in &result.extracted_memories {
                            tracing::info!(
                                kind = %mem.kind,
                                preview = %crate::runner::agentic_loop::context::truncate_for_summary(&mem.content, 100),
                                "Compaction: extracted memory"
                            );
                        }

                        tracing::info!(
                            agent_id = agent_id,
                            messages_before,
                            messages_after = result.compacted_messages.len(),
                            memories_extracted = result.extracted_memories.len(),
                            messages_discarded = result.messages_discarded,
                            error = ?result.error,
                            "LLM compaction completed"
                        );

                        messages = Arc::new(result.compacted_messages);
                    } else {
                        // Heuristic fallback
                        tracing::info!(
                            agent_id = agent_id,
                            msg_tokens,
                            messages_before,
                            "Heuristic compaction triggered (no compaction model)"
                        );
                        compress_context(Arc::make_mut(&mut messages), config.context_tail_keep, Some(budget));
                        tracing::info!(
                            agent_id = agent_id,
                            messages_after = messages.len(),
                            "Heuristic compaction completed"
                        );
                    }

                    known_token_count = estimate_messages_tokens(&messages);
                }
            } else if config.max_context_tokens > 0 && known_token_count > config.max_context_tokens {
                // Legacy fallback (no budget manager)
                tracing::debug!(
                    agent_id = agent_id,
                    tokens = known_token_count,
                    max = config.max_context_tokens,
                    "Legacy compression triggered"
                );
                compress_context(Arc::make_mut(&mut messages), config.context_tail_keep, None);
                known_token_count = estimate_messages_tokens(&messages);
            }
```

- [ ] **Step 2: Make `truncate_for_summary` accessible**

The memory logging above needs `truncate_for_summary` from `context.rs`. It's currently private. Change its visibility from `fn truncate_for_summary(` to `pub(crate) fn truncate_for_summary(` in `context.rs`.

- [ ] **Step 3: Verify build**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: Clean build.

- [ ] **Step 4: Run all agentic loop tests**

Run: `cargo test -p openalpaca_core -- agentic_loop --nocapture`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/runner/agentic_loop/mod.rs \
        crates/openalpaca_core/src/runner/agentic_loop/context.rs
git commit -m "feat(compaction): wire CompactionPipeline into agentic loop with LLM + heuristic fallback"
```

---

### Task 5: Wire ContextBudgetManager into pipeline_step, node_runner, and lead_agent

**Files:**
- Modify: `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs:187-194,368-380`
- Modify: `crates/openalpaca_core/src/runner/dag_executor/node_runner.rs:44-47,175-187`
- Modify: `crates/openalpaca_core/src/runner/lead_agent/mod.rs:283-314`

This task instantiates `ContextBudgetManager` in all three execution paths and passes it to `run_agentic_loop_routed()`. Also sets `compaction_model` on `LoopConfig`.

- [ ] **Step 1: Wire pipeline_step.rs**

In `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs`, first change `let loop_config =` (line 187) to `let mut loop_config =`. Then after the `LoopConfig` construction (around line 194), add:

```rust
    // Set compaction model from daemon config
    loop_config.compaction_model = pctx.daemon_config.load()
        .execution.context.compaction_model.clone();

    // Instantiate ContextBudgetManager for budget-aware compaction
    let context_budget = {
        let default_model = pctx.router.default_model();
        let model_id = agent.llm_config.model.as_deref()
            .unwrap_or(&default_model);
        let context_window = pctx.router.model_registry()
            .get_model_info(model_id)
            .map(|info| info.context_window as usize)
            .unwrap_or(200_000);
        crate::context_budget::ContextBudgetManager::new(
            context_window,
            &pctx.daemon_config.load().execution.context,
        )
    };
```

Then change the `run_agentic_loop_routed()` call (around line 378), replacing `None, // context_budget` with:

```rust
    Some(&context_budget),
```

- [ ] **Step 2: Wire node_runner.rs**

In `crates/openalpaca_core/src/runner/dag_executor/node_runner.rs`, after `LoopConfig` construction (around line 47), add:

```rust
    // Set compaction model from daemon config
    loop_config.compaction_model = daemon_config.load()
        .execution.context.compaction_model.clone();

    // Instantiate ContextBudgetManager for budget-aware compaction
    let context_budget = {
        let default_model = router.default_model();
        let model_id = agent.llm_config.model.as_deref()
            .unwrap_or(&default_model);
        let context_window = router.model_registry()
            .get_model_info(model_id)
            .map(|info| info.context_window as usize)
            .unwrap_or(200_000);
        crate::context_budget::ContextBudgetManager::new(
            context_window,
            &daemon_config.load().execution.context,
        )
    };
```

Then change `run_agentic_loop_routed()` call (around line 184), replacing `None, // context_budget` with:

```rust
    Some(&context_budget),
```

- [ ] **Step 3: Wire lead_agent/mod.rs**

In `crates/openalpaca_core/src/runner/lead_agent/mod.rs`, first change `let loop_config =` (line 283) to `let mut loop_config =`. Then after `LoopConfig` construction (around line 290), add:

```rust
    // Set compaction model from daemon config
    loop_config.compaction_model = daemon_config.load()
        .execution.context.compaction_model.clone();

    // Instantiate ContextBudgetManager for budget-aware compaction
    let context_budget = {
        let default_model = router.default_model();
        let model_id = lead_agent.llm_config.model.as_deref()
            .unwrap_or(&default_model);
        let context_window = router.model_registry()
            .get_model_info(model_id)
            .map(|info| info.context_window as usize)
            .unwrap_or(200_000);
        crate::context_budget::ContextBudgetManager::new(
            context_window,
            &daemon_config.load().execution.context,
        )
    };
```

Then change `run_agentic_loop_routed()` call (around line 312), replacing `None, // context_budget` with:

```rust
    Some(&context_budget),
```

- [ ] **Step 4: Verify build**

Run: `cargo check --all-targets`
Expected: Clean build.

- [ ] **Step 5: Run tests**

Run: `cargo test -p openalpaca_core -- --nocapture 2>&1 | tail -5`
Expected: All 1026+ tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs \
        crates/openalpaca_core/src/runner/dag_executor/node_runner.rs \
        crates/openalpaca_core/src/runner/lead_agent/mod.rs
git commit -m "feat(compaction): wire ContextBudgetManager into pipeline_step, node_runner, lead_agent"
```

---

### Task 6: Final verification + clippy cleanup

**Files:** All modified files from Tasks 1-5.

- [ ] **Step 1: Full workspace build**

Run: `cargo check --all-targets`
Expected: Clean build.

- [ ] **Step 2: All openalpaca_core tests**

Run: `cargo test -p openalpaca_core`
Expected: All tests pass (1026+).

- [ ] **Step 3: All openalpaca_llm tests**

Run: `cargo test -p openalpaca_llm`
Expected: All tests pass (195+).

- [ ] **Step 4: Clippy on openalpaca_core**

Run: `cargo clippy -p openalpaca_core -- -D warnings 2>&1 | grep "error" | head -20`

Fix any new clippy errors introduced by this work. Pre-existing errors (collapsible_if in outcome.rs, invocation.rs, simple_query_handler.rs) should be the only remaining ones.

Expected dead_code warnings for `CompactionPipeline`, `ExtractedMemory`, `MemoryExtractor`, `Summarizer` should be **GONE** (now used by LlmBackend trait impls).

The `div_ceil` warning on `compaction.rs:92` (`(min_recent + 2) / 3`) should be fixed by changing to `min_recent.div_ceil(3)`.

- [ ] **Step 5: Fix div_ceil clippy warning**

In `crates/openalpaca_core/src/context_budget/compaction.rs`, change:

```rust
let tail_keep = ((min_recent + 2) / 3).max(1);
```

to:

```rust
let tail_keep = min_recent.div_ceil(3).max(1);
```

- [ ] **Step 6: Daemon build check**

Run: `cargo check -p openalpacad`
Expected: Clean build.

- [ ] **Step 7: Commit any fixes**

```bash
git add -u
git commit -m "chore: clippy fixes for compaction wiring"
```
