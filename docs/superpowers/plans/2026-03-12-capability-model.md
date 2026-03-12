# Capability Model & Skill Composition Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the overloaded "skills" concept with a capability model: tools declare capabilities, agents/skills require capabilities, and skills can invoke other skills.

**Architecture:** Tool-side capability declaration with intersection-based resolution replaces 1:1 name matching. Agent configs rename `skills` → `capabilities`. Skills gain `requires_capabilities` and `depends_on` for composition via nested agentic loops.

**Tech Stack:** Rust, serde, async_trait, tokio, openalpaca_llm

**Spec:** `docs/superpowers/specs/2026-03-12-capability-model-design.md`

---

## Chunk 1: Tool Capability Infrastructure

### Task 1: Add `provides_capabilities` to RegisteredTool and ToolBackend::Contextual

**Files:**
- Modify: `crates/openalpaca_core/src/tools/registry/mod.rs`

- [ ] **Step 1: Add `provides_capabilities` field to `RegisteredTool`**

In `crates/openalpaca_core/src/tools/registry/mod.rs`, update the struct (currently lines 29-32):

```rust
pub struct RegisteredTool {
    pub definition: ToolDefinition,
    pub backend: ToolBackend,
    pub provides_capabilities: Vec<String>,
}
```

- [ ] **Step 2: Add `Contextual` variant to `ToolBackend`**

In the same file, add to the `ToolBackend` enum (currently lines 7-20):

```rust
pub enum ToolBackend {
    BuiltIn(Arc<dyn BuiltInTool>),
    Http {
        method: String,
        url: String,
        headers: HashMap<String, String>,
        timeout_secs: u64,
    },
    Command {
        command: String,
        args_template: Option<String>,
        timeout_secs: u64,
    },
    /// Tool whose execution is handled by ContextualToolExecutor at runtime.
    /// Definition is registered for capability-based resolution, but execution
    /// is delegated (e.g., workspace_read, workspace_write).
    Contextual,
}
```

- [ ] **Step 3: Make ToolRegistry and RegisteredTool cloneable**

`ToolRegistry` and `RegisteredTool` must implement `Clone` for per-invocation registry cloning in Task 12. Add `Clone` to:
- `RegisteredTool` — `ToolDefinition` is already Clone; `ToolBackend` needs Clone impl (all variants are Clone: `Arc<dyn BuiltInTool>` is Clone, `HashMap/String/u64/Option<String>` are Clone, `Contextual` is unit)
- `ToolRegistry` — `HashMap<String, RegisteredTool>` is Clone when values are; `reqwest::Client` is Clone (cheap Arc-based)

```rust
#[derive(Clone)]
pub struct RegisteredTool { ... }

#[derive(Clone)]
pub enum ToolBackend { ... }

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            http_client: self.http_client.clone(),
        }
    }
}
```

- [ ] **Step 4: Update `execute()` to handle Contextual variant**

In the `execute()` method (line ~139), add a match arm for `Contextual`:

```rust
ToolBackend::Contextual => {
    Err(format!("[tool_error] Tool '{}' requires contextual execution — must be called through ContextualToolExecutor", tool_name))
}
```

This is a safety net — contextual tools should be intercepted by `ContextualToolExecutor` before reaching the registry's execute method.

- [ ] **Step 5: Fix all existing test RegisteredTool constructions**

Every test that creates a `RegisteredTool` must add `provides_capabilities: vec![]`. Search the test files:
- `crates/openalpaca_core/src/tools/registry/tests.rs` — update the `make_tool()` helper (line ~17) to include `provides_capabilities: vec![]`, and update any other direct `RegisteredTool { ... }` literals
- `crates/openalpaca_core/src/tools/config/mod.rs` — the `load_tools_from_file()` function that constructs RegisteredTool (line ~104)

For the `make_tool()` helper, just add the field. For `load_tools_from_file()`, see Task 3.

- [ ] **Step 6: Build and verify**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: Clean compilation (all tests and code compile)

- [ ] **Step 7: Commit**

```bash
git add crates/openalpaca_core/src/tools/registry/mod.rs crates/openalpaca_core/src/tools/registry/tests.rs
git commit -m "feat(capability): add provides_capabilities to RegisteredTool + Contextual backend"
```

---

### Task 2: Add `tools_for_capabilities()` to ToolRegistry

**Files:**
- Modify: `crates/openalpaca_core/src/tools/registry/mod.rs`
- Modify: `crates/openalpaca_core/src/tools/registry/tests.rs`

- [ ] **Step 1: Write failing tests**

Add to `crates/openalpaca_core/src/tools/registry/tests.rs`:

Note: The existing test file uses `MockBuiltIn { response: String }` — use that, NOT `StubTool` (which doesn't exist).

Helper to reduce boilerplate (add near existing `make_tool` helper):
```rust
fn make_tool_with_caps(name: &str, caps: Vec<&str>) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: name.to_string(),
            description: format!("{} tool", name),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: "ok".to_string(),
        })),
        provides_capabilities: caps.into_iter().map(String::from).collect(),
    }
}
```

Tests:
```rust
#[test]
fn test_tools_for_capabilities_basic() {
    let mut registry = ToolRegistry::new();
    registry.register(make_tool_with_caps("file_read", vec!["file_read"]));
    registry.register(make_tool_with_caps("web_search", vec!["web_access"]));

    let result = registry.tools_for_capabilities(&["file_read".to_string()]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "file_read");
}

#[test]
fn test_tools_for_capabilities_multi_capability_tool() {
    let mut registry = ToolRegistry::new();
    registry.register(make_tool_with_caps("web_fetch", vec!["web_access"]));

    let result = registry.tools_for_capabilities(&["web_access".to_string()]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "web_fetch");

    let result = registry.tools_for_capabilities(&["shell_execute".to_string()]);
    assert!(result.is_empty());
}

#[test]
fn test_tools_for_capabilities_empty_returns_empty() {
    let mut registry = ToolRegistry::new();
    registry.register(make_tool_with_caps("file_read", vec!["file_read"]));

    let result = registry.tools_for_capabilities(&[]);
    assert!(result.is_empty());
}

#[test]
fn test_tools_for_capabilities_no_capability_tools_excluded() {
    let mut registry = ToolRegistry::new();
    registry.register(make_tool_with_caps("orphan_tool", vec![]));

    let result = registry.tools_for_capabilities(&["file_read".to_string()]);
    assert!(result.is_empty());
}

#[test]
fn test_tools_for_capabilities_with_deny_basic() {
    let mut registry = ToolRegistry::new();
    registry.register(make_tool_with_caps("file_read", vec!["file_read"]));
    registry.register(make_tool_with_caps("web_search", vec!["web_access"]));
    registry.register(make_tool_with_caps("shell", vec!["shell_execute"]));

    // Request file_read + web_access, deny web_access
    let result = registry.tools_for_capabilities_with_deny(
        &["file_read".to_string(), "web_access".to_string()],
        &["web_access".to_string()],
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "file_read");
}

#[test]
fn test_tools_for_capabilities_with_deny_excludes_any_denied() {
    let mut registry = ToolRegistry::new();
    // Tool provides both file_read AND file_write
    registry.register(make_tool_with_caps("file_rw", vec!["file_read", "file_write"]));
    registry.register(make_tool_with_caps("reader", vec!["file_read"]));

    // Request file_read, deny file_write — tool providing both is excluded
    let result = registry.tools_for_capabilities_with_deny(
        &["file_read".to_string()],
        &["file_write".to_string()],
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "reader");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p openalpaca_core -- tools::registry::tests::test_tools_for_capabilities --no-run`
Expected: Compilation error — `tools_for_capabilities` method doesn't exist

- [ ] **Step 3: Implement `tools_for_capabilities()`**

Add to `ToolRegistry` impl in `crates/openalpaca_core/src/tools/registry/mod.rs`:

```rust
/// Returns tool definitions for all tools whose `provides_capabilities`
/// intersects with the requested capabilities. Empty capabilities returns empty.
pub fn tools_for_capabilities(&self, capabilities: &[String]) -> Vec<ToolDefinition> {
    if capabilities.is_empty() {
        return vec![];
    }
    self.tools
        .values()
        .filter(|tool| {
            tool.provides_capabilities
                .iter()
                .any(|cap| capabilities.contains(cap))
        })
        .map(|tool| tool.definition.clone())
        .collect()
}

/// Returns tool definitions for all tools whose `provides_capabilities`
/// intersects with the requested capabilities, excluding tools that provide
/// any denied capability.
pub fn tools_for_capabilities_with_deny(
    &self,
    capabilities: &[String],
    denied: &[String],
) -> Vec<ToolDefinition> {
    if capabilities.is_empty() {
        return vec![];
    }
    self.tools
        .values()
        .filter(|tool| {
            tool.provides_capabilities
                .iter()
                .any(|cap| capabilities.contains(cap))
        })
        .filter(|tool| {
            !tool.provides_capabilities
                .iter()
                .any(|cap| denied.contains(cap))
        })
        .map(|tool| tool.definition.clone())
        .collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p openalpaca_core -- tools::registry::tests::test_tools_for_capabilities`
Expected: All 6 new tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/tools/registry/mod.rs crates/openalpaca_core/src/tools/registry/tests.rs
git commit -m "feat(capability): add tools_for_capabilities() to ToolRegistry"
```

---

### Task 3: Built-in tools declare capabilities + workspace registration + TOML parsing

**Files:**
- Modify: `crates/openalpaca_core/src/tools/builtins/mod.rs`
- Modify: `crates/openalpaca_core/src/tools/builtins/file_ops.rs`
- Modify: `crates/openalpaca_core/src/tools/builtins/web_search.rs`
- Modify: `crates/openalpaca_core/src/tools/builtins/web_fetch.rs`
- Modify: `crates/openalpaca_core/src/tools/builtins/shell_execute.rs`
- Modify: `crates/openalpaca_core/src/tools/builtins/memory_search.rs`
- Modify: `crates/openalpaca_core/src/tools/builtins/update_persona/mod.rs`
- Modify: `crates/openalpaca_core/src/tools/builtins/send.rs`
- Modify: `crates/openalpaca_core/src/tools/config/mod.rs`

- [ ] **Step 1: Update each tool module — add `provides_capabilities` to RegisteredTool**

`RegisteredTool` is constructed in individual tool module files, NOT in `builtins/mod.rs`. Each file has a function that returns `RegisteredTool`. Add `provides_capabilities` to each:

| File | Function | `provides_capabilities` |
|------|----------|------------------------|
| `file_ops.rs:49` | `file_read_tool()` | `vec!["file_read".into()]` |
| `file_ops.rs:156` | `file_write_tool()` | `vec!["file_write".into()]` |
| `web_search.rs:100` | `web_search_tool()` | `vec!["web_access".into()]` |
| `web_fetch.rs:101` | `web_fetch_tool()` | `vec!["web_access".into()]` |
| `shell_execute.rs:67` | `shell_execute_tool()` | `vec!["shell_execute".into()]` |
| `memory_search.rs:106` | `memory_search_tool()` | `vec!["memory_read".into()]` |
| `update_persona/mod.rs:178` | `update_persona_tool()` | `vec!["persona_write".into()]` |
| `send.rs:112` | `send_tool()` | `vec!["messaging".into()]` |

For each, add the field to the existing `RegisteredTool { definition, backend }` struct literal.

- [ ] **Step 2: Register workspace tools in `builtin_tools()`**

Ensure `ToolBackend` is imported: `use crate::tools::registry::ToolBackend;` (add if not already present).

Add workspace tool registration at the end of `builtin_tools()`. Use the existing `workspace_tool_definitions()` function to get definitions, then create `RegisteredTool` entries with `ToolBackend::Contextual`:

```rust
// Register workspace tools (execution handled by ContextualToolExecutor)
for def in workspace_tool_definitions() {
    let cap = if def.name == "workspace_read" {
        vec!["workspace_read".to_string()]
    } else {
        vec!["workspace_write".to_string()]
    };
    tools.push(RegisteredTool {
        definition: def,
        backend: ToolBackend::Contextual,
        provides_capabilities: cap,
    });
}
```

- [ ] **Step 3: Update TOML config parsing to include `provides_capabilities`**

In `crates/openalpaca_core/src/tools/config/mod.rs`, add to `ToolConfig` struct (line ~13):

```rust
#[derive(Deserialize)]
pub struct ToolConfig {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub backend: ToolBackendConfig,
    #[serde(default)]
    pub provides_capabilities: Vec<String>,
}
```

Then in `load_tools_from_file()` (line ~104), pass it through to `RegisteredTool`:

```rust
RegisteredTool {
    definition: ToolDefinition { ... },
    backend: ...,
    provides_capabilities: tool.provides_capabilities,
}
```

- [ ] **Step 4: Build and run all tool tests**

Run: `cargo test -p openalpaca_core -- tools::`
Expected: All existing + new tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/tools/builtins/ crates/openalpaca_core/src/tools/config/mod.rs
git commit -m "feat(capability): built-in tools declare capabilities, workspace tools registered"
```

---

## Chunk 2: Agent Rename (skills → capabilities)

### Task 4: Rename `Skill` → `Capability` in agent/subagent

**Files:**
- Modify: `crates/openalpaca_core/src/agent/subagent/mod.rs`

- [ ] **Step 1: Rename struct and field**

In `crates/openalpaca_core/src/agent/subagent/mod.rs`:

Rename `Skill` struct (lines 63-69) to `Capability`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub name: String,
    pub category: String,
    pub proficiency: f32,
}
```

Rename `SubAgent.skills` field (line 22) to `SubAgent.capabilities`:
```rust
pub capabilities: Vec<Capability>,
```

- [ ] **Step 2: Update all references in the same file**

Search-and-replace within `subagent/mod.rs`:
- `Skill {` → `Capability {`
- `.skills` → `.capabilities` (field access)
- `skills:` → `capabilities:` (struct literal fields)
- Update `SubAgent::from_config()` (line ~155):
  - Change: `let skills: Vec<Skill>` → `let capabilities: Vec<Capability>`
  - Change: `skills: skills,` → `capabilities: capabilities,`
  - **Do NOT rename `config.skills_json`** — this is the `openalpaca_storage::SubAgentConfig` field/DB column. Serde deserializes by field name inside the JSON (`name`, `category`, `proficiency`), not struct name, so renaming `Skill` to `Capability` does NOT break existing DB data. No DB migration needed.

- [ ] **Step 3: Fix compilation errors across crate**

This rename will cause errors in many files. Fix them one by one. The main callers:
- `agent/config/mod.rs` — `into_subagent()`, `from_subagent()`
- `agent/template/mod.rs` — `to_subagent()`
- `agent/registry/mod.rs` — `find_by_skill()`
- `tools/mod.rs` — `resolve_agent_tools()`
- `orchestrator/skill/matcher/mod.rs` — `match_skills()`
- `test_util.rs` — `make_agent()`

For now, do a mechanical rename: `Skill` → `Capability`, `.skills` → `.capabilities`. Don't change function names yet — that's Task 5.

- [ ] **Step 4: Build to verify compilation**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: Clean compilation

- [ ] **Step 5: Run tests**

Run: `cargo test -p openalpaca_core -- agent::`
Expected: All agent tests pass

- [ ] **Step 6: Commit**

```bash
git add -A crates/openalpaca_core/src/
git commit -m "refactor(agent): rename Skill struct to Capability, skills field to capabilities"
```

---

### Task 5: Rename methods and config parsing

**Files:**
- Modify: `crates/openalpaca_core/src/agent/config/mod.rs`
- Modify: `crates/openalpaca_core/src/agent/config/tests.rs`
- Modify: `crates/openalpaca_core/src/agent/template/mod.rs`
- Modify: `crates/openalpaca_core/src/agent/template/tests.rs`
- Modify: `crates/openalpaca_core/src/agent/registry/mod.rs`
- Modify: `crates/openalpaca_core/src/agent/registry/tests.rs`
- Modify: `crates/openalpaca_core/src/test_util.rs`

- [ ] **Step 1: Rename in config/mod.rs**

- `AgentSkillsConfig` → `AgentCapabilitiesConfig`
- Field `skills: AgentSkillsConfig` → `capabilities: AgentCapabilitiesConfig` in `AgentConfigFile`
- **Hard migration — do NOT add `serde(alias = "skills")`**. All config files are updated in Task 6.
- Update `into_subagent()` and `from_subagent()` to use new names

- [ ] **Step 2: Rename in template/mod.rs**

- `AgentTemplateFrontmatter.skills` → `AgentTemplateFrontmatter.capabilities`
- `AgentTemplateFrontmatter.denied_skills` → `AgentTemplateFrontmatter.denied_capabilities`
- Update `parse_agent_frontmatter_lines()` — parse `capabilities:` and `denied_capabilities:` keys
- Update `render_agent_markdown()` — write `capabilities:` and `denied_capabilities:`
- Update `to_subagent()` — use new field names

- [ ] **Step 3: Rename in registry/mod.rs**

- `find_templates_by_skill()` → `find_templates_by_capability()`
- `find_by_skill()` → `find_by_capability()`
- Update internal matching: `frontmatter.skills` → `frontmatter.capabilities`, `agent.capabilities` (already renamed in Task 4)

- [ ] **Step 4: Update registry/tests.rs**

- `test_find_by_skill` → `test_find_by_capability`
- `test_find_templates_by_skill` → `test_find_templates_by_capability`
- Update all test bodies to use new method names and field names

- [ ] **Step 5: Update config/tests.rs**

In `agent/config/tests.rs`, update ALL embedded TOML test data and assertions:
- `[skills]` section → `[capabilities]` in all `sample_toml()` and inline TOML strings (4+ embedded TOML blocks)
- `config.skills.assigned` → `config.capabilities.assigned` (all assertions)
- `config.skills.denied` → `config.capabilities.denied`
- `agent.skills.len()` → `agent.capabilities.len()` (multiple assertions)
- `template.frontmatter.skills` → `template.frontmatter.capabilities`
- `template.frontmatter.denied_skills` → `template.frontmatter.denied_capabilities`

- [ ] **Step 6: Update template/tests.rs**

In `agent/template/tests.rs`, update ALL YAML test constants and assertions:
- `VALID_AGENT`: `skills:` → `capabilities:`, `denied_skills:` → `denied_capabilities:`
- `MINIMAL_AGENT`: no skills/capabilities section (no change needed)
- `SINGLETON_AGENT`: `skills:` → `capabilities:`
- `fm.skills` → `fm.capabilities` (lines 78, 110, 126)
- `fm.denied_skills` → `fm.denied_capabilities` (lines 79, 111)
- All `sample_template()` YAML: `skills:` → `capabilities:`, `denied_skills:` → `denied_capabilities:`

- [ ] **Step 7: Update test_util.rs**

- `make_agent()` parameter name: `skills: Vec<&str>` → `capabilities: Vec<&str>`
- `Skill { ... }` → `Capability { ... }`
- `skills: capabilities.into_iter()...` → `capabilities: capabilities.into_iter()...`
- `template_from_agent()`: update `.skills` → `.capabilities`

- [ ] **Step 8: Update orchestrator files with `.skills` references**

These files reference `agent.skills` and must be updated to `agent.capabilities`:

- `orchestrator/task_planner/prompt.rs` (line 18): `agent.skills` → `agent.capabilities`, `skills_str` → `capabilities_str`
- `orchestrator/dispatcher/pipeline_step.rs` (line 223): `agent.skills` → `agent.capabilities`, update tracing message

- [ ] **Step 9: Fix any remaining compilation errors**

Run: `cargo check -p openalpaca_core --all-targets`
Fix any remaining references to old names. Check:
- `orchestrator/skill/matcher/mod.rs` and its tests
- `orchestrator/dispatcher/core.rs`
- `orchestrator/handlers.rs`
- `orchestrator/mod.rs`

- [ ] **Step 10: Run full test suite**

Run: `cargo test -p openalpaca_core`
Expected: All tests pass

- [ ] **Step 11: Commit**

```bash
git add -A crates/openalpaca_core/
git commit -m "refactor(agent): rename skills→capabilities in config, template, registry, tests"
```

---

### Task 6: Update agent config files

**Files:**
- Modify: `config/agents/*.md` (all 9 files)

**IMPORTANT ORDERING:** Task 6 MUST be committed together with Task 8 (or after it). Changing config files to use `capabilities: ["orchestration"]` while code still calls `find_templates_by_capability("lead_orchestration")` will break lead agent dispatch at runtime. If implementing sequentially, do Task 8 Step 3 (the "lead_orchestration" → "orchestration" string change) before committing Task 6.

- [ ] **Step 1: Update all agent configs**

For each agent config in `config/agents/`, rename:
- `skills:` → `capabilities:`
- `denied_skills:` → `denied_capabilities:`
- Map old tool names to capability names per the migration table:
  - `web_search` → `web_access`
  - `web_fetch` → `web_access`
  - `memory_search` → `memory_read`
  - `send` → `messaging`
  - `update_persona` → `persona_write`
  - `lead_orchestration`, `spawn_subagent`, `check_subagent_status`, `wait_for_subagents` → `orchestration`
  - `file_read`, `file_write`, `shell_execute`, `workspace_read`, `workspace_write` — unchanged

Apply this to each file. Example for `code_agent.md`:
```yaml
# Before
skills:
  - file_read
  - file_write
  - shell_execute
  - memory_search
  - workspace_read
  - workspace_write
denied_skills:
  - web_search
  - web_fetch

# After
capabilities:
  - file_read
  - file_write
  - shell_execute
  - memory_read
  - workspace_read
  - workspace_write
denied_capabilities:
  - web_access
```

For `lead_agent.md`, special mapping:
```yaml
# Before
skills:
  - lead_orchestration
  - spawn_subagent
  - check_subagent_status
  - wait_for_subagents
  - memory_search
  - workspace_read
  - workspace_write

# After
capabilities:
  - orchestration
  - memory_read
  - workspace_read
  - workspace_write
```

- [ ] **Step 2: Verify configs parse correctly**

Run: `cargo test -p openalpaca_core -- agent::template`
Expected: Template parsing tests pass (or if they parse from disk, verify no panics)

- [ ] **Step 3: Commit**

```bash
git add config/agents/
git commit -m "refactor(config): rename skills→capabilities in all agent templates"
```

---

## Chunk 3: Capability-Based Resolution

### Task 7: Rewrite `resolve_agent_tools()` with capability intersection

**Files:**
- Modify: `crates/openalpaca_core/src/tools/mod.rs`

- [ ] **Step 1: Rewrite `resolve_agent_tools()`**

Replace the current function (lines 26-53) with:

```rust
/// Resolve tools for an agent based on its declared capabilities.
/// Uses capability intersection: a tool is included if any of its
/// `provides_capabilities` matches any of the agent's capabilities,
/// and none of the tool's capabilities are in the agent's denied list.
pub fn resolve_agent_tools(
    agent: &SubAgent,
    tool_registry: &Arc<ToolRegistry>,
) -> Vec<ToolDefinition> {
    let caps: Vec<String> = agent.capabilities.iter().map(|c| c.name.clone()).collect();
    let denied: Vec<String> = agent.constraints.denied_capabilities.clone();
    tool_registry.tools_for_capabilities_with_deny(&caps, &denied)
}
```

This replaces:
- The old `definitions_for_skills()` call
- The special-casing for workspace_read/workspace_write
- The special-casing for memory_search

All tools are now resolved uniformly by capability intersection.

- [ ] **Step 2: Remove `definitions_for_skills()` from ToolRegistry**

In `crates/openalpaca_core/src/tools/registry/mod.rs`, remove the `definitions_for_skills()` method (lines 89-119) and its `RUNTIME_TOOLS` constant.

- [ ] **Step 3: Update tests that used `definitions_for_skills()`**

In `crates/openalpaca_core/src/tools/registry/tests.rs`, update or remove:
- `test_definitions_for_skills` → rewrite to use `tools_for_capabilities`
- `test_definitions_for_skills_no_match_returns_empty` → rewrite
- `test_definitions_for_empty_skills_returns_empty` → already covered by `test_tools_for_capabilities_empty_returns_empty`
- `test_definitions_for_skills_warns_on_mismatch` → remove (no longer applicable)

- [ ] **Step 4: Fix any callers of `definitions_for_skills()`**

Search crate for remaining callers:
```
grep -r "definitions_for_skills" crates/openalpaca_core/
```
Fix each to use `tools_for_capabilities()` or `tools_for_capabilities_with_deny()`.

- [ ] **Step 5: Build and run tests**

Run: `cargo test -p openalpaca_core`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/openalpaca_core/src/tools/
git commit -m "feat(capability): rewrite resolve_agent_tools with capability intersection"
```

---

### Task 8: Update SkillMatcher, lead agent, dispatcher, and all "lead_orchestration" → "orchestration" references

**Files:**
- Modify: `crates/openalpaca_core/src/orchestrator/skill/matcher/mod.rs`
- Modify: `crates/openalpaca_core/src/orchestrator/skill/matcher/tests.rs`
- Modify: `crates/openalpaca_core/src/orchestrator/dispatcher/core.rs`
- Modify: `crates/openalpaca_core/src/orchestrator/dispatcher/lead_agent.rs`
- Modify: `crates/openalpaca_core/src/orchestrator/dispatcher/tests.rs`
- Modify: `crates/openalpaca_core/src/runner/lead_agent/prompt.rs`
- Modify: `crates/openalpaca_core/src/runner/lead_agent/tools.rs`
- Modify: `crates/openalpaca_core/src/runner/lead_agent/tests.rs`
- Modify: `crates/openalpaca_core/src/agent/template/tests.rs`

- [ ] **Step 1: Update SkillMatcher**

In `matcher/mod.rs`, update `match_skills()` (line 54):
```rust
// Before
agent.skills.iter().any(|s| &s.name == *skill)
// After
agent.capabilities.iter().any(|c| &c.name == *skill)
```

Consider renaming `match_skills()` → `match_capabilities()` and updating all callers.

- [ ] **Step 2: Update matcher tests**

In `matcher/tests.rs`, update all `make_agent()` calls if the parameter name changed in test_util.

- [ ] **Step 3: Update "lead_orchestration" → "orchestration" across codebase**

This is a **semantic** rename — not just a field rename. The string `"lead_orchestration"` was a skill name, now becomes the capability name `"orchestration"`. Update these locations:

**`orchestrator/dispatcher/lead_agent.rs` (line 39):**
```rust
// Before
.find_templates_by_skill("lead_orchestration");
// After
.find_templates_by_capability("orchestration");
```

**`orchestrator/dispatcher/tests.rs` (lines ~174, 207-225):**
Update all test agents that use `"lead_orchestration"` to use `"orchestration"`.

**`test_util.rs` (line 34):**
```rust
// Before
let is_lead = agent.skills.iter().any(|s| s.name == "lead_orchestration");
// After
let is_lead = agent.capabilities.iter().any(|c| c.name == "orchestration");
```

**`agent/template/tests.rs` (lines ~55, 126):**
Update YAML test data and assertions from `"lead_orchestration"` to `"orchestration"`.

**`runner/lead_agent/prompt.rs` (lines 34-42):**
Rename `fm.skills` → `fm.capabilities`, `skills_str` → `capabilities_str`:
```rust
// Before
let skills_str = if fm.skills.is_empty() {
    "none".to_string()
} else {
    fm.skills.join(", ")
};
prompt.push_str(&format!(
    "- id=\"{}\" name=\"{}\" skills=[{}]: {}\n",
    fm.id, fm.name, skills_str, fm.description
));

// After
let capabilities_str = if fm.capabilities.is_empty() {
    "none".to_string()
} else {
    fm.capabilities.join(", ")
};
prompt.push_str(&format!(
    "- id=\"{}\" name=\"{}\" capabilities=[{}]: {}\n",
    fm.id, fm.name, capabilities_str, fm.description
));
```

**`runner/lead_agent/tests.rs`:**
Update any test agents or assertions referencing `"lead_orchestration"` to `"orchestration"`.

- [ ] **Step 4: Search and fix remaining `skills` references in orchestrator**

```
grep -rn "\.skills" crates/openalpaca_core/src/orchestrator/
grep -rn "\.skills" crates/openalpaca_core/src/runner/lead_agent/
grep -rn "skills" crates/openalpaca_core/src/orchestrator/dispatcher/
```

Update:
- `dispatcher/core.rs` — update any `matched_skills` references (these are SkillMatch output, may keep name since it refers to capability matching result)
- `dispatcher/pipeline_step.rs` — tracing logs
- `handlers.rs` — check for references
- `mod.rs` — check Orchestrator fields

- [ ] **Step 5: Full build and test**

Run: `cargo check --all-targets && cargo test -p openalpaca_core`
Expected: Clean compilation, all tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/openalpaca_core/src/orchestrator/ crates/openalpaca_core/src/runner/lead_agent/ crates/openalpaca_core/src/agent/template/tests.rs crates/openalpaca_core/src/test_util.rs
git commit -m "refactor: rename lead_orchestration→orchestration, update SkillMatcher and lead agent"
```

---

## Chunk 4: Skill System Updates & Composition

### Task 9: Add `requires_capabilities` and `depends_on` to SkillFrontmatter

**Files:**
- Modify: `crates/openalpaca_core/src/middleware/skill/types.rs`

- [ ] **Step 1: Add new fields to SkillFrontmatter**

In `types.rs`, add to `SkillFrontmatter` struct (after line ~332):

```rust
#[serde(default)]
pub requires_capabilities: Vec<String>,

#[serde(default)]
pub depends_on: Vec<String>,
```

Also add `max_depth` to `InvokeConfig` struct (currently lines 27-50):

```rust
/// Maximum skill nesting depth (default 2: root + 1 child).
#[serde(default = "default_invoke_max_depth")]
pub max_depth: usize,
```

Add the default function:
```rust
fn default_invoke_max_depth() -> usize { 2 }
```

**Also update the manual `Default` impl** for `InvokeConfig` (lines 41-51 of types.rs). Add `max_depth: default_invoke_max_depth()` to the `Self { ... }` block, otherwise `Default::default()` produces `max_depth: 0` which breaks the root skill.

- [ ] **Step 2: Update `apply_legacy_compat()`**

In `apply_legacy_compat()` (lines 360-383), add at the end:

```rust
// Bridge: tools_required → tools.allow (existing, line 377)
// No automatic conversion from tools.allow to requires_capabilities.
// The resolution algorithm handles the legacy fallback.
```

No code change needed — the existing `tools_required → tools.allow` bridge stays. The resolution algorithm (Task 10) checks for empty `requires_capabilities` and falls back to `tools.allow`.

- [ ] **Step 3: Build and verify**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: Clean compilation (new fields default to empty vec via serde)

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/middleware/skill/types.rs
git commit -m "feat(skill): add requires_capabilities and depends_on to SkillFrontmatter"
```

---

### Task 10: Update skill invocation to use `requires_capabilities`

**Files:**
- Modify: `crates/openalpaca_core/src/orchestrator/skill/invocation.rs`

- [ ] **Step 1: Rewrite tool resolution section**

In `invocation.rs`, replace the tool resolution section (lines ~93-156). The current code starts with:
```rust
let mut tool_names: Vec<String> = skill_doc.frontmatter.tools.allow.clone();
```

Replace with capability-based resolution:

```rust
// Resolve tools via capability model
let mut tool_defs: Vec<openalpaca_llm::ToolDefinition> = if !skill_doc.frontmatter.requires_capabilities.is_empty() {
    // New path: capability-based resolution
    self.tool_registry.tools_for_capabilities(&skill_doc.frontmatter.requires_capabilities)
} else if !skill_doc.frontmatter.tools.allow.is_empty() {
    // Legacy fallback: direct tool name matching
    skill_doc.frontmatter.tools.allow.iter()
        .filter_map(|name| self.tool_registry.get(name).map(|t| t.definition.clone()))
        .collect()
} else {
    vec![]
};

// Apply deny list (both paths)
let skill_deny = &skill_doc.frontmatter.tools.deny;
let global_deny = &self.daemon_config.load().execution.skill_defaults.global_tool_deny;
tool_defs.retain(|t| !skill_deny.contains(&t.name) && !global_deny.contains(&t.name));

// Add script tools
// ... (keep existing script tool resolution code)

// Add invoke_skill:* synthetic tools (from depends_on)
for dep_id in &skill_doc.frontmatter.depends_on {
    if let Some(dep_entry) = self.skill_catalog.get(dep_id) {
        tool_defs.push(openalpaca_llm::ToolDefinition {
            name: format!("invoke_skill:{}", dep_id),
            description: format!("Invoke the '{}' skill: {}", dep_entry.frontmatter.name, dep_entry.frontmatter.description),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The input/query to pass to the skill"
                    }
                },
                "required": ["query"]
            }),
            strict: None,
            input_examples: None,
        });
    } else {
        tracing::warn!("Skill '{}' depends on '{}' which is not in catalog", skill_doc.frontmatter.name, dep_id);
    }
}
```

- [ ] **Step 2: Build and verify**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: Clean compilation

- [ ] **Step 3: Commit**

```bash
git add crates/openalpaca_core/src/orchestrator/skill/invocation.rs
git commit -m "feat(skill): use requires_capabilities for tool resolution with legacy fallback"
```

---

### Task 11: Add `validate_dependencies()` to SkillCatalog

**Files:**
- Modify: `crates/openalpaca_core/src/orchestrator/skill/catalog/mod.rs`

- [ ] **Step 1: Implement cycle detection**

Add to `SkillCatalog` impl:

Uses three-color DFS (White/Gray/Black) for correct cycle detection:

```rust
/// Validate that all `depends_on` references exist and contain no cycles.
/// Call after `scan_directory()` or `scan_multi_scope()`.
pub fn validate_dependencies(&self) -> Vec<String> {
    use std::collections::{HashMap, HashSet};

    let entries = self.entries.read().unwrap_or_else(|p| p.into_inner());
    let mut errors = Vec::new();

    // Phase 1: Check existence of all dependency references
    for (id, entry) in entries.iter() {
        for dep_id in &entry.frontmatter.depends_on {
            if !entries.contains_key(dep_id) {
                errors.push(format!(
                    "Skill '{}' depends on '{}' which does not exist",
                    id, dep_id
                ));
            }
        }
    }

    // Phase 2: Three-color DFS cycle detection
    // White = not visited, Gray = in current path, Black = fully explored
    #[derive(Clone, Copy, PartialEq)]
    enum Color { White, Gray, Black }

    let mut color: HashMap<&str, Color> = entries.keys().map(|k| (k.as_str(), Color::White)).collect();
    let mut reported_cycles: HashSet<String> = HashSet::new();

    fn dfs<'a>(
        node: &'a str,
        entries: &'a std::collections::HashMap<String, crate::orchestrator::skill::catalog::SkillEntry>,
        color: &mut HashMap<&'a str, Color>,
        errors: &mut Vec<String>,
        reported: &mut HashSet<String>,
    ) {
        color.insert(node, Color::Gray);
        if let Some(entry) = entries.get(node) {
            for dep in &entry.frontmatter.depends_on {
                if !entries.contains_key(dep.as_str()) {
                    continue; // Already reported in Phase 1
                }
                match color.get(dep.as_str()) {
                    Some(Color::Gray) => {
                        // Back edge — cycle found
                        let msg = format!("Cycle detected: '{}' -> '{}'", node, dep);
                        if reported.insert(msg.clone()) {
                            errors.push(msg);
                        }
                    }
                    Some(Color::White) | None => {
                        dfs(dep, entries, color, errors, reported);
                    }
                    Some(Color::Black) => {} // Already fully explored
                }
            }
        }
        color.insert(node, Color::Black);
    }

    for id in entries.keys() {
        if color.get(id.as_str()) == Some(&Color::White) {
            dfs(id, &entries, &mut color, &mut errors, &mut reported_cycles);
        }
    }

    if !errors.is_empty() {
        for err in &errors {
            tracing::warn!("{}", err);
        }
        let mut validation_errors = self.validation_errors.write().unwrap_or_else(|p| p.into_inner());
        validation_errors.extend(errors.clone());
    }

    errors
}
```

**Note:** The `dfs` function uses the `SkillEntry` type from the catalog module. The implementer should adjust the type path based on actual imports. If lifetime issues arise with the recursive `dfs`, convert to an iterative approach using an explicit stack with `(node, iterator_position)` pairs.

- [ ] **Step 2: Call after scan**

In `scan_multi_scope()` (line ~151), add at the end before returning:

```rust
self.validate_dependencies();
```

- [ ] **Step 3: Build and verify**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: Clean compilation

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/orchestrator/skill/catalog/mod.rs
git commit -m "feat(skill): add dependency cycle detection to SkillCatalog"
```

---

### Task 12: Implement SkillInvocationToolExecutor

**Files:**
- Create: `crates/openalpaca_core/src/orchestrator/skill/invoke_executor.rs`
- Modify: `crates/openalpaca_core/src/orchestrator/skill/mod.rs` (add module declaration)

- [ ] **Step 1: Create the executor file**

Create `crates/openalpaca_core/src/orchestrator/skill/invoke_executor.rs`:

**Important signature notes:**
- `run_agentic_loop_routed` takes `router: &LlmRouter` (reference), `initial_messages: Vec<ChatMessage>` (owned Vec, NOT Arc), `tools: Vec<ToolDefinition>` (owned Vec)
- Returns `LoopResult` with field `final_content: String` (NOT `response`)
- Full signature: `(router, initial_messages, tools, config, sandbox, agent_id, sandbox_policy, task_id, context_budget, cancel_token) -> LoopResult`
- `cancel_token` is `Option<CancellationToken>` — propagate parent's token for proper cancellation

```rust
use crate::orchestrator::skill::catalog::SkillCatalog;
use crate::runner::{run_agentic_loop_routed, LoopConfig};  // re-exported from runner/mod.rs (agentic_loop is private)
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolRegistry};
use async_trait::async_trait;
use openalpaca_llm::LlmRouter;  // re-export, NOT openalpaca_llm::routing::router::LlmRouter
use openalpaca_llm::{ChatMessage, ToolDefinition};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Executor for `invoke_skill:*` synthetic tool calls.
/// Created per skill invocation, not shared. Call stack grows with nesting.
pub struct SkillInvocationToolExecutor {
    pub catalog: Arc<SkillCatalog>,
    pub tool_registry: Arc<ToolRegistry>,
    pub router: Arc<LlmRouter>,
    pub call_stack: Vec<String>,
    pub max_depth: usize,
    pub cancel_token: Option<CancellationToken>,
}

impl SkillInvocationToolExecutor {
    pub fn new(
        catalog: Arc<SkillCatalog>,
        tool_registry: Arc<ToolRegistry>,
        router: Arc<LlmRouter>,
        call_stack: Vec<String>,
        max_depth: usize,
        cancel_token: Option<CancellationToken>,
    ) -> Self {
        Self {
            catalog,
            tool_registry,
            router,
            call_stack,
            max_depth,
            cancel_token,
        }
    }

    /// Check if a tool name is an invoke_skill call.
    pub fn is_skill_invocation(tool_name: &str) -> bool {
        tool_name.starts_with("invoke_skill:")
    }

    /// Execute a skill invocation tool call.
    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        let skill_id = tool_name
            .strip_prefix("invoke_skill:")
            .ok_or_else(|| format!("Invalid invoke_skill tool name: {}", tool_name))?;

        // Depth check
        if self.call_stack.len() >= self.max_depth {
            return Err(format!(
                "Max skill nesting depth ({}) exceeded. Call stack: {:?}",
                self.max_depth, self.call_stack
            ));
        }

        // Cycle check
        if self.call_stack.contains(&skill_id.to_string()) {
            return Err(format!(
                "Circular skill invocation detected: '{}' already in call stack {:?}",
                skill_id, self.call_stack
            ));
        }

        // Extract query
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "invoke_skill requires a 'query' parameter".to_string())?;

        // Load skill
        let skill_doc = self
            .catalog
            .load_full(skill_id)
            .map_err(|e| format!("Failed to load skill '{}': {}", skill_id, e))?;

        // Resolve tools for nested skill
        let mut tool_defs: Vec<ToolDefinition> = if !skill_doc.frontmatter.requires_capabilities.is_empty() {
            let deny = &skill_doc.frontmatter.tools.deny;
            let mut defs = self
                .tool_registry
                .tools_for_capabilities(&skill_doc.frontmatter.requires_capabilities);
            defs.retain(|t| !deny.contains(&t.name));
            defs
        } else if !skill_doc.frontmatter.tools.allow.is_empty() {
            skill_doc
                .frontmatter
                .tools
                .allow
                .iter()
                .filter_map(|name| {
                    self.tool_registry.get(name).map(|t| t.definition.clone())
                })
                .collect()
        } else {
            vec![]
        };

        // Add invoke_skill:* synthetic tools for nested skill's own depends_on
        // (enables multi-level skill composition)
        for dep_id in &skill_doc.frontmatter.depends_on {
            if let Some(dep_entry) = self.catalog.get(dep_id) {
                tool_defs.push(ToolDefinition {
                    name: format!("invoke_skill:{}", dep_id),
                    description: format!(
                        "Invoke the '{}' skill: {}",
                        dep_entry.frontmatter.name, dep_entry.frontmatter.description
                    ),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "The input/query to pass to the skill"
                            }
                        },
                        "required": ["query"]
                    }),
                    strict: None,
                    input_examples: None,
                });
            }
        }

        // Build system prompt from skill instructions
        let system_prompt = format!(
            "You are executing the '{}' skill.\n\n{}",
            skill_doc.frontmatter.name,
            skill_doc.body
        );

        // Vec<ChatMessage> — NOT Arc
        let messages = vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user(query),
        ];

        let config = LoopConfig {
            max_rounds: 10,
            ..LoopConfig::default()
        };

        // Create child executor with extended call stack for multi-level nesting
        let mut child_stack = self.call_stack.clone();
        child_stack.push(skill_id.to_string());
        // Note: child_executor + nested SandboxManager construction shown in
        // "Wiring into the tool execution pipeline" section below.
        // The implementer should build a SandboxManager from the resolved tools
        // using the same pattern as invocation.rs lines 337-366.

        // Run nested agentic loop
        // Note: router is Arc<LlmRouter> — call .as_ref() to get &LlmRouter
        let result = run_agentic_loop_routed(
            self.router.as_ref(),   // &LlmRouter
            messages,               // Vec<ChatMessage> (owned)
            tool_defs,              // Vec<ToolDefinition> (owned)
            &config,
            // TODO: pass constructed SandboxManager here (see wiring section below)
            // Without a SandboxManager, nested skills cannot execute tools.
            None,
            &format!("skill:{}", skill_id),
            None,                   // sandbox_policy
            None,                   // task_id
            None,                   // context_budget
            self.cancel_token.clone(), // propagate cancellation token
        )
        .await;

        Ok(result.final_content) // NOT .response — the field is `final_content`
    }
}
```

**Wiring into the tool execution pipeline (CRITICAL — without this, invoke_skill:* tools fail at runtime):**

The `SkillInvocationToolExecutor`'s `execute()` method above is the standalone logic. But it must be wired into the existing `SandboxManager`/`ContextualToolExecutor` pipeline that the agentic loop uses for tool dispatch. The approach: register `invoke_skill:*` tools as `BuiltInTool` adapters in a per-invocation cloned `ToolRegistry`.

**Step A: BuiltInTool adapter**

```rust
/// Adapter to make SkillInvocationToolExecutor usable as a BuiltInTool.
struct SkillInvocationBuiltInAdapter {
    executor: Arc<SkillInvocationToolExecutor>,
    skill_id: String,
}

#[async_trait]
impl BuiltInTool for SkillInvocationBuiltInAdapter {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        self.executor
            .execute(&format!("invoke_skill:{}", self.skill_id), arguments)
            .await
    }
}
```

**Step B: Integration in `invocation.rs` (update Task 10 to include this)**

After generating `invoke_skill:*` tool definitions in the `tool_defs` vec, the code in `invocation.rs` must:

1. Clone the main `ToolRegistry` to create a per-invocation copy
2. Create a `SkillInvocationToolExecutor` with the current call stack
3. For each `depends_on` entry, register a `SkillInvocationBuiltInAdapter` in the cloned registry
4. Build the `ContextualToolExecutor` and `SandboxManager` using the cloned registry
5. Pass the `SandboxManager` to `run_agentic_loop_routed` (NOT `None`)

```rust
// In invocation.rs, after building tool_defs with invoke_skill:* entries:

// Clone registry and register invoke_skill adapters
let mut invocation_registry = (*self.tool_registry).clone();  // ToolRegistry must derive Clone
let call_stack = vec![skill_doc.frontmatter.id.clone()]; // root skill
let executor = Arc::new(SkillInvocationToolExecutor::new(
    self.skill_catalog.clone(),
    self.tool_registry.clone(),
    self.llm_router.clone().expect("LLM router required"),
    call_stack,
    skill_doc.frontmatter.invoke.max_depth.max(1),
    cancel_token.clone(),
));

for dep_id in &skill_doc.frontmatter.depends_on {
    let adapter = SkillInvocationBuiltInAdapter {
        executor: executor.clone(),
        skill_id: dep_id.clone(),
    };
    // Find the definition we already added to tool_defs
    if let Some(def) = tool_defs.iter().find(|d| d.name == format!("invoke_skill:{}", dep_id)) {
        invocation_registry.register(RegisteredTool {
            definition: def.clone(),
            backend: ToolBackend::BuiltIn(Arc::new(adapter)),
            provides_capabilities: vec![],  // synthetic tools have no capabilities
        });
    }
}

// Build SandboxManager with the augmented registry for the agentic loop
// (reuse existing SandboxManager construction pattern from invocation.rs lines 337-366)
```

**Step C: Nested skill execution must also get a SandboxManager**

In the `SkillInvocationToolExecutor::execute()` method, the nested agentic loop currently receives `sandbox: None`. This means nested skills CANNOT execute tools. Fix: create a per-nested-invocation `SandboxManager` with a `ContextualToolExecutor` wrapping the resolved tools, similar to Step B. The executor must also create a child `SkillInvocationToolExecutor` with extended call stack for multi-level composition:

```rust
// Inside SkillInvocationToolExecutor::execute(), before run_agentic_loop_routed:

// Create child executor with extended call stack
let mut child_stack = self.call_stack.clone();
child_stack.push(skill_id.to_string());
let child_executor = Arc::new(SkillInvocationToolExecutor::new(
    self.catalog.clone(),
    self.tool_registry.clone(),
    self.router.clone(),
    child_stack,
    self.max_depth,
    self.cancel_token.clone(),
));

// Clone registry and register child's invoke_skill adapters
let mut nested_registry = (*self.tool_registry).clone();
for dep_id in &skill_doc.frontmatter.depends_on {
    // ... same adapter pattern as Step B
}

// Build SandboxManager for nested skill (similar to invocation.rs construction)
// Pass to run_agentic_loop_routed instead of None
```

**Note:** `ToolRegistry` must implement `Clone`. Add `#[derive(Clone)]` or implement manually. The `reqwest::Client` inside is already `Clone` (cheap Arc-based). The `HashMap<String, RegisteredTool>` requires `RegisteredTool` to be `Clone` — add `Clone` to `RegisteredTool` (both `ToolDefinition` and `ToolBackend` need it; `Arc<dyn BuiltInTool>` is Clone).

**Note:** `InvokeConfig.max_depth` must be clamped to `max(1, max_depth)` to enforce the spec's minimum depth of 1. Add this clamp when reading the config value.

- [ ] **Step 2: Register module**

In `crates/openalpaca_core/src/orchestrator/skill/mod.rs`, add:
```rust
pub mod invoke_executor;
```

- [ ] **Step 3: Build and verify**

Run: `cargo check -p openalpaca_core --all-targets`
Expected: Clean compilation. Adjust imports/types as needed to match actual `run_agentic_loop_routed` signature.

Note: The exact signature of `run_agentic_loop_routed` may differ. Check `crates/openalpaca_core/src/runner/agentic_loop/mod.rs` for the current signature and adapt the call. The key parameters are: router, messages, tools, config. Others can be None/defaults.

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/orchestrator/skill/invoke_executor.rs crates/openalpaca_core/src/orchestrator/skill/mod.rs
git commit -m "feat(skill): implement SkillInvocationToolExecutor for nested skill calls"
```

---

### Task 13: Update skill configs and startup validation

**Files:**
- Modify: `config/skills/*/SKILL.md` (4 files)
- Modify: `apps/openalpacad/src/services/tools.rs` (if workspace registration needs updating)

- [ ] **Step 1: Update skill configs with `requires_capabilities`**

For each skill config:

**code-review/SKILL.md:**
```yaml
requires_capabilities: ["file_read"]
```
(Remove or keep `tools.allow: ["file_read"]` — if `requires_capabilities` is set, it takes precedence)

**create-skill/SKILL.md:**
```yaml
requires_capabilities: ["file_read"]
```

**explain-code/SKILL.md:**
```yaml
requires_capabilities: ["file_read"]
```

**commit-message/SKILL.md:**
```yaml
requires_capabilities: ["shell_execute"]
```

- [ ] **Step 2: Verify daemon starts without errors**

Run: `cargo build -p openalpacad && cargo run -p openalpacad -- --help`
Expected: No startup panics or config parse errors

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p openalpaca_core`
Expected: All tests pass

- [ ] **Step 4: Run workspace-level check**

Run: `cargo check --all-targets`
Expected: Clean compilation across all crates

- [ ] **Step 5: Commit**

```bash
git add config/skills/ apps/openalpacad/
git commit -m "feat(config): update skill configs with requires_capabilities"
```

---

### Task 14: Final verification and cleanup

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p openalpaca_core`
Expected: All tests pass (report count)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p openalpaca_core --all-targets`
Expected: No new warnings (pre-existing warnings acceptable)

- [ ] **Step 3: Verify no remaining `definitions_for_skills` references**

Run: `grep -r "definitions_for_skills" crates/`
Expected: Zero matches

- [ ] **Step 4: Verify no remaining `Skill {` struct literals (should be `Capability {`)**

Run: `grep -rn "Skill {" crates/openalpaca_core/src/ --include="*.rs" | grep -v "//"`
Expected: Zero matches (excluding comments)

- [ ] **Step 5: Verify daemon builds clean**

Run: `cargo build -p openalpacad`
Expected: Clean build
