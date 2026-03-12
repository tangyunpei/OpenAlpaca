# Context Budget Phase C: Sub-Agent Context Distillation

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enrich pipeline/DAG sub-agent prompts with distilled context via `ContextPackage`. Sub-agents get relevant memories, conversation summary, and user context — not raw history.

**Architecture:** `ContextPackageBuilder` in `context_budget/package.rs` constructs minimum-exposure packages. `AgentConstraints` gains `denied_sections` + `max_context_tokens` for per-agent information barriers. Pipeline step and DAG node runner assemble packages and inject into sub-agent prompts.

**Tech Stack:** Rust (edition 2024), serde, openalpaca_llm types, openalpaca_storage memory search.

**Spec:** `docs/superpowers/specs/2026-03-11-context-budget-design.md` — Sections 5.1–5.5, 6.1, 7.1 (ContextPackageBuilt event).

**Depends on:** Phase A (context_budget module, ContextBudgetManager must exist). Phase B is optional — compaction pipeline is independent from distillation.

**Pre-flight check:** Before starting Task 2, verify Phase A artifacts exist: `crates/openalpaca_core/src/context_budget/mod.rs` must exist and `lib.rs` must contain `pub mod context_budget;`. If not, execute Phase A first.

---

## File Structure

| Action | Path | Purpose |
|--------|------|---------|
| Create | `crates/openalpaca_core/src/context_budget/package.rs` | `ContextPackage`, `ContextPackageBuilder` |
| Modify | `crates/openalpaca_core/src/context_budget/mod.rs` | Add package submodule + re-exports |
| Modify | `crates/openalpaca_core/src/context_budget/tests.rs` | Package unit tests |
| Modify | `crates/openalpaca_core/src/agent/subagent/mod.rs` | Add `denied_sections`, `max_context_tokens` to `AgentConstraints` |
| Modify | `crates/openalpaca_core/src/events.rs` | Add `ContextPackageBuilt` variant |
| Modify | `apps/openalpacad/src/event_bridge.rs` | Add match arm for `ContextPackageBuilt` |
| Modify | `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs` | Build + inject ContextPackage |
| Modify | `crates/openalpaca_core/src/runner/dag_executor/node_runner.rs` | Build + inject ContextPackage |

---

### Task 1: Extend `AgentConstraints` with distillation fields

**Files:**
- Modify: `crates/openalpaca_core/src/agent/subagent/mod.rs`

- [ ] **Step 1: Read the current file**

Read `crates/openalpaca_core/src/agent/subagent/mod.rs`. Locate:
- `AgentConstraints` struct (lines ~104-125)
- `normalize()` method (lines ~130-143)

- [ ] **Step 2: Add fields to `AgentConstraints`**

Add after `auto_approve: bool` (before the closing `}`):

```rust
    /// Sections to exclude from ContextPackage (e.g. ["conversation_summary", "user_context"]).
    #[serde(default)]
    pub denied_sections: Vec<String>,
    /// Maximum total context tokens for this agent. Overrides model default if set.
    #[serde(default)]
    pub max_context_tokens: Option<usize>,
```

- [ ] **Step 3: Add `denied_sections` to `normalize()`**

In the `normalize()` method, add after the `denied_models` loop:

```rust
    for s in &mut self.denied_sections {
        *s = s.to_lowercase();
    }
```

- [ ] **Step 4: Verify build**

Run: `cargo check -p openalpaca_core --all-targets`

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/agent/subagent/mod.rs
git commit -m "feat(context_budget): add denied_sections + max_context_tokens to AgentConstraints (Phase C.1)"
```

---

### Task 2: Implement `ContextPackage` and `ContextPackageBuilder`

**Files:**
- Create: `crates/openalpaca_core/src/context_budget/package.rs`
- Modify: `crates/openalpaca_core/src/context_budget/mod.rs`
- Modify: `crates/openalpaca_core/src/context_budget/tests.rs`

**Known sections** that can appear in `denied_sections`:
- `conversation_summary`
- `relevant_memories`
- `user_context`
- `workspace_artifacts`

- [ ] **Step 1: Write failing tests**

```rust
// Add to tests.rs
use super::package::{ContextPackage, ContextPackageBuilder};

#[test]
fn test_context_package_always_has_task() {
    let pkg = ContextPackageBuilder::new("Analyze the logs".to_string()).build();
    assert_eq!(pkg.task_description, "Analyze the logs");
}

#[test]
fn test_context_package_includes_optional_sections() {
    let pkg = ContextPackageBuilder::new("Fix the bug".to_string())
        .conversation_summary("User reported a crash on login".to_string())
        .user_context("Prefers verbose logging".to_string())
        .workspace_artifact("Agent A found NPE in auth.rs line 42".to_string())
        .build();
    assert!(pkg.conversation_summary.is_some());
    assert!(pkg.user_context.is_some());
    assert_eq!(pkg.workspace_artifacts.len(), 1);
}

#[test]
fn test_context_package_denied_sections() {
    let denied = vec!["conversation_summary".to_string(), "user_context".to_string()];
    let pkg = ContextPackageBuilder::new("Task".to_string())
        .conversation_summary("summary".to_string())
        .user_context("prefs".to_string())
        .workspace_artifact("artifact".to_string())
        .denied_sections(&denied)
        .build();
    assert!(pkg.conversation_summary.is_none());
    assert!(pkg.user_context.is_none());
    // workspace_artifacts not denied, so kept
    assert_eq!(pkg.workspace_artifacts.len(), 1);
}

#[test]
fn test_context_package_denied_sections_case_insensitive() {
    let denied = vec!["Conversation_Summary".to_string()];
    let pkg = ContextPackageBuilder::new("Task".to_string())
        .conversation_summary("summary".to_string())
        .denied_sections(&denied)
        .build();
    assert!(pkg.conversation_summary.is_none());
}

#[test]
fn test_context_package_minimum_exposure() {
    // Builder with nothing optional -> only task_description
    let pkg = ContextPackageBuilder::new("Pure compute".to_string()).build();
    assert!(pkg.conversation_summary.is_none());
    assert!(pkg.relevant_memories.is_empty());
    assert!(pkg.user_context.is_none());
    assert!(pkg.workspace_artifacts.is_empty());
}

#[test]
fn test_context_package_format_for_prompt() {
    let pkg = ContextPackageBuilder::new("Fix the bug".to_string())
        .conversation_summary("User saw a crash".to_string())
        .workspace_artifact("Previous analysis output".to_string())
        .build();
    let prompt = pkg.format_for_prompt();
    assert!(prompt.contains("Fix the bug"));
    assert!(prompt.contains("User saw a crash"));
    assert!(prompt.contains("Previous analysis output"));
}

#[test]
fn test_context_package_sections_included() {
    let pkg = ContextPackageBuilder::new("Task".to_string())
        .conversation_summary("summary".to_string())
        .user_context("prefs".to_string())
        .build();
    let sections = pkg.sections_included();
    assert!(sections.contains(&"task_description"));
    assert!(sections.contains(&"conversation_summary"));
    assert!(sections.contains(&"user_context"));
    assert!(!sections.contains(&"workspace_artifacts"));
    assert!(!sections.contains(&"relevant_memories"));
}
```

- [ ] **Step 2: Run tests — verify they fail**

Run: `cargo test -p openalpaca_core -- context_budget::tests::test_context_package --nocapture`
Expected: FAIL — module `package` not found

- [ ] **Step 3: Implement**

```rust
// crates/openalpaca_core/src/context_budget/package.rs

/// Known section names for validation against `denied_sections`.
pub const KNOWN_SECTIONS: &[&str] = &[
    "conversation_summary",
    "relevant_memories",
    "user_context",
    "workspace_artifacts",
];

/// A minimum-exposure context package for sub-agent prompt assembly.
#[derive(Debug, Clone)]
pub struct ContextPackage {
    pub task_description: String,
    pub conversation_summary: Option<String>,
    pub relevant_memories: Vec<String>,
    pub user_context: Option<String>,
    pub workspace_artifacts: Vec<String>,
}

impl ContextPackage {
    /// Format the package as a prompt string for injection into the sub-agent's messages.
    pub fn format_for_prompt(&self) -> String {
        let mut parts = Vec::new();

        parts.push(format!("<assignment>\n{}\n</assignment>", self.task_description));

        if let Some(ref summary) = self.conversation_summary {
            parts.push(format!("<conversation-context>\n{}\n</conversation-context>", summary));
        }

        if !self.relevant_memories.is_empty() {
            let mem_block = self.relevant_memories.join("\n- ");
            parts.push(format!("<relevant-memories>\n- {}\n</relevant-memories>", mem_block));
        }

        if let Some(ref ctx) = self.user_context {
            parts.push(format!("<user-context>\n{}\n</user-context>", ctx));
        }

        if !self.workspace_artifacts.is_empty() {
            for (i, artifact) in self.workspace_artifacts.iter().enumerate() {
                parts.push(format!(
                    "<workspace-artifact index=\"{}\">\n{}\n</workspace-artifact>",
                    i, artifact
                ));
            }
        }

        parts.join("\n\n")
    }

    /// List which sections are present (for telemetry).
    pub fn sections_included(&self) -> Vec<&'static str> {
        let mut sections = vec!["task_description"];
        if self.conversation_summary.is_some() {
            sections.push("conversation_summary");
        }
        if !self.relevant_memories.is_empty() {
            sections.push("relevant_memories");
        }
        if self.user_context.is_some() {
            sections.push("user_context");
        }
        if !self.workspace_artifacts.is_empty() {
            sections.push("workspace_artifacts");
        }
        sections
    }

    /// Estimate total token count (bytes / 4 heuristic).
    pub fn estimated_tokens(&self) -> usize {
        self.format_for_prompt().len() / 4
    }
}

/// Builder for `ContextPackage` with `denied_sections` enforcement.
pub struct ContextPackageBuilder {
    task_description: String,
    conversation_summary: Option<String>,
    relevant_memories: Vec<String>,
    user_context: Option<String>,
    workspace_artifacts: Vec<String>,
    denied_sections: Vec<String>,
}

impl ContextPackageBuilder {
    pub fn new(task_description: String) -> Self {
        Self {
            task_description,
            conversation_summary: None,
            relevant_memories: Vec::new(),
            user_context: None,
            workspace_artifacts: Vec::new(),
            denied_sections: Vec::new(),
        }
    }

    pub fn conversation_summary(mut self, summary: String) -> Self {
        self.conversation_summary = Some(summary);
        self
    }

    pub fn relevant_memory(mut self, memory: String) -> Self {
        self.relevant_memories.push(memory);
        self
    }

    pub fn user_context(mut self, ctx: String) -> Self {
        self.user_context = Some(ctx);
        self
    }

    pub fn workspace_artifact(mut self, artifact: String) -> Self {
        self.workspace_artifacts.push(artifact);
        self
    }

    pub fn denied_sections(mut self, denied: &[String]) -> Self {
        self.denied_sections = denied.iter().map(|s| s.to_lowercase()).collect();
        self
    }

    pub fn build(self) -> ContextPackage {
        let is_denied = |section: &str| self.denied_sections.contains(&section.to_lowercase());

        ContextPackage {
            task_description: self.task_description,
            conversation_summary: if is_denied("conversation_summary") {
                None
            } else {
                self.conversation_summary
            },
            relevant_memories: if is_denied("relevant_memories") {
                Vec::new()
            } else {
                self.relevant_memories
            },
            user_context: if is_denied("user_context") {
                None
            } else {
                self.user_context
            },
            workspace_artifacts: if is_denied("workspace_artifacts") {
                Vec::new()
            } else {
                self.workspace_artifacts
            },
        }
    }
}
```

Update `context_budget/mod.rs`:

```rust
pub(crate) mod package;

pub use package::{ContextPackage, ContextPackageBuilder, KNOWN_SECTIONS};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p openalpaca_core -- context_budget::tests --nocapture`
Expected: All package tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/context_budget/
git commit -m "feat(context_budget): add ContextPackage + ContextPackageBuilder (Phase C.2)"
```

---

### Task 3: Add `ContextPackageBuilt` SystemEvent + event_bridge arm

**Files:**
- Modify: `crates/openalpaca_core/src/events.rs`
- Modify: `apps/openalpacad/src/event_bridge.rs`

**IMPORTANT:** `event_bridge.rs` has an exhaustive match on `SystemEvent` with no catch-all. Every new variant MUST get a match arm or the daemon crate won't compile.

- [ ] **Step 1: Add event variant**

In `events.rs`, add after the compaction events (or after `ContextBudgetComputed` if Phase B not yet done):

```rust
    /// Context package built for sub-agent dispatch
    ContextPackageBuilt {
        request_id: Uuid,
        agent_id: String,
        sections_included: Vec<String>,
        total_tokens: usize,
        memories_count: usize,
        timestamp: DateTime<Utc>,
    },
```

- [ ] **Step 2: Add match arm in `event_bridge.rs`**

```rust
openalpaca_core::events::SystemEvent::ContextPackageBuilt {
    request_id, ref agent_id, ref sections_included, total_tokens, memories_count, ..
} => {
    tracing::debug!(
        %request_id, %agent_id, ?sections_included, total_tokens, memories_count,
        "Context package built for sub-agent"
    );
}
```

- [ ] **Step 3: Verify full workspace build**

Run: `cargo check --all-targets`

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/events.rs apps/openalpacad/src/event_bridge.rs
git commit -m "feat(context_budget): add ContextPackageBuilt event (Phase C.3)"
```

---

### Task 4: Wire `ContextPackage` into `pipeline_step.rs`

**Files:**
- Modify: `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs`

**Current signature reference:**
```rust
pub(super) async fn execute_pipeline_step(
    pctx: &PipelineStepContext,
    step: usize,
    agent: &SubAgent,
    assignment_id: Option<&String>,
    role_description: &str,       // <-- this is the task description
    previous_output: &Option<String>,  // <-- &Option<String>, NOT iterable
    cached_workspace_context: &str,
) -> PipelineStepResult
```

**What changes:**
- After building the system prompt (~line 207-240), build a `ContextPackage` from the available data
- Inject `pkg.format_for_prompt()` as an additional user message after system prompt and before existing messages
- Emit `ContextPackageBuilt` event

**What stays the same:**
- System prompt assembly (persona, role_description, tool guidance)
- Existing workspace context handling (backward compat with `previous_output`)
- `run_agentic_loop_routed` call

- [ ] **Step 1: Read the current file**

Read `pipeline_step.rs` fully. Identify:
- Where system prompt is built (lines ~207-240) — **the `let system_prompt = format!(...)` binding must be changed to `let mut system_prompt`** so we can append package sections
- Where messages are assembled (after system prompt, before `run_agentic_loop_routed`)
- Where memory retrieval happens for step 0 (~lines 246-272)
- The exact line where you'll insert the package builder

- [ ] **Step 1.5: Change `system_prompt` to mutable**

Change `let system_prompt = format!(...)` to `let mut system_prompt = format!(...)` at the system prompt construction site (~line 221).

- [ ] **Step 2: Build ContextPackage and inject**

Insert after system prompt assembly, before messages vector creation:

```rust
// --- Context Package (Phase C) ---
let denied_sections = agent.constraints.denied_sections.clone();

let mut pkg_builder = crate::context_budget::ContextPackageBuilder::new(
    role_description.to_string(),
);

// Add workspace artifacts from previous_output
if let Some(ref output) = *previous_output {
    pkg_builder = pkg_builder.workspace_artifact(output.clone());
}

// Add cached workspace context as artifact if non-empty
if !cached_workspace_context.is_empty() {
    pkg_builder = pkg_builder.workspace_artifact(cached_workspace_context.to_string());
}

// Apply denied_sections from agent constraints
if !denied_sections.is_empty() {
    pkg_builder = pkg_builder.denied_sections(&denied_sections);
}

let context_package = pkg_builder.build();

// Emit telemetry — compute tokens from only the optional sections actually
// injected (not task_description, which is already in the <assignment> block)
let injected_sections_tokens = {
    let mut t = 0usize;
    if let Some(ref s) = context_package.conversation_summary { t += s.len() / 4 + 20; }
    for m in &context_package.relevant_memories { t += m.len() / 4 + 10; }
    if let Some(ref s) = context_package.user_context { t += s.len() / 4 + 20; }
    for a in &context_package.workspace_artifacts { t += a.len() / 4 + 20; }
    t
};
pctx.bus.publish(crate::events::SystemEvent::ContextPackageBuilt {
    request_id: uuid::Uuid::new_v4(),
    agent_id: agent.id.clone(),
    sections_included: context_package.sections_included()
        .into_iter()
        .map(|s| s.to_string())
        .collect(),
    total_tokens: injected_sections_tokens,
    memories_count: context_package.relevant_memories.len(),
    timestamp: chrono::Utc::now(),
});
```

**Note:** The current prompt assembly already includes `role_description` in the `<assignment>` block and workspace context in the `<workspace>` block. The `ContextPackage` enriches this — it does NOT replace the existing assembly. Instead, inject the package's optional sections (conversation_summary, relevant_memories, user_context) as additional blocks in the system prompt:

```rust
// Append context package optional sections to system_prompt
let package_sections = context_package.format_for_prompt();
// Only append non-task sections (task is already in <assignment>)
if context_package.conversation_summary.is_some()
    || !context_package.relevant_memories.is_empty()
    || context_package.user_context.is_some()
{
    system_prompt.push_str("\n\n");
    if let Some(ref summary) = context_package.conversation_summary {
        system_prompt.push_str(&format!("<conversation-context>\n{}\n</conversation-context>\n\n", summary));
    }
    if !context_package.relevant_memories.is_empty() {
        let mem_block = context_package.relevant_memories.join("\n- ");
        system_prompt.push_str(&format!("<relevant-memories>\n- {}\n</relevant-memories>\n\n", mem_block));
    }
    if let Some(ref ctx) = context_package.user_context {
        system_prompt.push_str(&format!("<user-context>\n{}\n</user-context>\n\n", ctx));
    }
}
```

- [ ] **Step 3: Verify build + existing tests**

Run: `cargo check --all-targets && cargo test -p openalpaca_core -- dispatcher --nocapture`

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs
git commit -m "feat(context_budget): wire ContextPackage into pipeline step (Phase C.4)"
```

---

### Task 5: Wire `ContextPackage` into `node_runner.rs`

**Files:**
- Modify: `crates/openalpaca_core/src/runner/dag_executor/node_runner.rs`

**Current signature reference:**
```rust
pub(super) async fn execute_single_node(
    node: DagNode,                          // node.description is the task
    router: Arc<LlmRouter>,
    tool_registry: Arc<ToolRegistry>,
    bus: EventBus,
    ...
    workspace_snapshot: Arc<Option<TaskState>>,
    ...
) -> NodeResult
```

**DagNode fields:**
- `node.title: String`
- `node.description: String`  (NOT `node.assignment`)
- `node.agent_id: String`
- `node.workspace_keys: Vec<String>`

**Workspace context access:**
- `state.workspace.format_for_prompt(&node.workspace_keys)` when snapshot is Some
- Falls back to `super::progress::load_workspace_context()`

- [ ] **Step 1: Read the current file**

Read `node_runner.rs` fully. Identify:
- Where the system prompt is assembled (~line 64) — **the `let system_prompt = format!(...)` binding must be changed to `let mut system_prompt`** so we can append package sections
- Where workspace context is loaded (lines ~90-94)
- Where messages are built
- The exact insertion point

- [ ] **Step 1.5: Change `system_prompt` to mutable**

Change `let system_prompt = format!(...)` to `let mut system_prompt = format!(...)` at the system prompt construction site (~line 64).

- [ ] **Step 2: Build ContextPackage and inject**

After workspace context is loaded and system prompt is assembled, add:

```rust
// --- Context Package (Phase C) ---
let denied_sections = agent.constraints.denied_sections.clone();

let mut pkg_builder = crate::context_budget::ContextPackageBuilder::new(
    node.description.clone(),
);

// Add workspace context as artifact
if !workspace_context.is_empty() {
    pkg_builder = pkg_builder.workspace_artifact(workspace_context.clone());
}

if !denied_sections.is_empty() {
    pkg_builder = pkg_builder.denied_sections(&denied_sections);
}

let context_package = pkg_builder.build();

// Compute tokens from only the optional sections actually injected
let injected_sections_tokens = {
    let mut t = 0usize;
    if let Some(ref s) = context_package.conversation_summary { t += s.len() / 4 + 20; }
    for m in &context_package.relevant_memories { t += m.len() / 4 + 10; }
    if let Some(ref s) = context_package.user_context { t += s.len() / 4 + 20; }
    for a in &context_package.workspace_artifacts { t += a.len() / 4 + 20; }
    t
};
bus.publish(crate::events::SystemEvent::ContextPackageBuilt {
    request_id: uuid::Uuid::new_v4(),
    agent_id: agent.id.clone(),
    sections_included: context_package.sections_included()
        .into_iter()
        .map(|s| s.to_string())
        .collect(),
    total_tokens: injected_sections_tokens,
    memories_count: context_package.relevant_memories.len(),
    timestamp: chrono::Utc::now(),
});
```

Then append optional sections to system_prompt (same pattern as pipeline_step):

```rust
if let Some(ref summary) = context_package.conversation_summary {
    system_prompt.push_str(&format!(
        "\n\n<conversation-context>\n{}\n</conversation-context>",
        summary
    ));
}
if !context_package.relevant_memories.is_empty() {
    let mem_block = context_package.relevant_memories.join("\n- ");
    system_prompt.push_str(&format!(
        "\n\n<relevant-memories>\n- {}\n</relevant-memories>",
        mem_block
    ));
}
if let Some(ref ctx) = context_package.user_context {
    system_prompt.push_str(&format!(
        "\n\n<user-context>\n{}\n</user-context>",
        ctx
    ));
}
```

- [ ] **Step 3: Verify build + existing tests**

Run: `cargo check --all-targets && cargo test -p openalpaca_core -- dag_executor --nocapture`

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/runner/dag_executor/node_runner.rs
git commit -m "feat(context_budget): wire ContextPackage into DAG node runner (Phase C.5)"
```

---

### Known Phase C limitations

1. **Memory retrieval not wired into ContextPackageBuilder.** The spec (Section 5.3) shows memory store queries feeding `relevant_memories`. In Phase C, the existing memory injection in `pipeline_step.rs` (step 0, lines ~246-272) continues to work independently. Wiring memory search results into the `ContextPackageBuilder` flow is deferred to a follow-up — it requires dependency on the memory repository, which the `context_budget` module currently avoids.

2. **`max_context_tokens` added but not enforced.** The field is added to `AgentConstraints` for forward compatibility but no code checks it in Phase C. The orchestrator does not yet truncate context packages to fit within this budget. Enforcement is planned for when `ContextBudgetManager` is integrated into sub-agent prompt assembly.

---

### Task 6: Phase C verification

- [ ] **Step 1:** `cargo check --all-targets`
- [ ] **Step 2:** `cargo test -p openalpaca_core`
- [ ] **Step 3:** `cargo clippy -p openalpaca_core -- -D warnings`
- [ ] **Step 4:** `cargo check -p openalpacad`
