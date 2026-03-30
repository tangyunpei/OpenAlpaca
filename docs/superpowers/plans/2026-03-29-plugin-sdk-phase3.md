# Plugin SDK Phase 3: Skill + Agent Bridges

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable skill plugins and agent plugins to integrate with OpenAlpaca at runtime. Skill plugins handle invocation via `skill/invoke` RPC with tool callback loops. Agent plugins run their own reasoning loops, communicating via `agent/spawn` + `agent/step` + push-based progress.

**Architecture:** Add `SkillSource` enum to `SkillEntry` (file-based vs plugin), add `AgentSource` enum to `AgentTemplate` (file-based vs plugin). The SkillInvocationToolExecutor detects plugin skills and routes to `skill/invoke` RPC instead of the internal agentic loop. The TaskDispatcher detects plugin agents and routes to `agent/spawn`/`agent/step` instead of `run_agentic_loop_routed`.

**Tech Stack:** Rust, tokio, async-trait, serde_json

**Spec:** `docs/superpowers/specs/2026-03-29-plugin-sdk-design.md` (Sections 1, 4, 6)

**Depends on:** Phase 1 + Phase 2

---

## File Map

### New Files

| File | Responsibility |
|---|---|
| `crates/openalpaca_plugins/src/bridge/skill_bridge.rs` | PluginSkillExecutor — proxy skill/invoke with tool callback loop |
| `crates/openalpaca_plugins/src/bridge/agent_bridge.rs` | PluginAgentExecutor — proxy agent/spawn + agent/step |
| `crates/openalpaca_api/src/plugin_traits/skill_executor.rs` | PluginSkillExecutor trait (in API crate to avoid cycles) |
| `crates/openalpaca_api/src/plugin_traits/agent_executor.rs` | PluginAgentExecutor trait (in API crate to avoid cycles) |

### Modified Files

| File | Change |
|---|---|
| `crates/openalpaca_api/src/plugin_traits.rs` | Split into module dir, add skill + agent executor traits |
| `crates/openalpaca_core/src/orchestrator/skill/catalog/mod.rs` | Add `SkillSource` to `SkillEntry`, make paths Optional |
| `crates/openalpaca_core/src/orchestrator/skill/invoke_executor.rs` | Detect plugin skills, route to PluginSkillExecutor |
| `crates/openalpaca_core/src/agent/template/mod.rs` | Add `AgentSource` to `AgentTemplate` |
| `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs` | Detect plugin agents, route to PluginAgentExecutor |
| `crates/openalpaca_plugins/src/bridge/mod.rs` | Export skill + agent bridge types |
| `crates/openalpaca_plugins/src/manager.rs` | Wire skill + agent discovery into hot-load |

---

## Chunk 1: Traits in openalpaca_api

### Task 1: Add PluginSkillExecutor and PluginAgentExecutor traits

These traits live in `openalpaca_api` to avoid circular deps (same pattern as `PluginToolExecutor`).

**Files:**
- Modify: `crates/openalpaca_api/src/plugin_traits.rs` (convert to module dir OR add traits to existing file)
- Modify: `crates/openalpaca_api/src/lib.rs`

- [ ] **Step 1: Add PluginSkillExecutor trait**

Add to `crates/openalpaca_api/src/plugin_traits.rs`:

```rust
/// Trait for executing skills via a plugin subprocess.
#[async_trait]
pub trait PluginSkillExecutor: Send + Sync {
    /// Invoke the skill with a query and available tools.
    /// Returns the skill's output text.
    /// The executor handles the tool callback loop internally:
    /// if the skill requests tools, the executor calls them via the provided
    /// tool_executor callback and sends results back to the skill.
    async fn invoke(
        &self,
        query: &str,
        context: &serde_json::Value,
        tool_executor: &dyn ToolCallbackExecutor,
    ) -> Result<String, String>;

    fn plugin_id(&self) -> &str;
    fn skill_id(&self) -> &str;
}

/// Callback interface for executing tool calls requested by a plugin skill.
#[async_trait]
pub trait ToolCallbackExecutor: Send + Sync {
    async fn execute_tool(&self, tool_name: &str, arguments: &serde_json::Value) -> Result<String, String>;
}
```

- [ ] **Step 2: Add PluginAgentExecutor trait**

Add to the same file:

```rust
/// Trait for executing agent tasks via a plugin subprocess.
#[async_trait]
pub trait PluginAgentExecutor: Send + Sync {
    /// Spawn an agent instance for a task.
    async fn spawn(
        &self,
        instance_id: &str,
        task_id: &str,
        instructions: &str,
        context: &serde_json::Value,
    ) -> Result<bool, String>;

    /// Poll for progress or send tool results.
    /// Returns (status, output) where status is "working", "complete", or "failed".
    async fn step(
        &self,
        instance_id: &str,
        tool_results: Option<&serde_json::Value>,
    ) -> Result<(String, String, Vec<serde_json::Value>), String>;

    /// Stop a running agent instance.
    async fn stop(&self, instance_id: &str) -> Result<(), String>;

    fn plugin_id(&self) -> &str;
    fn agent_id(&self) -> &str;
}
```

- [ ] **Step 3: Verify**

Run: `cargo check -p openalpaca_api`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_api/
git commit -m "feat: add PluginSkillExecutor and PluginAgentExecutor traits

Traits for executing skills and agent tasks via plugin subprocesses.
PluginSkillExecutor supports tool callback loops. PluginAgentExecutor
supports spawn/step/stop lifecycle. Both in openalpaca_api to avoid
circular deps."
```

---

## Chunk 2: SkillEntry Source + SkillCatalog Plugin Support

### Task 2: Add SkillSource to SkillEntry, update SkillCatalog

**Files:**
- Modify: `crates/openalpaca_core/src/orchestrator/skill/catalog/mod.rs`

- [ ] **Step 1: Define SkillSource enum**

Add above `SkillEntry`:
```rust
/// Where a skill's execution logic comes from.
#[derive(Debug, Clone)]
pub enum SkillSource {
    /// Traditional file-based skill (SKILL.md)
    FileBased,
    /// Plugin-backed skill (executed via PluginSkillExecutor)
    Plugin {
        plugin_id: String,
        executor: Arc<dyn openalpaca_api::plugin_traits::PluginSkillExecutor>,
    },
}
```

- [ ] **Step 2: Add source field to SkillEntry, make paths Optional**

```rust
pub struct SkillEntry {
    pub frontmatter: SkillFrontmatter,
    pub skill_md_path: Option<PathBuf>,     // None for plugin skills
    pub skill_dir: Option<PathBuf>,          // None for plugin skills
    pub compiled_triggers: Vec<Regex>,
    pub scope: SkillScope,
    pub source: SkillSource,                 // NEW
}
```

- [ ] **Step 3: Update all SkillEntry construction sites**

In `scan_directory()` and anywhere else `SkillEntry { ... }` is constructed, add `source: SkillSource::FileBased`. Change `skill_md_path` and `skill_dir` from `PathBuf` to `Some(path)`.

- [ ] **Step 4: Update load_full() to handle plugin skills**

In `load_full()`, check `source`. If `Plugin`, return a synthetic `SkillDocument` with the frontmatter and an empty body (or the description as body). If `FileBased`, read from disk as before (unwrap the Option path).

- [ ] **Step 5: Add register_plugin_skill() method**

```rust
pub fn register_plugin_skill(
    &self,
    skill_id: String,
    frontmatter: SkillFrontmatter,
    executor: Arc<dyn openalpaca_api::plugin_traits::PluginSkillExecutor>,
    plugin_id: String,
) {
    let compiled = compile_triggers_from_frontmatter(&frontmatter);
    let entry = SkillEntry {
        frontmatter: frontmatter.clone(),
        skill_md_path: None,
        skill_dir: None,
        compiled_triggers: compiled,
        scope: SkillScope::User,
        source: SkillSource::Plugin { plugin_id, executor },
    };
    // Insert and update indexes (same as scan_directory does for file skills)
    let mut entries = self.entries.write().unwrap();
    entries.insert(skill_id.clone(), entry);
    // Update command/alias indexes from frontmatter
    // ...
}
```

- [ ] **Step 6: Fix all compilation errors**

Run `cargo check -p openalpaca_core` and fix any sites that access `skill_md_path` or `skill_dir` without handling the `Option`.

- [ ] **Step 7: Verify**

Run: `cargo check --all-targets`
Run: `cargo test -p openalpaca_core`
Expected: All pass

- [ ] **Step 8: Commit**

```bash
git add crates/openalpaca_core/
git commit -m "refactor: add SkillSource enum to SkillEntry for plugin skills

SkillEntry now has source: SkillSource (FileBased or Plugin).
Paths become Optional for plugin skills. register_plugin_skill()
method added to SkillCatalog. load_full() handles both sources."
```

---

## Chunk 3: AgentTemplate Source + Dispatcher Plugin Path

### Task 3: Add AgentSource to AgentTemplate

**Files:**
- Modify: `crates/openalpaca_core/src/agent/template/mod.rs`

- [ ] **Step 1: Define AgentSource enum**

```rust
/// Where an agent's execution logic comes from.
#[derive(Debug, Clone)]
pub enum AgentSource {
    /// Internal agent running the built-in agentic loop
    Internal,
    /// Plugin-backed agent running an external reasoning loop
    Plugin {
        plugin_id: String,
        executor: Arc<dyn openalpaca_api::plugin_traits::PluginAgentExecutor>,
    },
}

impl Default for AgentSource {
    fn default() -> Self { Self::Internal }
}
```

- [ ] **Step 2: Add source field to AgentTemplate**

```rust
pub struct AgentTemplate {
    pub frontmatter: AgentTemplateFrontmatter,
    pub body: String,
    pub sections: HashMap<String, String>,
    pub source: AgentSource,  // NEW
}
```

- [ ] **Step 3: Update all AgentTemplate construction sites**

Add `source: AgentSource::Internal` wherever `AgentTemplate { ... }` is built. Search for `AgentTemplate {` across the codebase.

- [ ] **Step 4: Verify**

Run: `cargo check --all-targets`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/
git commit -m "refactor: add AgentSource enum to AgentTemplate for plugin agents

AgentTemplate now has source: AgentSource (Internal or Plugin).
Defaults to Internal for all existing templates."
```

---

### Task 4: Add plugin agent dispatch path in pipeline_step

**Files:**
- Modify: `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs`

- [ ] **Step 1: At the top of execute_pipeline_step(), detect plugin agents**

Before the existing agentic loop call (around line 450), check if the agent's template has a plugin source:

```rust
// Check if this is a plugin-backed agent
let agent_template = pctx.agent_registry.get_template(&agent.template_id);
if let Some(template) = agent_template {
    if let AgentSource::Plugin { ref executor, .. } = template.source {
        // Dispatch to plugin agent instead of internal agentic loop
        return execute_plugin_agent_step(
            executor.as_ref(),
            &agent.id,
            &pctx.task_id,
            &system_prompt_text,
            &previous_output,
            &pctx.tool_registry,
        ).await;
    }
}
```

- [ ] **Step 2: Implement execute_plugin_agent_step()**

Add a helper function that:
1. Calls `executor.spawn(instance_id, task_id, instructions, context)`
2. Loops calling `executor.step(instance_id, tool_results)`
3. If step returns tool_calls, execute them via tool_registry and send results
4. If step returns "complete", return the output
5. If step returns "failed", return error
6. Max iterations: 50 (configurable)

```rust
async fn execute_plugin_agent_step(
    executor: &dyn PluginAgentExecutor,
    agent_id: &str,
    task_id: &str,
    instructions: &str,
    context: &str,
    tool_registry: &ToolRegistry,
) -> Result<String, String> {
    let ctx = serde_json::json!({ "previous_output": context });

    let accepted = executor.spawn(agent_id, task_id, instructions, &ctx).await?;
    if !accepted {
        return Err("Plugin agent rejected the task".to_string());
    }

    let mut tool_results: Option<serde_json::Value> = None;
    for _ in 0..50 {
        let (status, output, tool_calls) = executor.step(agent_id, tool_results.as_ref()).await?;

        match status.as_str() {
            "complete" => return Ok(output),
            "failed" => return Err(output),
            "tool_request" => {
                // Execute requested tools
                let mut results = Vec::new();
                for call in &tool_calls {
                    let name = call.get("tool").and_then(|t| t.as_str()).unwrap_or("");
                    let args = call.get("arguments").cloned().unwrap_or_default();
                    let result = tool_registry.execute(name, &args).await;
                    results.push(serde_json::json!({
                        "tool": name,
                        "result": result.unwrap_or_else(|e| e),
                    }));
                }
                tool_results = Some(serde_json::json!(results));
            }
            _ => {
                // "working" — poll again
                tool_results = None;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }

    Err("Plugin agent exceeded max iterations".to_string())
}
```

- [ ] **Step 3: Add AgentRegistry::get_template() if it doesn't exist**

Check if AgentRegistry has a method to retrieve a template by ID. If not, add:
```rust
pub fn get_template(&self, template_id: &str) -> Option<AgentTemplate> {
    self.templates.lock().unwrap_or_else(|p| p.into_inner())
        .get(template_id).cloned()
}
```

- [ ] **Step 4: Verify**

Run: `cargo check --all-targets`
Run: `cargo test -p openalpaca_core`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/
git commit -m "feat: add plugin agent dispatch path in pipeline_step

execute_pipeline_step now detects plugin-backed agents and routes
to PluginAgentExecutor.spawn/step instead of the internal agentic
loop. Tool callback loop handles tool_request status."
```

---

## Chunk 4: Plugin Bridge Implementations

### Task 5: Implement PluginSkillBridge and PluginAgentBridge

**Files:**
- Create: `crates/openalpaca_plugins/src/bridge/skill_bridge.rs`
- Create: `crates/openalpaca_plugins/src/bridge/agent_bridge.rs`
- Modify: `crates/openalpaca_plugins/src/bridge/mod.rs`

- [ ] **Step 1: Write skill_bridge.rs**

Implements `PluginSkillExecutor` — proxies `skill/invoke` RPC with tool callback loop (max 20 iterations).

```rust
pub struct PluginSkillBridge {
    plugin_id: String,
    skill_id: String,
    channel: StdioChannel,
}
```

The `invoke()` method:
1. Sends `skill/invoke` with query, context, available_tools
2. If response has `tool_calls`, executes them via `tool_executor` callback
3. Sends `skill/invoke_continue` with tool_results
4. Repeats until response has no `tool_calls` or max iterations hit
5. Returns the final `result` text

- [ ] **Step 2: Write agent_bridge.rs**

Implements `PluginAgentExecutor` — proxies `agent/spawn`, `agent/step`, `agent/tool_results`, `agent/stop`.

```rust
pub struct PluginAgentBridge {
    plugin_id: String,
    agent_id: String,
    channel: StdioChannel,
}
```

- [ ] **Step 3: Update bridge/mod.rs**

```rust
pub mod skill_bridge;
pub mod agent_bridge;
pub use skill_bridge::PluginSkillBridge;
pub use agent_bridge::PluginAgentBridge;
```

- [ ] **Step 4: Verify**

Run: `cargo check -p openalpaca_plugins`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_plugins/
git commit -m "feat: add PluginSkillBridge and PluginAgentBridge

PluginSkillBridge proxies skill/invoke with tool callback loop.
PluginAgentBridge proxies agent/spawn + agent/step lifecycle."
```

---

## Chunk 5: Wire into PluginManager

### Task 6: Extend PluginManager with skill + agent discovery

**Files:**
- Modify: `crates/openalpaca_plugins/src/manager.rs`

- [ ] **Step 1: Add SkillCatalog and AgentRegistry to PluginManager**

The PluginManager needs access to register plugin skills and agents.

- [ ] **Step 2: Add skill discovery to hot-load sequence**

After provider discovery, if `manifest.types.skill`:
1. Call `skill/info` RPC
2. Parse response into SkillFrontmatter-compatible data
3. Create PluginSkillBridge
4. Call `skill_catalog.register_plugin_skill()`
5. Track in `PluginState.registered_skills`

- [ ] **Step 3: Add agent discovery to hot-load sequence**

If `manifest.types.agent`:
1. Call `agent/info` RPC
2. Parse response into AgentTemplate-compatible data
3. Create PluginAgentBridge
4. Register template with AgentSource::Plugin
5. Track in `PluginState.registered_agents`

- [ ] **Step 4: Update unload to clean up skills and agents**

- [ ] **Step 5: Verify**

Run: `cargo check --all-targets`
Run: `cargo test --workspace`
Expected: All pass

- [ ] **Step 6: Commit**

```bash
git add crates/openalpaca_plugins/ crates/openalpaca_core/
git commit -m "feat: extend PluginManager with skill and agent discovery

Hot-load sequence now calls skill/info and agent/info.
Plugin skills registered in SkillCatalog with PluginSkillBridge.
Plugin agents registered in AgentRegistry with PluginAgentBridge."
```

---

## Final Verification

- [ ] **Full workspace build**: `cargo build --all-targets`
- [ ] **All tests pass**: `cargo test --workspace`
- [ ] **No new warnings**: `cargo clippy --all-targets`
