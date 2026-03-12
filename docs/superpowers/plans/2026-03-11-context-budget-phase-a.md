# Context Budget Phase A: Token Accounting Foundation

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `RenderedSection`, `ContextBudgetManager`, and `ContextBudgetConfig` with full test coverage. **No behavior changes** — budget is computed and logged but does not enforce.

**Architecture:** New `context_budget` module in `openalpaca_core` with token accounting. Sections report sizes via `RenderedSection`, `ContextBudgetManager` computes free zone and compaction trigger point. Observation wiring emits events for telemetry.

**Tech Stack:** Rust (edition 2024), serde, openalpaca_llm types, tracing.

**Spec:** `docs/superpowers/specs/2026-03-11-context-budget-design.md` — Sections 3.1–3.5, 6.1, 6.4, 6.5, 7.1 (ContextBudgetComputed event).

**Depends on:** Nothing (first phase).

---

## File Structure

| Action | Path | Purpose |
|--------|------|---------|
| Create | `crates/openalpaca_core/src/context_budget/mod.rs` | Module root, re-exports |
| Create | `crates/openalpaca_core/src/context_budget/budget.rs` | `RenderedSection`, `ContextBudgetManager` |
| Create | `crates/openalpaca_core/src/context_budget/tests.rs` | Unit tests |
| Modify | `crates/openalpaca_core/src/lib.rs` | Add `pub mod context_budget;` |
| Modify | `crates/openalpaca_core/src/daemon_config/execution.rs` | Add `ContextBudgetConfig` struct + field on `ExecutionConfig` |
| Modify | `config/daemon.toml` | Add `[execution.context]` section |
| Modify | `crates/openalpaca_core/src/events.rs` | Add `ContextBudgetComputed` variant |
| Modify | `apps/openalpacad/src/event_bridge.rs` | Add match arm for `ContextBudgetComputed` |
| Modify | `crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs` | Budget observation (compute + log, no enforcement) |

---

### Task 1: Create `context_budget` module with `RenderedSection`

**Files:**
- Create: `crates/openalpaca_core/src/context_budget/mod.rs`
- Create: `crates/openalpaca_core/src/context_budget/budget.rs`
- Create: `crates/openalpaca_core/src/context_budget/tests.rs`
- Modify: `crates/openalpaca_core/src/lib.rs`

- [ ] **Step 1: Write failing test**

```rust
// crates/openalpaca_core/src/context_budget/tests.rs
use super::budget::RenderedSection;

#[test]
fn test_rendered_section_creation() {
    let section = RenderedSection::new("Hello world".to_string());
    assert_eq!(section.content, "Hello world");
    // "Hello world" = 11 bytes / 4 = 2 tokens (integer division)
    assert_eq!(section.token_estimate, 2);
}

#[test]
fn test_rendered_section_empty() {
    let section = RenderedSection::new(String::new());
    assert_eq!(section.token_estimate, 0);
}

#[test]
fn test_rendered_section_with_explicit_tokens() {
    let section = RenderedSection::with_token_estimate("content".to_string(), 500);
    assert_eq!(section.token_estimate, 500);
    assert_eq!(section.content, "content");
}
```

- [ ] **Step 2: Implement `RenderedSection` + module structure**

```rust
// crates/openalpaca_core/src/context_budget/budget.rs

/// A rendered prompt section with its estimated token count.
#[derive(Debug, Clone)]
pub struct RenderedSection {
    pub content: String,
    pub token_estimate: usize,
}

impl RenderedSection {
    /// Create a new section, estimating tokens as `bytes / 4`.
    pub fn new(content: String) -> Self {
        let token_estimate = content.len() / 4;
        Self { content, token_estimate }
    }

    /// Create with an explicit token estimate (e.g., from API response).
    pub fn with_token_estimate(content: String, token_estimate: usize) -> Self {
        Self { content, token_estimate }
    }

    /// Empty section (zero tokens).
    pub fn empty() -> Self {
        Self { content: String::new(), token_estimate: 0 }
    }
}
```

```rust
// crates/openalpaca_core/src/context_budget/mod.rs
mod budget;

#[cfg(test)]
mod tests;

pub use budget::RenderedSection;
```

Add to `crates/openalpaca_core/src/lib.rs` after `pub mod context;`:

```rust
pub mod context_budget;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p openalpaca_core -- context_budget::tests --nocapture`
Expected: 3 tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/context_budget/ crates/openalpaca_core/src/lib.rs
git commit -m "feat(context_budget): add RenderedSection type (Phase A.1)"
```

---

### Task 2: Add `ContextBudgetConfig`

**Files:**
- Modify: `crates/openalpaca_core/src/daemon_config/execution.rs`
- Modify: `config/daemon.toml`
- Modify: `crates/openalpaca_core/src/context_budget/tests.rs`

- [ ] **Step 1: Write failing tests**

```rust
// Add to tests.rs
use crate::daemon_config::ContextBudgetConfig;

#[test]
fn test_context_budget_config_defaults() {
    let config = ContextBudgetConfig::default();
    assert!((config.autocompact_buffer_ratio - 0.165).abs() < f64::EPSILON);
    assert!((config.compaction_target_ratio - 0.50).abs() < f64::EPSILON);
    assert_eq!(config.compaction_model, None);
    assert_eq!(config.max_extractions_per_compaction, 10);
    assert_eq!(config.min_recent_messages, 4);
}

#[test]
fn test_context_budget_config_from_toml() {
    let toml_str = r#"
        autocompact_buffer_ratio = 0.20
        compaction_target_ratio = 0.60
        compaction_model = "claude-haiku-4-5-20251001"
        max_extractions_per_compaction = 5
        min_recent_messages = 6
    "#;
    let config: ContextBudgetConfig = toml::from_str(toml_str).unwrap();
    assert!((config.autocompact_buffer_ratio - 0.20).abs() < f64::EPSILON);
    assert_eq!(config.compaction_model.as_deref(), Some("claude-haiku-4-5-20251001"));
    assert_eq!(config.max_extractions_per_compaction, 5);
}
```

- [ ] **Step 2: Run tests — verify they fail**

Run: `cargo test -p openalpaca_core -- context_budget::tests::test_context_budget_config --nocapture`
Expected: FAIL — `ContextBudgetConfig` not found

- [ ] **Step 3: Implement**

Add after `DagConfig` in `crates/openalpaca_core/src/daemon_config/execution.rs` (after line 206):

```rust
/// Context budget and compaction configuration.
///
/// Controls autocompact buffer sizing, compaction target, and extraction limits.
/// Deserialized from `[execution.context]` in daemon.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextBudgetConfig {
    pub autocompact_buffer_ratio: f64,
    pub compaction_target_ratio: f64,
    pub compaction_model: Option<String>,
    pub max_extractions_per_compaction: usize,
    pub min_recent_messages: usize,
}

impl Default for ContextBudgetConfig {
    fn default() -> Self {
        Self {
            autocompact_buffer_ratio: 0.165,
            compaction_target_ratio: 0.50,
            compaction_model: None,
            max_extractions_per_compaction: 10,
            min_recent_messages: 4,
        }
    }
}
```

Add `context` field to `ExecutionConfig` struct (line ~6-12):

```rust
pub context: ContextBudgetConfig,
```

Add to `config/daemon.toml` under `[execution]`:

```toml
[execution.context]
autocompact_buffer_ratio = 0.165
compaction_target_ratio = 0.50
# compaction_model = "claude-haiku-4-5-20251001"
max_extractions_per_compaction = 10
min_recent_messages = 4
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p openalpaca_core -- context_budget::tests --nocapture`
Expected: 5 tests PASS

Also: `cargo check -p openalpaca_core --all-targets`

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/daemon_config/execution.rs config/daemon.toml crates/openalpaca_core/src/context_budget/tests.rs
git commit -m "feat(context_budget): add ContextBudgetConfig (Phase A.2)"
```

---

### Task 3: Implement `ContextBudgetManager`

**Files:**
- Modify: `crates/openalpaca_core/src/context_budget/budget.rs`
- Modify: `crates/openalpaca_core/src/context_budget/mod.rs`
- Modify: `crates/openalpaca_core/src/context_budget/tests.rs`

- [ ] **Step 1: Write failing tests**

```rust
use super::budget::ContextBudgetManager;
use crate::daemon_config::ContextBudgetConfig;

#[test]
fn test_budget_computation_basic() {
    let config = ContextBudgetConfig::default();
    let mgr = ContextBudgetManager::new(200_000, &config);
    assert_eq!(mgr.model_context_window(), 200_000);
    assert_eq!(mgr.autocompact_buffer(), 33_000);
    assert_eq!(mgr.fixed_zone_tokens(), 0);
    assert_eq!(mgr.free_zone_capacity(), 167_000);
}

#[test]
fn test_budget_with_fixed_sections() {
    let config = ContextBudgetConfig::default();
    let mut mgr = ContextBudgetManager::new(200_000, &config);
    mgr.register_section("system_prompt", 5_000);
    mgr.register_section("tools", 3_000);
    mgr.register_section("memory", 1_000);
    assert_eq!(mgr.fixed_zone_tokens(), 9_000);
    assert_eq!(mgr.free_zone_capacity(), 158_000);
}

#[test]
fn test_budget_various_models() {
    let config = ContextBudgetConfig::default();
    assert_eq!(ContextBudgetManager::new(8_192, &config).autocompact_buffer(), 1_351);
    assert_eq!(ContextBudgetManager::new(128_000, &config).autocompact_buffer(), 21_120);
    assert_eq!(ContextBudgetManager::new(200_000, &config).autocompact_buffer(), 33_000);
}

#[test]
fn test_compaction_trigger_threshold() {
    let config = ContextBudgetConfig::default();
    let mut mgr = ContextBudgetManager::new(200_000, &config);
    mgr.register_section("system_prompt", 5_000);
    // compaction_trigger = 200K - 33K = 167K
    // should_compact fires when fixed(5K) + msg_tokens >= 167K => msg_tokens >= 162K
    assert!(!mgr.should_compact(161_999));
    assert!(mgr.should_compact(162_000));
    assert!(mgr.should_compact(170_000));
}

#[test]
fn test_compaction_not_triggered_below_threshold() {
    let config = ContextBudgetConfig::default();
    let mgr = ContextBudgetManager::new(200_000, &config);
    assert!(!mgr.should_compact(0));
    assert!(!mgr.should_compact(100_000));
    assert!(!mgr.should_compact(166_999));
}

#[test]
fn test_autocompact_buffer_ratio_config() {
    let mut config = ContextBudgetConfig::default();
    config.autocompact_buffer_ratio = 0.25;
    let mgr = ContextBudgetManager::new(100_000, &config);
    assert_eq!(mgr.autocompact_buffer(), 25_000);
    assert_eq!(mgr.free_zone_capacity(), 75_000);
}

#[test]
fn test_fixed_zone_overflow_warning() {
    let config = ContextBudgetConfig::default();
    let mut mgr = ContextBudgetManager::new(10_000, &config);
    mgr.register_section("huge_prompt", 6_000);
    assert!(mgr.is_fixed_zone_oversized());
}

#[test]
fn test_section_breakdown() {
    let config = ContextBudgetConfig::default();
    let mut mgr = ContextBudgetManager::new(200_000, &config);
    mgr.register_section("system_prompt", 5_000);
    mgr.register_section("tools", 3_000);
    let breakdown = mgr.section_breakdown();
    assert_eq!(breakdown.len(), 2);
    // Insertion order is guaranteed (Vec-backed)
    assert_eq!(breakdown[0], ("system_prompt", 5_000));
    assert_eq!(breakdown[1], ("tools", 3_000));
}
```

- [ ] **Step 2: Run tests — verify they fail**

- [ ] **Step 3: Implement `ContextBudgetManager`**

Add to `crates/openalpaca_core/src/context_budget/budget.rs`:

```rust
use crate::daemon_config::ContextBudgetConfig;

/// Manages token budget accounting for a single context window.
///
/// One instance per request (orchestrator) or per sub-agent loop.
#[derive(Debug)]
pub struct ContextBudgetManager {
    model_context_window: usize,
    autocompact_buffer: usize,
    compaction_target_ratio: f64,
    min_recent_messages: usize,
    sections: Vec<(&'static str, usize)>,
}

impl ContextBudgetManager {
    pub fn new(model_context_window: usize, config: &ContextBudgetConfig) -> Self {
        let autocompact_buffer =
            (model_context_window as f64 * config.autocompact_buffer_ratio) as usize;
        Self {
            model_context_window,
            autocompact_buffer,
            compaction_target_ratio: config.compaction_target_ratio,
            min_recent_messages: config.min_recent_messages,
            sections: Vec::new(),
        }
    }

    pub fn register_section(&mut self, name: &'static str, tokens: usize) {
        self.sections.push((name, tokens));
    }

    pub fn model_context_window(&self) -> usize { self.model_context_window }
    pub fn autocompact_buffer(&self) -> usize { self.autocompact_buffer }

    pub fn fixed_zone_tokens(&self) -> usize {
        self.sections.iter().map(|(_, t)| t).sum()
    }

    pub fn free_zone_capacity(&self) -> usize {
        self.model_context_window
            .saturating_sub(self.autocompact_buffer)
            .saturating_sub(self.fixed_zone_tokens())
    }

    /// Total input tokens at which compaction fires (window - buffer).
    pub fn compaction_trigger(&self) -> usize {
        self.model_context_window.saturating_sub(self.autocompact_buffer)
    }

    pub fn should_compact(&self, message_tokens: usize) -> bool {
        self.fixed_zone_tokens() + message_tokens >= self.compaction_trigger()
    }

    pub fn compaction_target_tokens(&self) -> usize {
        (self.free_zone_capacity() as f64 * self.compaction_target_ratio) as usize
    }

    pub fn min_recent_messages(&self) -> usize { self.min_recent_messages }

    pub fn is_fixed_zone_oversized(&self) -> bool {
        self.fixed_zone_tokens() > self.model_context_window / 2
    }

    pub fn section_breakdown(&self) -> Vec<(&'static str, usize)> {
        self.sections.clone()
    }
}
```

Update `mod.rs`:

```rust
pub use budget::{ContextBudgetManager, RenderedSection};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p openalpaca_core -- context_budget::tests --nocapture`
Expected: All 13 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/context_budget/
git commit -m "feat(context_budget): add ContextBudgetManager (Phase A.3)"
```

---

### Task 4: Add `ContextBudgetComputed` SystemEvent + event_bridge arm

**Files:**
- Modify: `crates/openalpaca_core/src/events.rs`
- Modify: `apps/openalpacad/src/event_bridge.rs`

**IMPORTANT:** `event_bridge.rs` has an exhaustive match on `SystemEvent` with no catch-all (line 491). Every new variant MUST get a match arm or the daemon crate won't compile.

- [ ] **Step 1: Add event variant to `events.rs`**

Add after `ToolConfirmationRequested` (line 379):

```rust
    /// Context budget was computed for a request (Phase A observability)
    ContextBudgetComputed {
        request_id: Uuid,
        model: String,
        window_size: usize,
        fixed_zone_tokens: usize,
        free_zone_tokens: usize,
        buffer_size: usize,
        section_breakdown: Vec<(String, usize)>,
        timestamp: DateTime<Utc>,
    },
```

- [ ] **Step 2: Add match arm in `event_bridge.rs`**

In `apps/openalpacad/src/event_bridge.rs`, add before the closing `}` of the match (before line 491):

```rust
                openalpaca_core::events::SystemEvent::ContextBudgetComputed {
                    request_id, model, window_size, fixed_zone_tokens, free_zone_tokens, buffer_size, ..
                } => {
                    tracing::debug!(
                        %request_id, %model, window_size, fixed_zone_tokens, free_zone_tokens, buffer_size,
                        "Context budget computed"
                    );
                }
```

- [ ] **Step 3: Verify full workspace build**

Run: `cargo check --all-targets`
Expected: Clean (this checks openalpacad too)

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/events.rs apps/openalpacad/src/event_bridge.rs
git commit -m "feat(context_budget): add ContextBudgetComputed event (Phase A.4)"
```

---

### Task 5: Wire budget observation into `simple_query_handler`

Computes budget and emits event. **No enforcement** — existing behavior unchanged.

**Files:**
- Modify: `crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs`

- [ ] **Step 1: Read the current file**

Read `simple_query_handler.rs` fully. Identify:
- Where `self.llm_router` is unwrapped (the `if let Some(ref router) = self.llm_router` block)
- Where `tools_for_loop` is defined
- Where `run_agentic_loop_routed` is called
- The exact insertion point: after message assembly, before sandbox creation, **inside** the `if let Some(ref router)` block

- [ ] **Step 2: Add budget observation**

Insert **inside** the `if let Some(ref router) = self.llm_router` block, after message assembly (after memory injection), before `run_agentic_loop_routed`:

```rust
// --- Context Budget Observation (Phase A) ---
{
    let ctx_config = &self.daemon_config.load().execution.context;
    let model_id = config_for_loop.model.as_deref();
    let model_window = model_id
        .and_then(|m| router.model_registry().get_model_info(m))
        .map(|info| info.context_window as usize)
        .unwrap_or(200_000);

    let mut budget = crate::context_budget::ContextBudgetManager::new(model_window, ctx_config);
    budget.register_section("system_prompt", system_prompt.len() / 4);
    budget.register_section("tools", tools_for_loop.len() * 200); // rough placeholder

    if budget.is_fixed_zone_oversized() {
        tracing::warn!(
            request_id = %request_id,
            fixed_zone = budget.fixed_zone_tokens(),
            window = model_window,
            "Fixed zone exceeds 50% of context window"
        );
    }

    tracing::debug!(
        request_id = %request_id,
        model_window,
        fixed_zone = budget.fixed_zone_tokens(),
        free_zone = budget.free_zone_capacity(),
        buffer = budget.autocompact_buffer(),
        "Context budget computed"
    );

    self.bus.publish(crate::events::SystemEvent::ContextBudgetComputed {
        request_id,
        model: model_id.unwrap_or("default").to_string(),
        window_size: model_window,
        fixed_zone_tokens: budget.fixed_zone_tokens(),
        free_zone_tokens: budget.free_zone_capacity(),
        buffer_size: budget.autocompact_buffer(),
        section_breakdown: budget.section_breakdown()
            .into_iter()
            .map(|(n, t)| (n.to_string(), t))
            .collect(),
        timestamp: chrono::Utc::now(),
    });
}
```

If `self.llm_router` is `None` (echo stub mode), add in the else branch:

```rust
tracing::debug!(request_id = %request_id, "Context budget observation skipped: no LLM router");
```

- [ ] **Step 3: Verify build + existing tests pass**

Run: `cargo check --all-targets && cargo test -p openalpaca_core -- query_handler --nocapture`

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs
git commit -m "feat(context_budget): wire budget observation into query handler (Phase A.5)"
```

---

### Task 6: Phase A verification

- [ ] **Step 1: Full workspace build** — `cargo check --all-targets`
- [ ] **Step 2: Full test suite** — `cargo test -p openalpaca_core`
- [ ] **Step 3: Clippy** — `cargo clippy -p openalpaca_core -- -D warnings`
- [ ] **Step 4: Verify event_bridge compiles** — `cargo check -p openalpacad`
