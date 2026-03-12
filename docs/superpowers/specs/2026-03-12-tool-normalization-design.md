# Tool Normalization Design (Group B)

**Date:** 2026-03-12
**Status:** Approved
**Scope:** Unify all tool dispatch into a single path through `ToolRegistry`, eliminating executor wrappers
**Depends on:** Group A — Capability Model & Skill Composition (completed)

---

## 1. Problem Statement

Tool execution currently flows through **four dispatch layers**:

1. **`ContextualToolExecutor`** — injects `owner_id`/`workspace_id` for owner-scoped tools, handles workspace read/write directly, handles `skill_script:*` prefix tools
2. **`LeadAgentToolExecutor`** — intercepts `spawn_subagent`, `spawn_subagents_batch`, `check_subagent_status`, `wait_for_subagents`; delegates everything else to `ContextualToolExecutor`
3. **`RegistryToolExecutor`** (via `ToolRegistry::execute()`) — dispatches to `BuiltInTool`, `Http`, `Command` backends
4. **`SandboxManager`** — wraps `dyn ToolExecutor` with security layers (capability check, sanitization, confirmation, circuit breaker, timeout, event emission)

This creates:
- **Multiple execution paths** that must be kept in sync
- **Implicit context injection** via argument mutation (inserting `owner_id` into JSON args)
- **Separate tool advertising** via `registered_tools()` method that manually merges tool lists
- **Hardcoded routing** via const arrays (`OWNER_ONLY_TOOLS`, `COORDINATION_TOOLS`, etc.)

## 2. Goal

Single execution path: `SandboxManager` → `ToolRegistry` → `BuiltInTool::execute_with_context()`. All tools are self-contained implementations in the registry. No executor wrappers. No argument mutation.

## 3. Design

### 3.1 ToolContext — Per-Invocation Identity

A small struct passed through the execution chain, carrying per-invocation identity without polluting tool arguments:

```rust
/// Per-invocation execution context passed to tools that need identity.
/// Lightweight — no Arc deps, no DB handles. Just identity strings.
pub struct ToolContext {
    pub agent_id: Option<String>,
    pub task_id: Option<String>,
    pub owner_id: Option<String>,
    pub workspace_id: Option<String>,
}
```

**Where it's constructed:** At the `SandboxManager` call site, from the same data currently used to build `ToolExecutionContext`. The `SandboxManager` passes it through to `ToolRegistry`, which passes it to `BuiltInTool::execute_with_context()`.

**What it replaces:** `ToolExecutionContext` struct in `contextual_executor/mod.rs` (which also carries `db: Option<Database>` — tools that need DB access hold it as an `Arc` field instead).

### 3.2 BuiltInTool Trait Extension

Backward-compatible extension to the existing trait:

```rust
#[async_trait]
pub trait BuiltInTool: Send + Sync {
    /// Execute without context (existing method, unchanged).
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

**Backward compatibility:** Existing tools that only implement `execute()` automatically get a working `execute_with_context()` via the default. No changes needed for Category A tools.

### 3.3 Tool Categories

All tools fall into three categories based on their context requirements:

#### Category A — No Context Needed

These tools are pure functions of their arguments. No changes needed.

| Tool | Current | After |
|------|---------|-------|
| `file_read` | `BuiltInTool::execute()` | Same (default `execute_with_context` delegates) |
| `file_write` | Same | Same |
| `shell_execute` | Same | Same |
| `web_search` | Same | Same |
| `web_fetch` | Same | Same |

#### Category A+ — Context Injected But Unused

These tools receive `owner_id` via argument injection in `ContextualToolExecutor` but do not actually use it. The injection path is removed; no `execute_with_context()` override needed.

| Tool | Current | After |
|------|---------|-------|
| `update_persona` | In `OWNER_ONLY_TOOLS` → `owner_id` injected into args, but `PersonaUpdateTool::execute()` silently skips it (`if key == "owner_id" { continue; }`) | Category A — default `execute_with_context` delegates to `execute()`. Remove the `owner_id` skip guard. |
| `send` | Not in any const array — no injection happens today despite spec claims. `SendTool` reads `action`, `channel`, `recipient`, `content` only. | Category A — no changes needed. |

#### Category B — Need Per-Invocation Identity (ToolContext)

These tools currently get identity via argument mutation in `ContextualToolExecutor`. After: they override `execute_with_context()` and read identity from `ToolContext`.

| Tool | Current Context Source | After |
|------|----------------------|-------|
| `memory_search` | `owner_id` + `workspace_id` injected into JSON args by `OWNER_AND_WORKSPACE_TOOLS` routing | Override `execute_with_context()`, read `ctx.owner_id` + `ctx.workspace_id`. When `ctx.owner_id` is `None` (e.g. simple query path), return same error as today: `"Missing owner_id"`. |
| `workspace_read` | Handled directly by `ContextualToolExecutor::handle_workspace_read()` | Standalone `WorkspaceReadTool` struct holding `Arc<Database>`. Override `execute_with_context()`, read `ctx.task_id`. |
| `workspace_write` | Handled directly by `ContextualToolExecutor::handle_workspace_write()` | Standalone `WorkspaceWriteTool` struct holding `Arc<Database>`. Override `execute_with_context()`, read `ctx.task_id`, `ctx.agent_id`. |

**Error behavior when context is missing:** Tools must return the same errors as today when required context is absent. For `memory_search`, when `ctx.owner_id` is `None`, return `Err("Tool 'memory_search' requires owner_id but none provided in execution context")`. For workspace tools, when `ctx.task_id` is `None`, return `Err("workspace_read requires a task context")`.

**workspace_read/workspace_write migration:** Currently implemented as methods on `ContextualToolExecutor` with access to `ToolExecutionContext.db`. After normalization, they become standalone `BuiltInTool` implementations that hold `Arc<Database>` (injected at construction) and read task identity from `ToolContext`. The `ToolBackend::Contextual` variant is eliminated — they use `ToolBackend::BuiltIn` like all other tools.

#### Category C — Need Heavy Arc State + ToolContext

Lead agent coordination tools that currently live in `LeadAgentToolExecutor` with access to shared state (`SubagentTracker`, `AgentRegistry`, etc.). After: these structs hold their dependencies as `Arc` fields (MCP "server holds its own state" pattern) and are registered as normal `BuiltInTool` implementations.

| Tool | Arc Dependencies |
|------|-----------------|
| `spawn_subagent` | `SubagentTracker`, `AgentRegistry`, `LlmRouter`, `EventBus`, `Database`, `ToolRegistry`, `ConfirmationBroker` |
| `spawn_subagents_batch` | Same as `spawn_subagent` (wraps `SpawnSubagentTool`) |
| `check_subagent_status` | `SubagentTracker` |
| `wait_for_subagents` | `SubagentTracker` |

These tools already exist as structs (`SpawnSubagentTool`, `CheckSubagentStatusTool`, etc.) in `runner/lead_agent/tools.rs`. The change is:
1. Move them to implement `BuiltInTool` instead of using the custom `LeadAgentToolExecutor` dispatch
2. Hold all dependencies as `Arc` fields (most already do)
3. Register them in `ToolRegistry` when lead agent mode is active

**Recursive subagent spawning:** `SpawnSubagentTool::execute()` itself spawns child agents, each needing their own `SandboxManager`. Currently it constructs a `ContextualToolExecutor` internally (lines 219-231 of `tools.rs`). After normalization, it constructs a child `ToolContext` and passes `self.tool_registry.clone()` directly to a child `SandboxManager::with_defaults(registry, bus)`. The child `ToolContext` uses the spawned subagent's `agent_id`, not the lead agent's.

### 3.4 Architecture Changes

#### Eliminated

| Component | Location | Reason |
|-----------|----------|--------|
| `ContextualToolExecutor` | `tools/contextual_executor/mod.rs` | Logic absorbed into individual tools |
| `LeadAgentToolExecutor` | `runner/lead_agent/tools.rs` | Tools registered directly in registry |
| `ToolExecutor` trait | `security/sandbox/mod.rs` | `SandboxManager` calls `ToolRegistry` directly |
| `ToolBackend::Contextual` | `tools/registry/mod.rs` | All tools use `ToolBackend::BuiltIn` |
| `ToolExecutionContext` | `tools/contextual_executor/mod.rs` | Replaced by `ToolContext` |
| `OWNER_ONLY_TOOLS` const | `tools/contextual_executor/mod.rs` | Tools handle own context |
| `OWNER_AND_WORKSPACE_TOOLS` const | `tools/contextual_executor/mod.rs` | Tools handle own context |
| `WORKSPACE_SCOPED_TOOLS` const | `tools/contextual_executor/mod.rs` | Tools handle own context |
| `COORDINATION_TOOLS` const | `security/sandbox/mod.rs` | Replaced by `exempt_from_timeout` field |

#### Modified

| Component | Change |
|-----------|--------|
| `SandboxManager` | Replace `executor: Arc<dyn ToolExecutor>` with `registry: Arc<ToolRegistry>`. Replace `execute_tool(&self, agent_id, tool_call, policy)` signature with `execute_tool(&self, tool_call, policy, ctx: &ToolContext)` — `agent_id` is now `ctx.agent_id` (already in `SandboxPolicy.agent_id` for logging; `ToolContext` is for tool execution identity). Call `registry.execute_with_context(name, args, ctx)`. |
| `ToolRegistry` | Add `execute_with_context()` method that resolves tool and calls `BuiltInTool::execute_with_context()`. Remove `ToolBackend::Contextual` variant. |
| `RegisteredTool` | Add `exempt_from_timeout: bool` field (default `false`). Set `true` for `wait_for_subagents` and `check_subagent_status`. |
| `BuiltInTool` trait | Add `execute_with_context()` method with default delegation to `execute()`. |

#### New Execution Flow

```
SandboxManager::execute_tool(tool_call, policy, &tool_context)
  │
  ├── 1. Capability check (unchanged)
  ├── 2. Input sanitization (unchanged)
  ├── 3. Confirmation check (unchanged)
  ├── 4. Circuit breaker check (unchanged)
  ├── 5. Timeout wrapping
  │     └── if registered_tool.exempt_from_timeout → no timeout
  │     └── else → wrap in tokio::time::timeout
  ├── 6. registry.execute_with_context(name, args, &tool_context)
  │     └── BuiltInTool::execute_with_context(args, ctx)
  │         ├── Category A: delegates to execute(args) via default
  │         ├── Category B: reads ctx.owner_id/task_id, executes
  │         └── Category C: uses Arc state + ctx, executes
  └── 7. Event emission (unchanged)
```

### 3.5 Skill Script Handling

Skill scripts (`skill_script:*`) are currently handled by `ScriptExecutionContext` attached to `ContextualToolExecutor`. After normalization:

**Per-invocation registry clone:** When a skill invocation requires script tools:
1. Clone the `ToolRegistry` (already supports `Clone`)
2. Register each script as a `ScriptToolBuiltIn` implementing `BuiltInTool`
3. Pass the cloned registry to the skill's agentic loop
4. The clone is dropped when the skill invocation ends

```rust
/// BuiltInTool implementation for skill-bundled scripts.
struct ScriptToolBuiltIn {
    /// Canonicalized, path-traversal-validated script path.
    path: PathBuf,
    interpreter: Option<String>,
    timeout_secs: u64,
    skill_dir: PathBuf,
}

impl ScriptToolBuiltIn {
    /// Construct with path-traversal validation (same security check as
    /// current ScriptExecutionContext::new()). Returns Err if the resolved
    /// path escapes skill_dir/scripts/.
    fn new(skill_dir: &Path, cfg: &ScriptConfig) -> Result<Self, String> {
        let script_path = skill_dir.join("scripts").join(&cfg.file);
        let canonical = script_path.canonicalize()
            .map_err(|e| format!("Script '{}' not found: {}", cfg.file, e))?;
        let scripts_dir = skill_dir.join("scripts").canonicalize()
            .map_err(|e| format!("Scripts directory not found: {}", e))?;
        if !canonical.starts_with(&scripts_dir) {
            return Err(format!(
                "Script '{}' resolves outside scripts/ directory (path traversal blocked)",
                cfg.file
            ));
        }
        Ok(Self { path: canonical, interpreter: cfg.interpreter.clone(),
                  timeout_secs: cfg.timeout_secs, skill_dir: skill_dir.to_path_buf() })
    }
}

#[async_trait]
impl BuiltInTool for ScriptToolBuiltIn {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // Same logic as ScriptExecutionContext::execute_script():
        // build Command, set current_dir, timeout, capture output
    }
}
```

This eliminates the `ScriptExecutionContext` struct and the special `skill_script:*` prefix handling in `ContextualToolExecutor`.

### 3.6 Timeout Exemption

Currently, `COORDINATION_TOOLS` is a hardcoded const in `sandbox/mod.rs`:

```rust
const COORDINATION_TOOLS: &[&str] = &["wait_for_subagents", "check_subagent_status"];
```

After: `RegisteredTool` gains an `exempt_from_timeout: bool` field. Tools that manage their own timeouts (like coordination tools that poll with internal timeouts) set this to `true` at registration. `SandboxManager` checks this field instead of the const array.

This is extensible — any future tool that needs timeout exemption just sets the field at registration.

## 4. Call Site Changes

Every place that currently constructs a `ContextualToolExecutor` or `LeadAgentToolExecutor` switches to constructing a `ToolContext` and passing `Arc<ToolRegistry>` directly.

### 4.1 Regular Agent Path (pipeline_step.rs, node_runner.rs)

**Before:**
```rust
let ctx = ToolExecutionContext { owner_id, task_id, agent_id, db, workspace_id };
let executor = ContextualToolExecutor::new(registry.clone(), ctx);
let sandbox = SandboxManager::with_defaults(Arc::new(executor), bus);
```

**After:**
```rust
let tool_ctx = ToolContext { agent_id, task_id, owner_id, workspace_id };
let sandbox = SandboxManager::with_defaults(registry.clone(), bus);
// tool_ctx passed per-call to sandbox.execute_tool()
```

### 4.2 Lead Agent Path (runner/lead_agent/mod.rs)

The `run_lead_agent()` function in `runner/lead_agent/mod.rs` (lines 163-184) is the actual runtime call site — it constructs `ToolExecutionContext`, `ContextualToolExecutor`, and `LeadAgentToolExecutor`.

**Before:**
```rust
let ctx_exec = ToolExecutionContext { owner_id, task_id, agent_id, db, workspace_id };
let contextual_executor = Arc::new(ContextualToolExecutor::new(tool_registry.clone(), ctx_exec));
let lead_executor = Arc::new(LeadAgentToolExecutor::new(
    spawn_tool, batch_spawn_tool, check_status_tool, wait_tool, contextual_executor,
));
let sandbox = SandboxManager::with_defaults(lead_executor, bus);
```

**After:**
```rust
// Clone registry and register coordination tools for this lead agent session
let mut registry = (*tool_registry).clone();
registry.register_coordination_tools(tracker.clone(), agent_registry.clone(), ...);
let registry = Arc::new(registry);
let tool_ctx = ToolContext { agent_id, task_id, owner_id, workspace_id };
let sandbox = SandboxManager::with_defaults(registry, bus);
```

Note: The dispatcher file `orchestrator/dispatcher/lead_agent.rs` calls `run_lead_agent()` — it does not construct executors itself. Only `runner/lead_agent/mod.rs` needs updating.

### 4.3 Skill Invocation Path

**Before:**
```rust
let scripts = ScriptExecutionContext::new(skill_dir, &configs)?;
let executor = ContextualToolExecutor::with_scripts(registry.clone(), ctx, scripts);
```

**After:**
```rust
let mut registry = (*shared_registry).clone();
for cfg in &configs {
    let tool = ScriptToolBuiltIn::new(skill_dir, cfg)?; // validates path traversal
    registry.register(RegisteredTool {
        definition: script_tool_definition(&cfg.name),
        backend: ToolBackend::BuiltIn(Arc::new(tool)),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
    });
}
let registry = Arc::new(registry);
```

### 4.4 Simple Query Path (query_handler)

**Before:**
```rust
let ctx = ToolExecutionContext { owner_id: None, task_id: None, ... };
let executor = ContextualToolExecutor::new(registry.clone(), ctx);
let sandbox = SandboxManager::with_defaults(Arc::new(executor), bus);
```

**After:**
```rust
let tool_ctx = ToolContext { agent_id: None, task_id: None, owner_id: None, workspace_id: None };
let sandbox = SandboxManager::with_defaults(registry.clone(), bus);
```

## 5. Migration Strategy

The migration is staged to keep the codebase compiling at each step:

1. **Add `ToolContext` + trait extension** — new types, no callers yet
2. **Add `exempt_from_timeout` to `RegisteredTool`** — default `false`, no behavior change
3. **Migrate Category B tools** — each tool gets `execute_with_context()` override, removing corresponding routing from `ContextualToolExecutor`
4. **Migrate Category C tools** — implement `BuiltInTool` on coordination tool structs, add registry helper for lead agent
5. **Migrate workspace tools** — extract from `ContextualToolExecutor` into standalone `BuiltInTool` impls
6. **Update `SandboxManager`** — replace `dyn ToolExecutor` with `Arc<ToolRegistry>`, add `ToolContext` parameter
7. **Update call sites** — switch from executor construction to `ToolContext` + direct registry
8. **Handle skill scripts** — `ScriptToolBuiltIn` + per-invocation clone
9. **Cleanup** — remove `ContextualToolExecutor`, `LeadAgentToolExecutor`, `ToolExecutor` trait, `ToolBackend::Contextual`

## 6. Files Modified

| File | Changes |
|------|---------|
| `tools/registry/mod.rs` | Add `ToolContext`, extend `BuiltInTool` trait, add `execute_with_context()` on `ToolRegistry`, add `exempt_from_timeout` to `RegisteredTool`, remove `ToolBackend::Contextual` |
| `tools/builtins/memory_search.rs` | Override `execute_with_context()`, read `ctx.owner_id` + `ctx.workspace_id` instead of from args |
| `tools/builtins/update_persona/mod.rs` | Remove `owner_id` skip guard (line 62-63). No `execute_with_context()` override needed — becomes Category A. |
| `tools/builtins/mod.rs` | Register `WorkspaceReadTool` and `WorkspaceWriteTool` as `BuiltInTool` (new structs holding `Arc<Database>`), remove `ToolBackend::Contextual` registrations |
| `security/sandbox/mod.rs` | Replace `executor: Arc<dyn ToolExecutor>` with `registry: Arc<ToolRegistry>`. Remove `ToolExecutor` trait. Remove `COORDINATION_TOOLS` const. Use `exempt_from_timeout` field. Change `execute_tool()` signature to take `&ToolContext` instead of `agent_id: &str`. |
| `runner/lead_agent/tools.rs` | Implement `BuiltInTool` on `SpawnSubagentTool`, `CheckSubagentStatusTool`, `WaitForSubagentsTool`, `SpawnSubagentsBatchTool`. Remove `LeadAgentToolExecutor`. Add `register_coordination_tools()` helper. Update `SpawnSubagentTool`'s internal subagent spawn to use `ToolContext` + `Arc<ToolRegistry>` instead of `ContextualToolExecutor`. |
| `runner/lead_agent/mod.rs` | Replace `ToolExecutionContext` + `ContextualToolExecutor` + `LeadAgentToolExecutor` construction with registry clone + `register_coordination_tools()` + `ToolContext` |
| `orchestrator/dispatcher/pipeline_step.rs` | Construct `ToolContext` instead of `ToolExecutionContext` + `ContextualToolExecutor` |
| `dag_executor/node_runner.rs` | Same as pipeline_step.rs |
| `orchestrator/query_handler/simple_query_handler.rs` | Construct empty `ToolContext` instead of empty `ContextualToolExecutor` |
| `orchestrator/skill/invocation.rs` | Clone registry, register `ScriptToolBuiltIn` tools (with path-traversal validation), construct `ToolContext` |
| `tools/contextual_executor/mod.rs` | **Deleted** (all logic absorbed into individual tools + `ScriptToolBuiltIn`) |

## 7. Invariants

- All existing tool behavior is preserved exactly
- No new public API surface beyond `ToolContext` and the trait method
- `SandboxManager` security layers (capability check, sanitization, confirmation, circuit breaker, timeout, events) are unchanged
- Tool definitions exposed to LLMs are unchanged
- Tests updated but coverage unchanged
- No new crate dependencies
