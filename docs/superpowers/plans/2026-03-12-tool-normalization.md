# Tool Normalization Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify all tool dispatch into a single `SandboxManager` → `ToolRegistry` → `BuiltInTool::execute_with_context()` path, eliminating `ContextualToolExecutor`, `LeadAgentToolExecutor`, `RegistryToolExecutor`, and the `ToolExecutor` trait.

**Architecture:** Add `ToolContext` struct and `execute_with_context()` default method to `BuiltInTool`. Tools that need per-invocation identity override `execute_with_context()`. `SandboxManager` holds `Arc<ToolRegistry>` directly instead of `Arc<dyn ToolExecutor>`. Lead agent coordination tools register into per-session registry clones. Skill scripts become `ScriptToolBuiltIn` implementations in per-invocation registry clones.

**Tech Stack:** Rust, async_trait, tokio, serde_json, openalpaca_llm

**Spec:** `docs/superpowers/specs/2026-03-12-tool-normalization-design.md`

---

## File Structure

### Modified files

| File | Responsibility |
|------|---------------|
| `crates/openalpaca_core/src/tools/registry/mod.rs` | `ToolContext` struct, `BuiltInTool` trait extension, `execute_with_context()` on registry, `exempt_from_timeout` on `RegisteredTool` |
| `crates/openalpaca_core/src/tools/registry/tests.rs` | Tests for new trait method, `exempt_from_timeout` field |
| `crates/openalpaca_core/src/tools/builtins/memory_search.rs` | Override `execute_with_context()` to read `ctx.owner_id`/`ctx.workspace_id` |
| `crates/openalpaca_core/src/tools/builtins/update_persona/mod.rs` | Remove `owner_id` skip guard |
| `crates/openalpaca_core/src/tools/builtins/mod.rs` | Register workspace tools as `BuiltInTool`, add `WorkspaceReadTool`/`WorkspaceWriteTool`, `ScriptToolBuiltIn`, `json_to_cli_args` |
| `crates/openalpaca_core/src/security/sandbox/mod.rs` | Replace `dyn ToolExecutor` with `Arc<ToolRegistry>`, new `execute_tool()` signature |
| `crates/openalpaca_core/src/runner/lead_agent/tools.rs` | Add `register_coordination_tools()` helper (existing `BuiltInTool` impls), update `SpawnSubagentTool` internal sandbox, remove `LeadAgentToolExecutor` |
| `crates/openalpaca_core/src/runner/lead_agent/mod.rs` | Replace executor construction with registry clone + `ToolContext` |
| `crates/openalpaca_core/src/runner/lead_agent/tests.rs` | Update test construction to use `ToolContext` + direct registry |
| `crates/openalpaca_core/src/runner/agentic_loop/mod.rs` | Pass `ToolContext` to `execute_tool()` instead of `agent_id` |
| `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs` | Replace `ContextualToolExecutor` with `ToolContext` |
| `crates/openalpaca_core/src/runner/dag_executor/mod.rs` | Update imports (remove `ContextualToolExecutor`/`ToolExecutionContext`) |
| `crates/openalpaca_core/src/runner/dag_executor/node_runner.rs` | Replace `ContextualToolExecutor` with `ToolContext` |
| `crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs` | Replace `ContextualToolExecutor` with `ToolContext` |
| `crates/openalpaca_core/src/orchestrator/skill/invocation.rs` | Replace `ScriptExecutionContext` with `ScriptToolBuiltIn` in cloned registry |
| `crates/openalpaca_core/src/tools/mod.rs` | Update re-exports |

### Deleted files

| File | Reason |
|------|--------|
| `crates/openalpaca_core/src/tools/contextual_executor/mod.rs` | Logic absorbed into individual tools |
| `crates/openalpaca_core/src/tools/contextual_executor/tests.rs` | Tests migrated to new locations |
| `crates/openalpaca_core/src/tools/executor.rs` | `RegistryToolExecutor` eliminated |

---

## Chunk 1: Foundation — ToolContext, Trait Extension, exempt_from_timeout

### Task 1: Add ToolContext struct and extend BuiltInTool trait

**Files:**
- Modify: `crates/openalpaca_core/src/tools/registry/mod.rs`
- Test: `crates/openalpaca_core/src/tools/registry/tests.rs`

**Context:** The `BuiltInTool` trait currently has only `execute(&self, arguments)`. We add `ToolContext` and a default `execute_with_context()` method that delegates to `execute()`. This is additive — nothing breaks.

- [ ] **Step 1: Write failing test for ToolContext and execute_with_context**

In `crates/openalpaca_core/src/tools/registry/tests.rs`, add:

```rust
#[tokio::test]
async fn test_execute_with_context_defaults_to_execute() {
    // A tool that only implements execute() should work via execute_with_context()
    let tool = MockBuiltIn;
    let ctx = super::ToolContext {
        agent_id: Some("test-agent".to_string()),
        task_id: None,
        owner_id: None,
        workspace_id: None,
    };
    let args = serde_json::json!({"key": "value"});
    let result = tool.execute_with_context(&args, &ctx).await.unwrap();
    assert_eq!(result, "mock");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p openalpaca_core -- registry::tests::test_execute_with_context_defaults_to_execute`
Expected: FAIL — `ToolContext` and `execute_with_context` don't exist yet

- [ ] **Step 3: Implement ToolContext and trait extension**

In `crates/openalpaca_core/src/tools/registry/mod.rs`, add after the `use` statements (before `ToolBackend`):

```rust
/// Per-invocation execution context passed to tools that need identity.
/// Lightweight — no Arc deps, no DB handles. Just identity strings.
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    pub agent_id: Option<String>,
    pub task_id: Option<String>,
    pub owner_id: Option<String>,
    pub workspace_id: Option<String>,
}
```

Update the `BuiltInTool` trait to add the default method:

```rust
#[async_trait]
pub trait BuiltInTool: Send + Sync {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String>;

    /// Execute with per-invocation context. Default delegates to execute().
    /// Override for tools that need identity (owner_id, task_id, etc.).
    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<String, String> {
        self.execute(arguments).await
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p openalpaca_core -- registry::tests::test_execute_with_context_defaults_to_execute`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/tools/registry/mod.rs crates/openalpaca_core/src/tools/registry/tests.rs
git commit -m "feat: add ToolContext struct and execute_with_context default method on BuiltInTool"
```

---

### Task 2: Add execute_with_context to ToolRegistry

**Files:**
- Modify: `crates/openalpaca_core/src/tools/registry/mod.rs`
- Test: `crates/openalpaca_core/src/tools/registry/tests.rs`

**Context:** `ToolRegistry` currently has `execute()` which dispatches to `BuiltInTool::execute()`. We add `execute_with_context()` which dispatches to `BuiltInTool::execute_with_context()`. The existing `execute()` stays for backward compatibility during migration.

- [ ] **Step 1: Write failing test**

In `crates/openalpaca_core/src/tools/registry/tests.rs`, add:

```rust
#[tokio::test]
async fn test_registry_execute_with_context_routes_to_builtin() {
    let mut registry = ToolRegistry::new();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "test_tool".to_string(),
            description: "Test".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn)),
        provides_capabilities: vec![],
    });

    let ctx = super::ToolContext::default();
    let result = registry
        .execute_with_context("test_tool", &serde_json::json!({}), &ctx)
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "mock");
}

#[tokio::test]
async fn test_registry_execute_with_context_unknown_tool() {
    let registry = ToolRegistry::new();
    let ctx = super::ToolContext::default();
    let result = registry
        .execute_with_context("no_such_tool", &serde_json::json!({}), &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p openalpaca_core -- registry::tests::test_registry_execute_with_context`
Expected: FAIL — `execute_with_context` method doesn't exist on `ToolRegistry`

- [ ] **Step 3: Implement execute_with_context on ToolRegistry**

In `crates/openalpaca_core/src/tools/registry/mod.rs`, add method to `impl ToolRegistry`:

```rust
    /// Execute a tool by name with per-invocation context.
    /// Routes to BuiltInTool::execute_with_context() for BuiltIn backends.
    /// For Http/Command backends, context is ignored (they don't need identity).
    pub async fn execute_with_context(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let tool = self
            .tools
            .get(tool_name)
            .ok_or_else(|| format!("Tool '{}' not found in registry", tool_name))?;

        match &tool.backend {
            ToolBackend::BuiltIn(implementation) => {
                implementation.execute_with_context(arguments, ctx).await
            }
            ToolBackend::Http { .. } => self.execute(tool_name, arguments).await,
            ToolBackend::Command { .. } => self.execute(tool_name, arguments).await,
            ToolBackend::Contextual => Err(format!(
                "Tool '{}' has Contextual backend — must be executed via ContextualToolExecutor",
                tool_name
            )),
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p openalpaca_core -- registry::tests::test_registry_execute_with_context`
Expected: PASS (both tests)

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/tools/registry/mod.rs crates/openalpaca_core/src/tools/registry/tests.rs
git commit -m "feat: add execute_with_context method to ToolRegistry"
```

---

### Task 3: Add exempt_from_timeout field to RegisteredTool

**Files:**
- Modify: `crates/openalpaca_core/src/tools/registry/mod.rs`
- Modify: `crates/openalpaca_core/src/tools/registry/tests.rs` (update all `RegisteredTool` literals)
- Modify: `crates/openalpaca_core/src/tools/builtins/mod.rs` (update all registrations)
- Modify: `crates/openalpaca_core/src/tools/executor.rs` (update test literals)

**Context:** `RegisteredTool` gets a new `exempt_from_timeout: bool` field (default `false`). This replaces the hardcoded `COORDINATION_TOOLS` const in `sandbox/mod.rs`. Every existing `RegisteredTool` literal must be updated to include the field.

- [ ] **Step 1: Add field to RegisteredTool**

In `crates/openalpaca_core/src/tools/registry/mod.rs`, update `RegisteredTool`:

```rust
pub struct RegisteredTool {
    pub definition: ToolDefinition,
    pub backend: ToolBackend,
    pub provides_capabilities: Vec<String>,
    /// When true, SandboxManager skips the per-tool timeout for this tool.
    /// Used for coordination tools that manage their own timeouts.
    pub exempt_from_timeout: bool,
}
```

- [ ] **Step 2: Fix all compilation errors**

Add `exempt_from_timeout: false` to every `RegisteredTool` literal in:
- `crates/openalpaca_core/src/tools/registry/tests.rs` — all `make_tool_with_caps()` calls and raw `RegisteredTool` literals
- `crates/openalpaca_core/src/tools/builtins/mod.rs` — `builtin_tools()` and workspace tool registrations
- `crates/openalpaca_core/src/tools/builtins/memory_search.rs` — `memory_search_tool()` return
- `crates/openalpaca_core/src/tools/builtins/update_persona/mod.rs` — `update_persona_tool()` return
- `crates/openalpaca_core/src/tools/builtins/send.rs` — `send_tool()` return
- `crates/openalpaca_core/src/tools/builtins/file_read.rs` — if it returns `RegisteredTool`
- `crates/openalpaca_core/src/tools/builtins/file_write.rs` — same
- `crates/openalpaca_core/src/tools/builtins/shell_execute.rs` — same
- `crates/openalpaca_core/src/tools/builtins/web_search.rs` — same
- `crates/openalpaca_core/src/tools/builtins/web_fetch.rs` — same
- `crates/openalpaca_core/src/tools/executor.rs` — test `RegisteredTool` literals
- `crates/openalpaca_core/src/tools/contextual_executor/tests.rs` — `make_registry_with_tools()` helper

Search for all occurrences: `grep -rn "RegisteredTool {" crates/openalpaca_core/src/`
(Note: scope is all of `src/`, not just `src/tools/` — hits in `runner/lead_agent/` too)

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: compiles with no errors

- [ ] **Step 4: Run all tests**

Run: `cargo test -p openalpaca_core -- tools::`
Expected: all tests pass

- [ ] **Step 5: Commit**

```bash
git add -A crates/openalpaca_core/src/tools/
git commit -m "feat: add exempt_from_timeout field to RegisteredTool"
```

---

### Task 4: Add shell_like_tool_names() and registered_tool_names_set() to ToolRegistry

**Files:**
- Modify: `crates/openalpaca_core/src/tools/registry/mod.rs`

**Context:** `SandboxManager` currently calls `self.executor.registered_tools()` and `self.executor.shell_like_tools()` for input sanitization. When we replace the executor with `Arc<ToolRegistry>`, we need these methods on `ToolRegistry`. `registered_tool_names()` already exists. We need `shell_like_tool_names()` which is identical to the existing `command_backend_tool_names()` — just add an alias or use the existing method. Also add a method to check if a tool is exempt from timeout.

- [ ] **Step 1: Add is_exempt_from_timeout method**

In `crates/openalpaca_core/src/tools/registry/mod.rs`, add to `impl ToolRegistry`:

```rust
    /// Check if a tool is exempt from the per-tool sandbox timeout.
    pub fn is_exempt_from_timeout(&self, tool_name: &str) -> bool {
        self.tools
            .get(tool_name)
            .map(|t| t.exempt_from_timeout)
            .unwrap_or(false)
    }
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p openalpaca_core`
Expected: compiles

- [ ] **Step 3: Commit**

```bash
git add crates/openalpaca_core/src/tools/registry/mod.rs
git commit -m "feat: add is_exempt_from_timeout to ToolRegistry"
```

---

## Chunk 2: Category B Tool Migration + ScriptToolBuiltIn

### Task 5: Migrate memory_search to execute_with_context

**Files:**
- Modify: `crates/openalpaca_core/src/tools/builtins/memory_search.rs`
- Test: `crates/openalpaca_core/src/tools/registry/tests.rs` (new test)

**Context:** `memory_search` currently receives `owner_id` and `workspace_id` via argument injection in `ContextualToolExecutor`. After: it overrides `execute_with_context()` to read from `ToolContext`. The `execute()` method still works but logs a warning when `owner_id` is missing from args (backward compat during migration). Anti-spoofing: the tool reads from `ctx` not from args, so LLM-supplied `owner_id` in args is ignored.

- [ ] **Step 1: Write failing test**

In `crates/openalpaca_core/src/tools/registry/tests.rs`, add:

```rust
#[tokio::test]
async fn test_memory_search_reads_context_not_args() {
    // Verify that execute_with_context reads owner_id from ctx, not args
    use super::ToolContext;

    struct ContextCaptureTool;

    #[async_trait]
    impl super::BuiltInTool for ContextCaptureTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Err("should not be called directly".to_string())
        }
        async fn execute_with_context(
            &self,
            arguments: &serde_json::Value,
            ctx: &ToolContext,
        ) -> Result<String, String> {
            Ok(serde_json::json!({
                "ctx_owner": ctx.owner_id,
                "ctx_workspace": ctx.workspace_id,
                "args": arguments,
            }).to_string())
        }
    }

    let tool = ContextCaptureTool;
    let ctx = ToolContext {
        owner_id: Some("real-owner".to_string()),
        workspace_id: Some("ws-1".to_string()),
        ..Default::default()
    };
    let result = tool
        .execute_with_context(
            &serde_json::json!({"query": "test", "owner_id": "spoofed"}),
            &ctx,
        )
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["ctx_owner"], "real-owner");
    assert_eq!(parsed["ctx_workspace"], "ws-1");
}
```

- [ ] **Step 2: Run test — passes (demonstrates the pattern)**

Run: `cargo test -p openalpaca_core -- registry::tests::test_memory_search_reads_context_not_args`
Expected: PASS (this is a pattern test)

- [ ] **Step 3: Implement execute_with_context on MemorySearchTool**

In `crates/openalpaca_core/src/tools/builtins/memory_search.rs`, add the `use` for `ToolContext`:

```rust
use crate::tools::registry::ToolContext;
```

Then add `execute_with_context` override to `impl BuiltInTool for MemorySearchTool`:

```rust
    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        // Read identity from context, not from arguments (anti-spoofing)
        let owner_id = ctx.owner_id.as_deref().ok_or_else(|| {
            "Tool 'memory_search' requires owner_id but none provided in execution context"
                .to_string()
        })?;

        let mut args = arguments.clone();
        if let Some(obj) = args.as_object_mut() {
            obj.insert(
                "owner_id".to_string(),
                serde_json::Value::String(owner_id.to_string()),
            );
            if let Some(ref ws_id) = ctx.workspace_id {
                obj.insert(
                    "workspace_id".to_string(),
                    serde_json::Value::String(ws_id.clone()),
                );
            }
        }
        self.execute(&args).await
    }
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p openalpaca_core`
Expected: compiles

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/tools/builtins/memory_search.rs crates/openalpaca_core/src/tools/registry/tests.rs
git commit -m "feat: memory_search reads owner_id from ToolContext via execute_with_context"
```

---

### Task 6: Remove owner_id skip guard from update_persona

**Files:**
- Modify: `crates/openalpaca_core/src/tools/builtins/update_persona/mod.rs`

**Context:** `PersonaUpdateTool::execute()` has `if key == "owner_id" { continue; }` at line 62-63 to silently skip the injected `owner_id`. After normalization, no `owner_id` is injected into args, so this guard becomes dead code. Remove it.

- [ ] **Step 1: Remove the owner_id skip guard**

In `crates/openalpaca_core/src/tools/builtins/update_persona/mod.rs`, remove lines 62-64:

```rust
                if key == "owner_id" {
                    continue;
                }
```

- [ ] **Step 2: Verify compilation and tests**

Run: `cargo test -p openalpaca_core -- update_persona`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/openalpaca_core/src/tools/builtins/update_persona/mod.rs
git commit -m "refactor: remove dead owner_id skip guard from PersonaUpdateTool"
```

---

### Task 7: Extract workspace tools into standalone BuiltInTool implementations

**Files:**
- Modify: `crates/openalpaca_core/src/tools/builtins/mod.rs`

**Context:** `workspace_read` and `workspace_write` are currently handled as methods on `ContextualToolExecutor`. We extract them into standalone `WorkspaceReadTool` and `WorkspaceWriteTool` structs that implement `BuiltInTool` with `execute_with_context()`. They hold `Arc<Database>` for DB access and read `task_id`/`agent_id` from `ToolContext`. They are registered with `ToolBackend::BuiltIn` instead of `ToolBackend::Contextual`.

- [ ] **Step 1: Add workspace tool structs and implementations**

In `crates/openalpaca_core/src/tools/builtins/mod.rs`, add the workspace tool imports and structs. The logic is moved from `contextual_executor/mod.rs` lines 170-312:

```rust
use crate::orchestrator::task_state::{TaskState, WorkspaceEntryType};
use crate::tools::registry::ToolContext;

/// Standalone workspace_read tool.
struct WorkspaceReadTool {
    db: openalpaca_storage::Database,
}

#[async_trait]
impl BuiltInTool for WorkspaceReadTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("workspace_read requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let task_id = ctx.task_id.as_deref()
            .ok_or_else(|| "workspace_read requires a task context".to_string())?;

        let repo = openalpaca_storage::repository::TaskRepository::new(&self.db);
        let task = repo.get(task_id)
            .map_err(|e| format!("Failed to load task: {e}"))?
            .ok_or_else(|| format!("Task '{}' not found", task_id))?;

        let state: TaskState = match task.state_json.as_deref() {
            Some(json) => serde_json::from_str(json)
                .map_err(|e| format!("Failed to parse task state: {e}"))?,
            None => return Ok("[]".to_string()),
        };

        let key = arguments.get("key").and_then(|v| v.as_str()).unwrap_or("");
        let entries = state.workspace.read(key);
        let result: Vec<serde_json::Value> = entries.iter().map(|e| {
            serde_json::json!({
                "key": e.key,
                "content": e.content,
                "author": e.author_agent_id,
                "type": e.entry_type,
            })
        }).collect();

        serde_json::to_string(&result).map_err(|e| format!("Serialization error: {e}"))
    }
}

/// Standalone workspace_write tool.
struct WorkspaceWriteTool {
    db: openalpaca_storage::Database,
}

#[async_trait]
impl BuiltInTool for WorkspaceWriteTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("workspace_write requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let task_id = ctx.task_id.as_deref()
            .ok_or_else(|| "workspace_write requires a task context".to_string())?;
        let agent_id = ctx.agent_id.as_deref().unwrap_or("unknown");

        let key = arguments.get("key").and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: key".to_string())?;
        let content = arguments.get("content").and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        const MAX_WORKSPACE_CONTENT_SIZE: usize = 32768;
        if content.len() > MAX_WORKSPACE_CONTENT_SIZE {
            return Err(format!(
                "Content size {} bytes exceeds the {} byte limit. \
                 Condense or summarize your content to fit within the limit, then retry.",
                content.len(), MAX_WORKSPACE_CONTENT_SIZE
            ));
        }

        let entry_type_str = arguments.get("entry_type").and_then(|v| v.as_str()).unwrap_or("text");
        let entry_type = match entry_type_str {
            "artifact" => WorkspaceEntryType::Artifact,
            "summary" => WorkspaceEntryType::Summary,
            "context" => WorkspaceEntryType::Context,
            _ => WorkspaceEntryType::Text,
        };

        const MAX_RETRIES: usize = 5;
        for attempt in 0..MAX_RETRIES {
            let repo = openalpaca_storage::repository::TaskRepository::new(&self.db);
            let task = repo.get(task_id)
                .map_err(|e| format!("Failed to load task: {e}"))?
                .ok_or_else(|| format!("Task '{}' not found", task_id))?;

            let mut state: TaskState = match task.state_json.as_deref() {
                Some(json) => serde_json::from_str(json)
                    .map_err(|e| format!("Failed to parse task state: {e}"))?,
                None => return Err("Task has no state".to_string()),
            };

            state.workspace.write(key, content, agent_id, entry_type.clone(), &[])?;

            if let Some(fid) = arguments.get("file_asset_id").and_then(|v| v.as_str()) {
                state.workspace.set_file_asset_id(key, fid);
            }

            let new_json = state.to_json();
            let updated = repo.update_state(task_id, &new_json, task.state_version)
                .map_err(|e| format!("Failed to persist workspace: {e}"))?;

            if updated {
                return Ok(format!("Workspace entry '{}' written successfully", key));
            }

            if attempt < MAX_RETRIES - 1 {
                tracing::debug!(
                    "Workspace write version conflict for key '{}' (attempt {}/{}), retrying",
                    key, attempt + 1, MAX_RETRIES
                );
                tokio::time::sleep(std::time::Duration::from_millis(50 * (1 << attempt))).await;
            }
        }

        Err(format!(
            "Workspace write for key '{}' failed after {} retries due to concurrent modifications",
            key, MAX_RETRIES
        ))
    }
}
```

- [ ] **Step 2: Update workspace tool registration**

In the same file, update the workspace tool registration in `builtin_tools()` (or `builtin_tools_with_persona_context()`) to use `ToolBackend::BuiltIn` instead of `ToolBackend::Contextual`. The workspace tools need a `db` parameter, so update the function signature to accept `Option<Database>`.

Find the existing workspace tool registrations (which use `ToolBackend::Contextual`) and replace with:

```rust
// Register workspace tools when database is available
if let Some(ref db) = db {
    tools.push(RegisteredTool {
        definition: workspace_read_definition(),
        backend: ToolBackend::BuiltIn(Arc::new(WorkspaceReadTool { db: db.clone() })),
        provides_capabilities: vec!["workspace_read".to_string()],
        exempt_from_timeout: false,
    });
    tools.push(RegisteredTool {
        definition: workspace_write_definition(),
        backend: ToolBackend::BuiltIn(Arc::new(WorkspaceWriteTool { db: db.clone() })),
        provides_capabilities: vec!["workspace_write".to_string()],
        exempt_from_timeout: false,
    });
}
```

Note: The existing `workspace_tool_definitions()` function is used to get the `ToolDefinition` objects — extract the definition creation into helper functions (`workspace_read_definition()`, `workspace_write_definition()`) or inline them.

- [ ] **Step 3: Update all callers of builtin_tools to pass db**

Search for all callers: `grep -rn "builtin_tools\b" crates/openalpaca_core/`

Update each call site to pass the database reference. If `db` is not available at that call site, pass `None`.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: compiles

- [ ] **Step 5: Run tests**

Run: `cargo test -p openalpaca_core -- tools::`
Expected: all pass

- [ ] **Step 6: Commit**

```bash
git add -A crates/openalpaca_core/src/tools/
git commit -m "feat: extract workspace tools into standalone BuiltInTool implementations"
```

---

### Task 8: Create ScriptToolBuiltIn implementation

**Files:**
- Modify: `crates/openalpaca_core/src/tools/builtins/mod.rs`

**Context:** Skill-bundled script tools (`skill_script:*`) are currently handled by `ScriptExecutionContext` inside `ContextualToolExecutor`. We extract this into a standalone `ScriptToolBuiltIn` struct implementing `BuiltInTool`. Each script tool becomes a `RegisteredTool` in a per-invocation registry clone (Task 9 Step 7 uses this). The `json_to_cli_args` helper moves here too. The constructor includes the same path-traversal security validation as the existing `ScriptExecutionContext::new()`.

- [ ] **Step 1: Add ScriptToolBuiltIn struct and BuiltInTool impl**

In `crates/openalpaca_core/src/tools/builtins/mod.rs`, add:

```rust
use std::path::PathBuf;
use tokio::process::Command;

/// A skill-bundled script tool wrapped as a BuiltInTool.
///
/// Each instance represents one script from a skill's `scripts` frontmatter.
/// Registered dynamically in per-invocation registry clones.
pub struct ScriptToolBuiltIn {
    /// Canonicalized path to the script file (validated to be within skill_dir/scripts/).
    script_path: PathBuf,
    /// Optional interpreter (e.g. "python3", "bash"). If None, script is executed directly.
    interpreter: Option<String>,
    /// Timeout in seconds for script execution.
    timeout_secs: u64,
    /// Skill directory (used as working directory for script execution).
    skill_dir: PathBuf,
}

impl ScriptToolBuiltIn {
    /// Create a new ScriptToolBuiltIn with path-traversal validation.
    ///
    /// # Errors
    /// Returns error if:
    /// - Script file does not exist
    /// - Scripts directory does not exist
    /// - Resolved path escapes the skill's scripts/ directory (path traversal)
    pub fn new(
        skill_dir: &std::path::Path,
        cfg: &crate::middleware::skill::ScriptConfig,
    ) -> Result<Self, String> {
        let script_path = skill_dir.join("scripts").join(&cfg.file);
        let canonical = script_path.canonicalize().map_err(|e| {
            format!("Script '{}' not found: {}", cfg.file, e)
        })?;
        let scripts_dir = skill_dir.join("scripts").canonicalize().map_err(|e| {
            format!("Scripts directory not found: {}", e)
        })?;
        if !canonical.starts_with(&scripts_dir) {
            return Err(format!(
                "Script '{}' resolves outside scripts/ directory (path traversal blocked)",
                cfg.file
            ));
        }
        Ok(Self {
            script_path: canonical,
            interpreter: cfg.interpreter.clone(),
            timeout_secs: cfg.timeout_secs,
            skill_dir: skill_dir.to_path_buf(),
        })
    }

    /// Generate a ToolDefinition for a script tool.
    pub fn tool_definition(name: &str) -> openalpaca_llm::ToolDefinition {
        openalpaca_llm::ToolDefinition {
            name: format!("skill_script:{}", name),
            description: format!("Skill script: {}", name),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }
}

#[async_trait]
impl BuiltInTool for ScriptToolBuiltIn {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let args = json_to_cli_args(arguments);

        let mut cmd = if let Some(ref interp) = self.interpreter {
            let mut c = Command::new(interp);
            c.arg(&self.script_path);
            c
        } else {
            Command::new(&self.script_path)
        };
        cmd.args(&args);
        cmd.current_dir(&self.skill_dir);

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            cmd.output(),
        )
        .await
        .map_err(|_| format!("Script timed out after {}s", self.timeout_secs))?
        .map_err(|e| format!("Failed to execute script: {}", e))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "Script failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                stderr.chars().take(500).collect::<String>()
            ))
        }
    }
}

/// Convert JSON object to `--key=value` CLI arguments.
pub fn json_to_cli_args(value: &serde_json::Value) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(obj) = value.as_object() {
        for (key, val) in obj {
            let str_val = match val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Number(n) => n.to_string(),
                other => other.to_string(),
            };
            args.push(format!("--{}={}", key, str_val));
        }
    }
    args
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: compiles (nothing references ScriptToolBuiltIn yet — just confirming no syntax errors)

- [ ] **Step 3: Commit**

```bash
git add crates/openalpaca_core/src/tools/builtins/mod.rs
git commit -m "feat: add ScriptToolBuiltIn implementation for skill-bundled script tools"
```

---

## Chunk 3: SandboxManager + Agentic Loop + All Call Sites (Atomic Migration)

**IMPORTANT:** This is a single atomic commit. Changing `SandboxManager` without updating all call sites simultaneously would leave the codebase in a non-compiling state.

### Task 9: Atomic migration — SandboxManager + agentic loop + all call sites

**Files:**
- Modify: `crates/openalpaca_core/src/security/sandbox/mod.rs`
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/mod.rs`
- Modify: `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs`
- Modify: `crates/openalpaca_core/src/runner/dag_executor/node_runner.rs`
- Modify: `crates/openalpaca_core/src/runner/dag_executor/mod.rs` (imports)
- Modify: `crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs`
- Modify: `crates/openalpaca_core/src/orchestrator/skill/invocation.rs`

**Context:** This is the core migration. All changes must compile together — there is no intermediate state where `SandboxManager` takes `Arc<ToolRegistry>` but call sites still pass `Arc<dyn ToolExecutor>`.

**Part A: Update SandboxManager**

- [ ] **Step 1: Update SandboxManager struct and constructors**

In `crates/openalpaca_core/src/security/sandbox/mod.rs`:

Add imports:
```rust
use crate::tools::registry::ToolContext;
use crate::tools::ToolRegistry;
```

Replace `executor: Arc<dyn ToolExecutor>` with `registry: Arc<ToolRegistry>` in the struct and all constructors (`new`, `with_db`, `with_defaults`).

- [ ] **Step 2: Update execute_tool signature and body**

Replace the `execute_tool` method signature:
```rust
pub async fn execute_tool(
    &self,
    tool_call: &ToolCall,
    policy: &SandboxPolicy,
    ctx: &ToolContext,
) -> Result<String, String> {
```

Update the body:
1. Replace `self.executor.registered_tools()` → `self.registry.registered_tool_names()`
2. Replace `self.executor.shell_like_tools()` → `self.registry.command_backend_tool_names()`
3. Replace `COORDINATION_TOOLS.contains(...)` → `self.registry.is_exempt_from_timeout(&tool_call.name)`
4. Replace `executor.execute(&tool_name, &arguments)` → `self.registry.execute_with_context(&tool_name, &arguments, ctx)`
5. Replace all `agent_id` parameter references with `policy.agent_id.as_str()` (SandboxPolicy already carries agent_id)
6. Remove the `COORDINATION_TOOLS` const

Keep `ToolExecutor` trait temporarily — it's removed in cleanup.

**Part B: Update agentic loop**

- [ ] **Step 3: Add ToolContext parameter to run_agentic_loop and run_agentic_loop_routed**

Add `tool_context: Option<&ToolContext>` parameter after `sandbox_policy`. Note: `run_agentic_loop*` is NOT called from `apps/` — only from within `openalpaca_core`.

Update the `sbx.execute_tool()` call:
```rust
let ctx = tool_context.cloned().unwrap_or_else(|| ToolContext {
    agent_id: Some(agent_id.to_string()),
    ..Default::default()
});
match sbx.execute_tool(tc, policy, &ctx).await {
```

**Part C: Update all call sites simultaneously**

- [ ] **Step 4: Update pipeline_step.rs**

Replace:
```rust
use crate::tools::{ContextualToolExecutor, ToolExecutionContext, ToolRegistry};
```
With:
```rust
use crate::tools::ToolRegistry;
use crate::tools::registry::ToolContext;
```

Replace executor construction with:
```rust
let tool_ctx = ToolContext {
    agent_id: Some(agent_id.clone()),
    task_id: Some(pctx.task_id.clone()),
    owner_id: Some(pctx.created_by.clone()),
    workspace_id: pctx.workspace_id.clone(),
};
let mut per_request_sandbox =
    SandboxManager::with_defaults(pctx.tool_registry.clone(), pctx.bus.clone());
```

Pass `Some(&tool_ctx)` to `run_agentic_loop_routed`.

- [ ] **Step 5: Update node_runner.rs**

Same pattern as pipeline_step.rs. Also update `dag_executor/mod.rs` imports to remove `ContextualToolExecutor`/`ToolExecutionContext`.

- [ ] **Step 6: Update simple_query_handler.rs**

Replace executor construction with:
```rust
let tool_ctx = ToolContext {
    agent_id: None,
    task_id: None,
    owner_id: owner_id.map(|s| s.to_string()),
    workspace_id: scope_ctx.workspace_id.clone(),
};
let mut per_request_sandbox =
    SandboxManager::with_defaults(self.tool_registry.clone(), self.bus.clone());
```

Pass `Some(&tool_ctx)` to `run_agentic_loop_routed`.

- [ ] **Step 7: Update skill invocation.rs (ScriptToolBuiltIn + registry clone)**

Replace `ScriptExecutionContext` + `ContextualToolExecutor::with_scripts()` with:
```rust
let tool_ctx = ToolContext {
    agent_id: None,
    task_id: None,
    owner_id: owner_id.map(|s| s.to_string()),
    workspace_id: scope_ctx.workspace_id.clone(),
};

let registry = if !skill_doc.frontmatter.scripts.is_empty() {
    let mut cloned = (*self.tool_registry).clone();
    for cfg in &skill_doc.frontmatter.scripts {
        let tool = ScriptToolBuiltIn::new(&entry.skill_dir, cfg)?;
        cloned.register(RegisteredTool {
            definition: ScriptToolBuiltIn::tool_definition(&cfg.name),
            backend: ToolBackend::BuiltIn(Arc::new(tool)),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
        });
    }
    Arc::new(cloned)
} else {
    self.tool_registry.clone()
};
let mut per_request_sandbox = SandboxManager::with_defaults(registry, self.bus.clone());
```

Pass `Some(&tool_ctx)` to `run_agentic_loop_routed`.

**Part D: Verification**

- [ ] **Step 8: Verify compilation**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: compiles

- [ ] **Step 9: Run full test suite**

Run: `cargo test -p openalpaca_core`
Expected: all tests pass

- [ ] **Step 10: Commit**

```bash
git add -A crates/openalpaca_core/src/
git commit -m "refactor: SandboxManager uses Arc<ToolRegistry> + ToolContext; migrate all call sites atomically"
```

---

## Chunk 4: Lead Agent Migration

### Task 10: Add register_coordination_tools + update SpawnSubagentTool internal sandbox

**Files:**
- Modify: `crates/openalpaca_core/src/runner/lead_agent/tools.rs`

**Context:** All four coordination tools (`SpawnSubagentTool`, `CheckSubagentStatusTool`, `WaitForSubagentsTool`, `SpawnSubagentsBatchTool`) **already implement `BuiltInTool`** with `execute()` methods (lines 132, 428, 554, 623 of `tools.rs`). We do NOT add new impls — we add a `register_coordination_tools()` helper that registers these existing impls into a `ToolRegistry`. We also update `SpawnSubagentTool::execute()` to replace its internal `ContextualToolExecutor` construction with direct `ToolContext` + `Arc<ToolRegistry>`.

- [ ] **Step 1: Add register_coordination_tools helper**

In `crates/openalpaca_core/src/runner/lead_agent/tools.rs`, add:

```rust
use crate::tools::registry::{RegisteredTool, ToolBackend, ToolContext};

/// Register lead agent coordination tools into a mutable ToolRegistry.
/// Called before the lead agent's agentic loop starts.
pub fn register_coordination_tools(
    registry: &mut crate::tools::ToolRegistry,
    spawn_tool: Arc<SpawnSubagentTool>,
    batch_spawn_tool: Option<Arc<SpawnSubagentsBatchTool>>,
    check_status_tool: Arc<CheckSubagentStatusTool>,
    wait_tool: Arc<WaitForSubagentsTool>,
    spawn_def: openalpaca_llm::ToolDefinition,
    batch_def: Option<openalpaca_llm::ToolDefinition>,
    check_def: openalpaca_llm::ToolDefinition,
    wait_def: openalpaca_llm::ToolDefinition,
) {
    registry.register(RegisteredTool {
        definition: spawn_def,
        backend: ToolBackend::BuiltIn(spawn_tool),
        provides_capabilities: vec!["orchestration".to_string()],
        exempt_from_timeout: false,
    });
    if let (Some(batch), Some(def)) = (batch_spawn_tool, batch_def) {
        registry.register(RegisteredTool {
            definition: def,
            backend: ToolBackend::BuiltIn(batch),
            provides_capabilities: vec!["orchestration".to_string()],
            exempt_from_timeout: false,
        });
    }
    registry.register(RegisteredTool {
        definition: check_def,
        backend: ToolBackend::BuiltIn(check_status_tool),
        provides_capabilities: vec!["orchestration".to_string()],
        exempt_from_timeout: true, // manages own timeout
    });
    registry.register(RegisteredTool {
        definition: wait_def,
        backend: ToolBackend::BuiltIn(wait_tool),
        provides_capabilities: vec!["orchestration".to_string()],
        exempt_from_timeout: true, // manages own timeout
    });
}
```

- [ ] **Step 2: Update SpawnSubagentTool::execute() internal sandbox construction**

In the existing `impl BuiltInTool for SpawnSubagentTool`, replace the internal `ContextualToolExecutor` construction (lines 219-231):

```rust
// Before:
let ctx_exec = ToolExecutionContext { owner_id: ..., task_id: ..., agent_id: ..., db: ..., workspace_id: ... };
let contextual_executor = Arc::new(ContextualToolExecutor::new(self.tool_registry.clone(), ctx_exec));
let mut sandbox = SandboxManager::with_defaults(contextual_executor, self.bus.clone());
```

With:
```rust
// After:
let child_ctx = ToolContext {
    agent_id: Some(agent_id.to_string()),
    task_id: Some(self.task_id.clone()),
    owner_id: Some(self.created_by.clone()),
    workspace_id: self.workspace_id.clone(),
};
let mut sandbox = SandboxManager::with_defaults(self.tool_registry.clone(), self.bus.clone());
```

Then pass `Some(&child_ctx)` to the `run_agentic_loop_routed` call for the spawned subagent.

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/runner/lead_agent/tools.rs
git commit -m "feat: add register_coordination_tools, update SpawnSubagentTool internal sandbox"
```

---

### Task 11: Migrate run_lead_agent + update lead_agent tests

**Files:**
- Modify: `crates/openalpaca_core/src/runner/lead_agent/mod.rs`
- Modify: `crates/openalpaca_core/src/runner/lead_agent/tests.rs`

**Context:** `run_lead_agent()` currently constructs `ToolExecutionContext` → `ContextualToolExecutor` → `LeadAgentToolExecutor` → `SandboxManager`. After: it clones the registry, registers coordination tools, and passes the registry directly to `SandboxManager`. Also, `tests.rs` uses `ContextualToolExecutor` and `ToolExecutionContext` in 4+ test functions — these must be updated to use `ToolContext` + direct registry.

- [ ] **Step 1: Update imports**

Replace:
```rust
use crate::tools::{ContextualToolExecutor, ToolExecutionContext};
```
With:
```rust
use crate::tools::registry::ToolContext;
```

- [ ] **Step 2: Replace executor construction**

Replace the block at lines ~163-185:
```rust
let ctx_exec = ToolExecutionContext { ... };
let contextual_executor = Arc::new(ContextualToolExecutor::new(...));
let lead_executor = Arc::new(LeadAgentToolExecutor::new(...));
let mut sandbox = SandboxManager::with_defaults(lead_executor, bus.clone());
```

With:
```rust
// Clone registry and register coordination tools for this lead agent session
let mut lead_registry = (*tool_registry).clone();
register_coordination_tools(
    &mut lead_registry,
    spawn_tool,
    batch_spawn_tool,
    check_status_tool,
    wait_tool,
    &tool_defs, // struct holding the ToolDefinition objects
);
let lead_registry = Arc::new(lead_registry);

let tool_ctx = ToolContext {
    agent_id: Some(lead_agent.id.clone()),
    task_id: Some(task_id.to_string()),
    owner_id: Some(created_by.to_string()),
    workspace_id: workspace_id.clone(),
};

let mut sandbox = SandboxManager::with_defaults(lead_registry, bus.clone());
```

Pass `Some(&tool_ctx)` to `run_agentic_loop_routed`.

- [ ] **Step 3: Verify compilation and tests**

Run: `cargo check -p openalpaca_core --all-targets && cargo test -p openalpaca_core -- lead_agent`
Expected: compiles and tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/runner/lead_agent/mod.rs crates/openalpaca_core/src/runner/lead_agent/tests.rs
git commit -m "refactor: run_lead_agent uses registry clone + ToolContext instead of executor wrappers"
```

---

## Chunk 5: Cleanup — Remove Dead Code

### Task 12: Remove ContextualToolExecutor, RegistryToolExecutor, ToolExecutor trait

**Files:**
- Delete: `crates/openalpaca_core/src/tools/contextual_executor/mod.rs`
- Delete: `crates/openalpaca_core/src/tools/contextual_executor/tests.rs`
- Delete: `crates/openalpaca_core/src/tools/executor.rs`
- Modify: `crates/openalpaca_core/src/tools/mod.rs`
- Modify: `crates/openalpaca_core/src/security/sandbox/mod.rs` (remove ToolExecutor trait)

**Context:** All call sites now use `Arc<ToolRegistry>` + `ToolContext`. The executor wrappers are dead code.

- [ ] **Step 1: Remove ToolExecutor trait from sandbox/mod.rs**

Delete the `ToolExecutor` trait definition and the `COORDINATION_TOOLS` const from `crates/openalpaca_core/src/security/sandbox/mod.rs`.

- [ ] **Step 2: Delete contextual_executor module**

Remove the entire `crates/openalpaca_core/src/tools/contextual_executor/` directory.

- [ ] **Step 3: Delete executor.rs**

Remove `crates/openalpaca_core/src/tools/executor.rs`.

- [ ] **Step 4: Update tools/mod.rs**

Replace:
```rust
pub mod contextual_executor;
pub mod executor;

pub use contextual_executor::{ContextualToolExecutor, ScriptExecutionContext, ToolExecutionContext};
pub use executor::RegistryToolExecutor;
```

With:
```rust
pub use registry::ToolContext;
```

- [ ] **Step 5: Remove LeadAgentToolExecutor from tools.rs**

In `crates/openalpaca_core/src/runner/lead_agent/tools.rs`, remove the `LeadAgentToolExecutor` struct and its `impl ToolExecutor` block. Also remove the import of `ToolExecutor`.

- [ ] **Step 6: Remove ToolBackend::Contextual variant**

In `crates/openalpaca_core/src/tools/registry/mod.rs`, remove the `Contextual` variant from `ToolBackend` enum. Update the `execute()` and `execute_with_context()` methods to remove the `Contextual` match arm.

- [ ] **Step 7: Fix remaining compilation errors**

Search for any remaining references:
```bash
cargo check -p openalpaca_core --all-targets 2>&1 | head -50
```

Fix any remaining imports or references to the removed types.

- [ ] **Step 8: Run full test suite**

Run: `cargo test -p openalpaca_core`
Expected: all tests pass

- [ ] **Step 9: Commit**

```bash
git add -A crates/openalpaca_core/
git commit -m "refactor: remove ContextualToolExecutor, RegistryToolExecutor, ToolExecutor trait, ToolBackend::Contextual"
```

---

### Task 13: Migrate contextual_executor tests

**Files:**
- Modify: `crates/openalpaca_core/src/tools/registry/tests.rs`

**Context:** The deleted `contextual_executor/tests.rs` had valuable test coverage for owner_id injection, anti-spoofing, workspace tool listing, and script tool listing. These behaviors now live in different places: owner_id injection is in `memory_search`'s `execute_with_context`, workspace tools are in the registry, script tools are in `ScriptToolBuiltIn`. We rewrite equivalent tests targeting the new code paths.

- [ ] **Step 1: Add tests for memory_search context injection**

```rust
#[tokio::test]
async fn test_memory_search_context_injects_owner_id() {
    // Test that execute_with_context on a memory_search-like tool
    // injects owner_id from ToolContext, not from arguments
    // (anti-spoofing test)
}

#[tokio::test]
async fn test_memory_search_context_missing_owner_errors() {
    // Test that execute_with_context returns error when ctx.owner_id is None
}
```

- [ ] **Step 2: Add tests for workspace tool context requirements**

```rust
#[tokio::test]
async fn test_workspace_read_without_task_context_errors() {
    // Test that WorkspaceReadTool returns error when ctx.task_id is None
}
```

- [ ] **Step 3: Add tests for script tool CLI arg conversion**

```rust
#[test]
fn test_json_to_cli_args_strings() { /* moved from contextual_executor/tests.rs */ }

#[test]
fn test_json_to_cli_args_empty() { /* moved */ }

#[test]
fn test_json_to_cli_args_non_object() { /* moved */ }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p openalpaca_core -- tools::`
Expected: all pass

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/tools/
git commit -m "test: migrate contextual_executor tests to new tool locations"
```

---

### Task 14: Final verification

**Files:** None (verification only)

- [ ] **Step 1: Full workspace build**

Run: `cargo check --all-targets`
Expected: compiles with no errors

- [ ] **Step 2: Full test suite**

Run: `cargo test -p openalpaca_core`
Expected: all tests pass, no regressions

- [ ] **Step 3: Clippy**

Run: `cargo clippy -p openalpaca_core -- -D warnings`
Expected: no warnings

- [ ] **Step 4: Verify no remaining references to removed types**

Run:
```bash
grep -rn "ContextualToolExecutor\|ToolExecutionContext\|ScriptExecutionContext\|RegistryToolExecutor\|LeadAgentToolExecutor" crates/openalpaca_core/src/ --include="*.rs"
```
Expected: no matches (except possibly comments explaining the migration)

Run:
```bash
grep -rn "ToolBackend::Contextual" crates/openalpaca_core/src/ --include="*.rs"
```
Expected: no matches

Run:
```bash
grep -rn "COORDINATION_TOOLS\|OWNER_ONLY_TOOLS\|OWNER_AND_WORKSPACE_TOOLS\|WORKSPACE_SCOPED_TOOLS" crates/openalpaca_core/src/ --include="*.rs"
```
Expected: no matches

- [ ] **Step 5: Commit verification**

```bash
git log --oneline -20
```
Expected: clean commit history with descriptive messages
