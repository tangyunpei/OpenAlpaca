# Storage design — Stage 0 implementation plan: the four fixes (F1–F4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the four fixes of the storage design's Stage 0 — the shipped `system_agent` template prompts again (F1), the lead reads the head and the tail of each subagent's answer and a failed answer is kept whole (F2), the master key leaves the daemon's environment (F3), and a follow-up or a steering message keeps the turn's resolved project (F4) — each pinned by tests, then one gate pass, one live check and the records.

**Architecture:** Four independent fixes in `crates/openalpaca_core`, `crates/openalpaca_llm`, `apps/openalpacad` and one shipped template; no migration, no new route, CLI verb, config key or event variant; nothing under `apps/openalpaca-gui` changes. Each fix is one or more tasks with its own test cycle and commit; the stage ends with one workspace test run, the clippy baseline, the two checks, a sandboxed live check on the rebuilt daemon, and the record updates.

**Tech Stack:** Rust 1.93.0 (edition 2024), tokio, rusqlite; `cargo test` per crate; a local Ollama (`qwen3.8:27b-mlx`) for the live check only.

**Spec:** `tasks/storage-design/design.md` rev 0.6 (approved 2026-10-05) — the Stage 0 table, §12 R30, §16 *Order* and §17 *Records*. Owner decisions ST-1…ST-22 are in its §0.1; T34 is taken by ST-14 ("the `system_agent` approval fix is therefore mandatory").

**Where this plan came from:** four drafts written against the tree at `d569885b`, each verified in place by a second reader (`tasks/storage-design/plan-drafts/`, untracked; the code facts they rest on are `plan-drafts/facts-stage0.md`, from the design's fact check). Line numbers are of that tree and may drift by a few lines; a drift is not an error, a different function is.

## Global Constraints

- **Model policy (owner, 2026-10-06):** Opus 5.5 does every job — implementing, each task's review and the final review of the whole stage. Fable 5.1 is used only as the advisor.
- **One cargo process at a time**, always. Per task: `cargo test -p <crate> <filter>`. The workspace run happens **once**, at Task G: `cargo test --workspace --exclude openalpaca_gui`, then `pkill -f "sleep 3600"` (the MCP manager's hang tests leave orphaned children that hold a piped stdout open).
- **Clippy baseline is 80** warnings (`CARGO_TERM_COLOR=never cargo clippy --workspace --exclude openalpaca_gui --all-targets`, counted as lines starting `warning` minus the `generated N warnings` lines). A task may not add one.
- `python3 scripts/gen_api_docs.py --check` must stay clean (no route or migration changes here), and `cargo check -p openalpaca_gui` must compile (CI compiles nothing under `src-tauri`).
- **The four bun gates are not run**, because no file under `apps/openalpaca-gui/` changes; Task G says to run all four if `git diff --stat` shows one did.
- **Never run the offline memory evaluation harness** (`cargo test -p openalpaca_core --features live-eval --test memory_eval` without the live flag) as a gate. Not run here at all.
- **Tests never reach a real Ollama** (`localhost:11434`): a test that enables a provider uses a mock or `127.0.0.1:1`. The live check (Task L) is the one place the local model is used, under the owner's standing authorization, and the daemon runs there under `scripts/eval/memory/loopback-only.sb` from its first launch.
- **`~/.openalpaca` is the owner's real store.** Never read it, write it, list it, or point any command at it. Every test uses a `tempfile` directory; the live check uses `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` on a scratch directory, with the current directory outside the repository.
- **No `&s[..n]` cut on any string** — `str::floor_char_boundary`/`ceil_char_boundary`, `crate::utils::text::byte_prefix` and the `head_tail` this plan moves there. **`std::env::set_var`/`remove_var` only inside `unsafe { }` in a single-threaded preamble**, with a `// SAFETY:` comment (edition 2024).
- **Test names are sentences in snake_case.** Edits to existing files are made with anchored scripts that assert each anchor matches exactly once — never BSD `sed -i`.
- **Commits:** conventional prefix (`fix:`, `feat:`, `docs:`, `test:`), a `Verified:` line naming the command and its result, and the `Co-Authored-By` line the implementing session is given, as its last line. Stage explicit paths only: the tree carries an untracked `tasks/deps-upgrade/` that belongs to another session.
- **Scope:** a task does what its spec row asks and nothing more. Removing dead code the fix orphans (F4's `ToolContext.workspace_path`), restoring the daemon's `OPENALPACA_MASTER_KEY` export for any reader, or changing `effective_confirmation_set`'s "explicit list wins" rule is out of scope; each is recorded below under *Rulings on the drafts' open points*.

## Review Focus

Input classes the spec implies and no task's tests exercised in the drafts; each now has its test in the owning task.

1. **A follow-up or a steer from a path that no longer exists** (a CLI started in a since-deleted directory): the row must hold NULL, never the string as sent, and nothing panics — Task F4.1, `a_queue_followup_from_a_path_that_no_longer_exists_stores_no_project`.
2. **More subagents than the inline bound can hold** (60 entries; past 25 the per-answer budget floors at 512 bytes): the summary still names every entry and every answer's opening and end, and may exceed the bound, which the loop's own cut then handles — Task F2.2, `a_batch_past_the_bound_still_lists_every_entry`.
3. **A malformed master key handed to the installer** (not 64 hex characters): refused before anything is installed, and the environment fallback still answers — Task F3.1, the two assertions before the install in the integration test.
4. **A failed subagent's error text in a non-ASCII script, longer than 512 bytes**: cut on character boundaries, never inside a character — Task F2.1, `head_tail_keeps_whole_characters_at_both_cuts` (the error cut and the answer cut are the same function).
5. **A user's own template with an explicit `require_confirmation_for` naming real tools**: keeps it — the "explicit list wins" rule is untouched and pinned by the existing `security::sandbox::tests::explicit_policy_overrides_annotations`, which Task F1.1 Step 4 runs.

## Rulings on the drafts' open points (the lead's; recorded, not reopened by an implementer)

- **F2 — `check_subagent_status`'s failed path stays `Err`.** A long failed answer reaches the model as head and tail at the inline bound with no `result_ref`; returning `Ok` would change the loop's error accounting (`agentic_loop/mod.rs:1181`, `:1186`, the `tool_executed` success flag). Minimal impact wins; the description says what each path returns.
- **F2 — the budget's shape** (header reserve 1,024 B, per-entry overhead 768 B, cap 8 KiB, floor 512 B, 1:1 head:tail; queued and running entries counted) is as the draft decided. Below about 8 KiB of `tool_result_inline_bytes` a batch of eight no longer fits and the loop's cut takes over; no separate handling.
- **F3 — the test installs a key in its own process.** The spec's "the test fills no process-wide value" is about the daemon's shared test binary (finding 5 of the fact check); an integration test file holding one `#[test]` is its own process and is the one shape that drives the function the daemon calls. `scripts/eval/steer_routing/README.md:50` stays as it is: true for the harness before and after.
- **F4 — `ToolContext.workspace_path` stays**, dead, with corrected comments; its removal is the collapse `tools/registry/mod.rs:27-28` already records as a follow-up. A steer from a project-less chat turn into a project's run stores NULL with no fallback to the run's project (the HTTP route has one): nothing reads an `unprocessed_steering` row's path today, and the fallback is a design question outside F4.
- **F1 — the switch-over note is part of F1** (design §12 R30 names it), so Task F1.2 writes it; the record updates (T34 to taken in `tasks/api-fix-plan.md`, `CLAUDE.md`'s Round G paragraph) are Task R, after the gate, so no task destroys the anchor another relies on.

**Order of execution:** F1 → F2 → F3 → F4 (independent; this order puts the smallest first), then Task G, Task L, Task R. Record BASE (`git rev-parse HEAD`) before the first task; the stage's range is BASE..HEAD at Task R.

---

## Fix F1 — the shipped `system_agent` template prompts again (T34)

**Stage 0, fix F1** (`spec-stage0.md` row F1; design `tasks/storage-design/design.md` §0.3 R30 and §12 R30; facts A1 and D7 in `facts-stage0.md`). Draft written 2026-10-05 against the tree at `d569885b`; line numbers are of that tree and may drift by a few lines.

### What is wrong, in the code's own terms

- `config/agents/system_agent.md:22-23` ends its frontmatter with
  ```yaml
  require_confirmation_for:
    - "file_delete"
  ```
- The sandbox decides which tools pause for approval with `effective_confirmation_set` (`crates/openalpaca_core/src/security/sandbox/mod.rs:754-771`): a **non-empty** `policy.require_confirmation_for` is returned verbatim (`:758-759`); only an empty one is derived from the `destructive_hint` annotations of every registered tool (`:761-771`). The check is a tool-**name** match (`:280`).
- No tool is named `file_delete`. `annotations_for_builtin` (`crates/openalpaca_core/src/tools/builtins/mod.rs:592-609`) knows `file_write`, `workspace_write`, `artifact_write`, `update_persona`, `memory_forget`, `memory_update`, `memory_confirm` (`destructive_local_annotations()`) and `shell_execute`, `send` (`destructive_open_world_annotations()`); `file_delete` is `_ => None`, and `grep -rn file_delete` over `crates/ apps/ config/` finds the string only in the template and in the inline fixtures of `crates/openalpaca_core/src/agent/config/tests.rs`.
- So for `system_agent` the list `["file_delete"]` matches nothing: its `shell_execute`, `file_write`, `artifact_write` and `workspace_write` run with no approval card. Round G's live check saw the lead delegate to it a shell write the person had just denied another agent (CLAUDE.md, Round G; `tasks/api-fix-plan.md:106`, row T34).
- The path from the file to the policy, which the test below walks exactly: `parse_agent_markdown` (`crates/openalpaca_core/src/agent/template/mod.rs:384`) → the frontmatter's `require_confirmation_for` (`:80`; parser default `Vec::new()`, `:234`; filled at `:338-339`) → `AgentTemplate::to_subagent` copies it into `AgentConstraints.require_confirmation_for` (`:588`) → `SandboxPolicy::from_constraints` copies it onto the policy (`sandbox/mod.rs:74`) → `effective_confirmation_set(policy, registry)`. In production the subagent spawn does the same: `agent_registry.spawn_instance` (`runner/lead_agent/tools.rs:246-249`) calls `to_subagent` (`crates/openalpaca_core/src/agent/registry/mod.rs:206`, `:228`), and the policy is `SandboxPolicy::from_constraints(&instance_id, &agent.constraints)` (`runner/lead_agent/tools.rs:571`) over the shared registry (`:352-353`). `admit_tool_surface` (`:578`) touches only the allow list, never the confirmation set.
- Deleting the key gives the parser's default, an empty list, and the derived set applies: over the builtins — `builtin_tools`, the registry Task F1.1's test builds — that is `file_write`, `shell_execute`, `artifact_write` and `workspace_write` (design §12 ST-15 later removes the last two from the derived set — a *later* stage, not this fix); the daemon's shared registry also carries `update_persona` and `send` (`builtin_tools_with_persona_context`, `tools/builtins/mod.rs:400-414`) and any extension tool whose annotations set the hint, so in production the derived set is wider by those names.

### Why an installed store needs a manual note (deliverable 4)

The daemon embeds the nine templates with `include_str!` (`apps/openalpacad/src/bootstrap/content.rs:42`, `seed!("agents/system_agent.md")` at `:57`) and seeds `config/agents/` **only when the directory is absent** (`:104-107`: `let root = config_dir.join(dir.name); if root.exists() { continue; }`). A store that already has `config/agents/` keeps the `system_agent.md` an earlier build wrote, and `openalpaca config reset --factory` does not touch `config/agents/` (`docs/CLI_Manual.md:181`; the switch-over note's step 3 says the same). A running daemon does re-read an edited template: `main.rs:310-312` adds `config/agents` to the watch paths and `:556-566` spawns `hot_reload::spawn_file_watcher` with that `agents_dir`; `handle_agents_change` (`apps/openalpacad/src/hot_reload.rs:588-656`) re-parses the `.md`, replaces the template in the registry and logs `Agent template hot-reloaded: <path>`. A daemon run from the repository checkout reads `./config/agents` directly and sees the edit at once.

### Deliverable 3 — what relies on the template being unprompted: nothing

Run and recorded on 2026-10-05 (the implementer repeats both in Task F1.1, step 0):

```bash
grep -rn "system_agent" --include='*.rs' --include='*.ts' --include='*.tsx' --include='*.toml' --include='*.json' --include='*.sh' --include='*.py' --include='*.ps1' --include='*.yml' crates apps scripts config .github | grep -v /target/
```

Two hits, neither a test of the template's behaviour:

- `crates/openalpaca_core/src/orchestrator/dispatcher/lead_agent.rs:903` — a comment about an instance-id format (`system_agent::1706337e`).
- `apps/openalpacad/src/bootstrap/content.rs:57` — the seed list. Its only test, `no_shipped_agent_template_claims_the_main_loop_s_id` (`content.rs:171-192`), reads each template's `id:` line and nothing else.

```bash
grep -rn "file_delete" --include='*.rs' --include='*.md' --include='*.ts' --include='*.tsx' --include='*.toml' crates apps config docs scripts | grep -v /target/
```

Hits: `config/agents/system_agent.md:23` (the line this fix deletes) and `crates/openalpaca_core/src/agent/config/tests.rs:24`, `:58`, `:175` — `sample_toml()` and `sample_template()` are **inline string fixtures** of the TOML/YAML parsers (`test_require_confirmation_for_roundtrip` asserts its own sample round-trips `["file_delete"]`); they never read the shipped file and do not change. No GUI file, fixture under `tests/fixtures/`, script or CI job names the template. The two tests that *do* read the shipped `config/agents/` directory — `every_shipped_template_grants_read_result` (`crates/openalpaca_core/src/agent/template/tests.rs:302`) and `all_nine_templates_name_memory_read` (`:444`) — assert `read_result` and `memory_read` only and stay green. `docs/Skill_Template_Reference.md:209` already states the rule ("A non-empty list replaces that default") and names no agent; no manual says `system_agent` prompts for `file_delete`, so no other doc flips.

### Repository rules that bite here

- Test names are sentences in snake_case (`a_failed_subagents_whole_answer_is_kept`).
- Edit files with an anchored Python script that asserts each anchor matches exactly once — never BSD `sed -i` (no `\b`, needs `''`; CLAUDE.md Gotchas).
- No string is cut with `&s[..n]` anywhere in this fix (nothing is cut).
- Tests are run as `cargo test -p <crate> <filter>`, one cargo process at a time; the workspace run is the stage gate (Task G), not this task's.
- Commit messages: conventional prefix, a `Verified:` line, then the `Co-Authored-By` line the implementing session is given.
- Never read, list or touch `~/.openalpaca`; never start the daemon or the CLI in this task.

---

### Task F1.1: The shipped `system_agent` template loses its confirmation list, pinned by a test on its effective set

**Files:** Modify: `config/agents/system_agent.md:22-23` (delete the two lines); Test: `crates/openalpaca_core/src/agent/template/tests.rs` (one new `#[test]`, inserted before the comment line `// ── Design 09 §5 (LQ3): memory search for every built-in agent ──────────`, which is at `:340` and occurs once).

**Interfaces:** Consumes — `pub fn parse_agent_markdown(input: &str) -> Result<AgentTemplate, AgentParseError>` (`agent/template/mod.rs:384`, in scope through the test module's `use super::*;`); `pub fn to_subagent(&self, instance_id: &str, task_id: &str) -> super::subagent::SubAgent` (`agent/template/mod.rs:485`); `pub fn from_constraints(agent_id: &str, constraints: &AgentConstraints) -> Self` on `SandboxPolicy` (`security/sandbox/mod.rs:69`); `pub(crate) fn effective_confirmation_set(policy: &SandboxPolicy, registry: &crate::tools::ToolRegistry) -> Vec<String>` (`security/sandbox/mod.rs:754`; crate-private, so the test must live inside `openalpaca_core` — `main_loop.rs:930` imports it the same way); `pub fn builtin_tools(db: Option<openalpaca_storage::Database>, daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>, web_search_config: Option<Arc<ArcSwap<WebSearchConfig>>>, workspace_root: Option<PathBuf>) -> Vec<RegisteredTool>` (`tools/builtins/mod.rs:331`; `builtin_tools(None, None, None, None)` is what `tools/builtins/tests.rs:5` and `:23` call; the two tools the test pins are registered with their annotations — `annotations: annotations_for_builtin("shell_execute")` at `tools/builtins/shell_execute.rs:101`, `annotations: annotations_for_builtin("file_write")` at `tools/builtins/file_ops.rs:536`, and `file_read`'s read-only set at `file_ops.rs:106` — so the derived set over this registry is `file_write`, `shell_execute`, `artifact_write`, `workspace_write`); `impl Default for ToolRegistry` and `pub fn register(&self, tool: RegisteredTool) -> Result<(), String>` (`tools/registry/mod.rs:438`, `:503`; the register-every-builtin loop is `tools/registry/tests.rs:707-710`). Produces — nothing other tasks consume; the template file as the daemon embeds it (`bootstrap/content.rs:57`) changes.

- [ ] **Step 0: Confirm the inventory (deliverable 3) is still what this draft recorded**

Run, from the repository root:

```bash
grep -rn "system_agent" --include='*.rs' --include='*.ts' --include='*.tsx' --include='*.toml' --include='*.json' --include='*.sh' --include='*.py' --include='*.ps1' --include='*.yml' crates apps scripts config .github | grep -v /target/
grep -rn "file_delete" --include='*.rs' --include='*.md' --include='*.ts' --include='*.tsx' --include='*.toml' crates apps config docs scripts | grep -v /target/
```

Expected: the first prints exactly two lines (`dispatcher/lead_agent.rs:903`, a comment; `bootstrap/content.rs:57`, the seed list); the second prints `config/agents/system_agent.md:23` and three lines in `agent/config/tests.rs` (`:24`, `:58`, `:175`). Any other hit is a new reader of the template: stop and report it before going on.

- [ ] **Step 1: Write the failing test**

Insert into `crates/openalpaca_core/src/agent/template/tests.rs`, immediately before the line `// ── Design 09 §5 (LQ3): memory search for every built-in agent ──────────` (today `:340`), with this script run from the repository root (it asserts the anchor is unique and the test is not already there):

```bash
python3 - <<'EOF'
import pathlib
p = pathlib.Path("crates/openalpaca_core/src/agent/template/tests.rs")
s = p.read_text()
anchor = "// ── Design 09 §5 (LQ3): memory search for every built-in agent"
assert s.count(anchor) == 1, s.count(anchor)
assert "the_shipped_system_agent_template_prompts_for_its_shell_and_file_writes" not in s
test = r'''/// Owner decision T34 (`tasks/api-fix-plan.md` §0; storage design §12 R30,
/// Stage 0 fix F1). The shipped `system_agent` template used to end its
/// frontmatter with `require_confirmation_for: ["file_delete"]`. An explicit,
/// non-empty list is returned verbatim by `effective_confirmation_set`
/// (`security/sandbox/mod.rs`) instead of the set derived from the tools'
/// `destructive_hint` annotations, and no tool is named `file_delete`, so
/// that agent's `shell_execute` and `file_write` never prompted — live, the
/// lead delegated to it a shell write the person had just denied another
/// agent. The template is read from the checkout and walked exactly as the
/// subagent spawn walks it: `to_subagent`, then `SandboxPolicy::from_constraints`,
/// then the effective set over a registry holding the builtins.
///
/// Only `shell_execute` and `file_write` are pinned, plus one read that must
/// not prompt: the design's ST-15 later takes `artifact_write` and
/// `workspace_write` out of the derived set, so the whole set is not asserted.
#[test]
fn the_shipped_system_agent_template_prompts_for_its_shell_and_file_writes() {
    use crate::security::sandbox::{SandboxPolicy, effective_confirmation_set};
    use crate::tools::ToolRegistry;
    use crate::tools::builtins::builtin_tools;

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/agents/system_agent.md");
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let doc = parse_agent_markdown(&body)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
    assert_eq!(doc.frontmatter.id, "system_agent");

    // The registry the spawn checks against: the builtins, with the
    // annotations `builtin_tools` registers them with.
    let registry = ToolRegistry::default();
    for tool in builtin_tools(None, None, None, None) {
        registry.register(tool).unwrap();
    }

    let agent = doc.to_subagent("system_agent::t34", "task-1");
    let policy = SandboxPolicy::from_constraints(&agent.id, &agent.constraints);
    let set = effective_confirmation_set(&policy, &registry);

    for name in ["shell_execute", "file_write"] {
        assert!(
            set.contains(&name.to_string()),
            "{name} must prompt for system_agent; the effective set is {set:?}"
        );
    }
    assert!(
        !set.contains(&"file_read".to_string()),
        "a read never prompts; the effective set is {set:?}"
    );
}

'''
p.write_text(s.replace(anchor, test + anchor, 1))
print("inserted")
EOF
```

Expected: `inserted`. The file now holds 24 `#[test]` functions (23 today).

- [ ] **Step 2: Run the test to see it fail**

Run:

```bash
cargo test -p openalpaca_core the_shipped_system_agent_template_prompts_for_its_shell_and_file_writes
```

Expected: FAIL. With the template unchanged, `to_subagent` copies `["file_delete"]` into the constraints, `from_constraints` copies it onto the policy, and `effective_confirmation_set` returns it verbatim (`sandbox/mod.rs:758-759`), so the first iteration of the loop panics:

```text
---- agent::template::tests::the_shipped_system_agent_template_prompts_for_its_shell_and_file_writes stdout ----
thread 'agent::template::tests::the_shipped_system_agent_template_prompts_for_its_shell_and_file_writes' panicked at crates/openalpaca_core/src/agent/template/tests.rs:<line>:
shell_execute must prompt for system_agent; the effective set is ["file_delete"]
...
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; <n> filtered out
```

If instead it fails at `reading .../config/agents/system_agent.md`, the test was not run from a checkout that has `config/agents/` two directories above `crates/openalpaca_core/` — the two existing shipped-template tests at `template/tests.rs:302` and `:444` have the same requirement, so check they fail the same way before suspecting the new test.

- [ ] **Step 3: Write the minimal implementation — delete the two lines**

Run, from the repository root (the block including its neighbours is asserted to occur exactly once):

```bash
python3 - <<'EOF'
import pathlib
p = pathlib.Path("config/agents/system_agent.md")
s = p.read_text()
block = 'timeout_seconds: 600\nrequire_confirmation_for:\n  - "file_delete"\n---\n'
assert s.count(block) == 1, s.count(block)
p.write_text(s.replace(block, 'timeout_seconds: 600\n---\n', 1))
print("deleted")
EOF
```

Expected: `deleted`. The frontmatter now ends:

```yaml
max_tool_calls: 30
timeout_seconds: 600
---
```

and `git diff --stat config/agents/system_agent.md` reads `1 file changed, 2 deletions(-)`. Nothing else in the file changes — not the persona text, whose sentence "confirm destructive operations before executing" was until now the only confirmation this agent had. No Rust code changes: the parser's default for a missing key is `Vec::new()` (`agent/template/mod.rs:234`), and the daemon's embedded copy follows the file at its next build (`bootstrap/content.rs:42`).

- [ ] **Step 4: Run the test to see it pass, with its neighbours**

Run:

```bash
cargo test -p openalpaca_core the_shipped_system_agent_template_prompts_for_its_shell_and_file_writes
```

Expected: `test result: ok. 1 passed; 0 failed; 0 ignored`. Then the module the test lives in, which includes the two other tests that re-parse the edited file:

```bash
cargo test -p openalpaca_core agent::template::tests
```

Expected: the **lib's** line reads `test result: ok. 24 passed; 0 failed` — today's 23 `#[test]` functions plus this one (the module has no `#[tokio::test]`). The command also runs the crate's four integration binaries (`crates/openalpaca_core/tests/`: `mcp_integration`, `memory_eval`, `memory_eval_corpus`, `steer_routing_eval`) and the doc-tests, each printing its own `test result: ok. 0 passed; … filtered out` line, so the last line of the output is not the one to read; `0 failed` on every line is the gate — among the 24 `every_shipped_template_grants_read_result` and `all_nine_templates_name_memory_read` (both still see nine templates, both unaffected by the deleted key). Also unchanged and still green, since no Rust file but the test module changed: `security::sandbox::tests::explicit_policy_overrides_annotations` (the "explicit list wins" rule stays, `sandbox/tests.rs:848`) and `agent::config::tests::test_require_confirmation_for_roundtrip` (its own inline `["file_delete"]` fixture, `agent/config/tests.rs:53`); run them if in doubt:

```bash
cargo test -p openalpaca_core explicit_policy_overrides_annotations
cargo test -p openalpaca_core test_require_confirmation_for_roundtrip
```

Expected: `1 passed` each. The daemon's `bootstrap::content::tests::no_shipped_agent_template_claims_the_main_loop_s_id` recompiles the embedded file through `include_str!` and reads only `id:`; it runs at the stage gate (`cargo test --workspace --exclude openalpaca_gui`, Task G), not here — one cargo process at a time.

- [ ] **Step 5: Commit**

```bash
git add config/agents/system_agent.md crates/openalpaca_core/src/agent/template/tests.rs
git commit -m "fix(agents): the shipped system_agent template prompts for its shell and file writes again (T34, F1)" -m "config/agents/system_agent.md set require_confirmation_for: [\"file_delete\"]. An
explicit list replaces the set derived from the tools' destructive_hint
annotations (effective_confirmation_set, security/sandbox/mod.rs), and no
tool is named file_delete, so that agent's shell_execute, file_write,
artifact_write and workspace_write ran with no approval card — live, the lead
delegated to it a shell write the person had just denied another agent
(Round G, T34). The two lines are gone; the rule that an explicit list wins
stays, and a template that wants fewer prompts can still say so. The daemon's
embedded copy (bootstrap/content.rs) follows the file.

The new test reads the shipped template from the checkout and walks the
spawn's own path — to_subagent, SandboxPolicy::from_constraints, the
effective set over a registry holding the builtins — and asserts
shell_execute and file_write are in it and file_read is not. A store seeded
by an earlier build keeps its old file; the switch-over note (next commit)
says which two lines to delete.

Verified: cargo test -p openalpaca_core the_shipped_system_agent_template_prompts_for_its_shell_and_file_writes
fails on the unchanged template (the effective set is [\"file_delete\"]) and
passes after the edit (1 passed); cargo test -p openalpaca_core
agent::template::tests 24 passed, 0 failed." -m "Co-Authored-By: <the line the implementing session is given>"
```

The last `-m` is the attribution line the implementing session was given, verbatim.

---

### Task F1.2: The switch-over note names the two lines a seeded store keeps

**Files:** Modify: `docs/Installation_Manual.md` — section `## Switching to the Markdown Memory Build` (`:148`), a new step 7 after step 6 (`:206-213`, "Review `USER.md` by hand"), inserted before the paragraph `The reset leaves `~/.openalpaca/README.md` as it is` (`:215`). No Rust test: this task changes one Markdown file. The step-1/step-2 slots below are the anchor check that stands in for a failing test.

**Interfaces:** Consumes — the facts the note states: `apps/openalpacad/src/bootstrap/content.rs:104-107` (seeded only when `config/agents/` is absent), `docs/CLI_Manual.md:181` (the reset does not touch `config/agents/`), `apps/openalpacad/src/main.rs:310-312`, `:556-566` and `apps/openalpacad/src/hot_reload.rs:588-656` (an edited template is re-read by a running daemon and logged `Agent template hot-reloaded`), `openalpaca daemon restart` (`docs/CLI_Manual.md:122`) and Settings → Connection's `Stop daemon…` / `Start daemon` (`docs/GUI_Manual.md:142-147`). Produces — nothing other tasks consume.

- [ ] **Step 1: The anchor check (stands in for the failing test)**

Save this as the check both before and after the edit; run it from the repository root:

```bash
python3 - <<'EOF'
import pathlib
m = pathlib.Path("docs/Installation_Manual.md").read_text()
anchor = "   delete the rest. Nothing moves them into memory notes for you.\n\nThe reset leaves `~/.openalpaca/README.md` as it is"
print("anchor =", m.count(anchor), "step7 =", m.count("7. **Check `agents/system_agent.md`.**"))
EOF
```

- [ ] **Step 2: Run it to see the note is missing**

Expected: `anchor = 1 step7 = 0`. Any other `anchor` count means step 6's tail or the README paragraph moved since this draft: re-read `docs/Installation_Manual.md:206-216` and fix the anchor string before going on.

- [ ] **Step 3: Insert step 7**

Run, from the repository root:

```bash
python3 - <<'EOF'
import pathlib
p = pathlib.Path("docs/Installation_Manual.md")
s = p.read_text()
tail = "   delete the rest. Nothing moves them into memory notes for you.\n"
after = "\nThe reset leaves `~/.openalpaca/README.md` as it is"
anchor = tail + after
assert s.count(anchor) == 1, s.count(anchor)
assert s.count("7. **Check `agents/system_agent.md`.**") == 0
step7 = """7. **Check `agents/system_agent.md`.** `~/.openalpaca/config/agents/` survives
   the reset too, and the daemon writes that directory only when it is absent,
   so a store an earlier build set up keeps the `system_agent.md` that build
   wrote. As earlier builds shipped it, that file ends its frontmatter with two
   lines that switched the agent's approval prompts off: `file_delete` is not
   the name of any tool, and a template's own `require_confirmation_for` list
   replaces the default — every tool marked destructive asks first — instead
   of adding to it, so with those two lines the agent's `shell_execute`,
   `file_write`, `artifact_write` and `workspace_write` ran with no approval
   card. Open the file and delete exactly these two lines:

   ```yaml
   require_confirmation_for:
     - "file_delete"
   ```

   A running daemon reads the change on its own (`state/logs/daemon.log` says
   `Agent template hot-reloaded`); `openalpaca daemon restart`, or `Stop
   daemon…` then `Start daemon` in the app's Settings → Connection, does the
   same. A file that does not have the two lines needs nothing. A template of
   your own that names real tools in `require_confirmation_for` is unaffected:
   an explicit list still wins.
"""
p.write_text(s.replace(anchor, tail + "\n" + step7 + after, 1))
print("inserted")
EOF
```

Expected: `inserted`.

- [ ] **Step 4: Run the check again, and read the result**

Run the step-1 script again. Expected: `anchor = 0 step7 = 1` (the anchor's two halves are now separated by the new step, which is the point). Then:

```bash
sed -n 206,240p docs/Installation_Manual.md
```

Expected: step 6 ends with "Nothing moves them into memory notes for you.", one blank line, step 7 as written above (its fenced block indented three spaces, inside the list item), one blank line, then "The reset leaves `~/.openalpaca/README.md` as it is, …" unchanged. `git diff --stat docs/Installation_Manual.md` reads `1 file changed, 23 insertions(+)`. The file is tracked (`git ls-files docs/Installation_Manual.md` prints it), so no `.gitignore` whitelist is needed.

- [ ] **Step 5: Commit**

```bash
git add docs/Installation_Manual.md
git commit -m "docs(install): the switch-over note names the two template lines a seeded store keeps (T34, F1)" -m "The daemon seeds config/agents/ only when the directory is absent
(bootstrap/content.rs) and the factory reset leaves it alone, so a store an
earlier build set up keeps the system_agent.md that shipped
require_confirmation_for: [\"file_delete\"] — the list that let that agent's
shell and file writes run unprompted (T34; fixed in the shipped file by the
previous commit). Step 7 of \"Switching to the Markdown Memory Build\" quotes
the two lines to delete, says why, and that a running daemon re-reads the
template on its own.

Verified: the step's claims re-read against bootstrap/content.rs:104-107,
main.rs:310-312 and :556-566, hot_reload.rs:588-656 and
docs/CLI_Manual.md:181; the insertion anchor matched exactly once; no code
changed." -m "Co-Authored-By: <the line the implementing session is given>"
```

---

### What this fix deliberately does not do

- It does not change `effective_confirmation_set` or the "explicit list wins" rule (design §12 R30; `sandbox/tests.rs:848` keeps pinning it).
- It does not touch `annotations_for_builtin`: ST-15 (`artifact_write` and `workspace_write` stop prompting) is a later stage with its own flipping test (`annotations_for_builtin_destructive_tools_tagged`, `tools/builtins/tests.rs:243-248`), which is why Task F1.1's test pins only `shell_execute` and `file_write`.
- It does not re-seed or rewrite an installed store's `config/agents/` — the owner's directory is the owner's (L9; `content.rs:14-17`); the manual tells the person what to delete.
- It does not move T34 from pending to taken in `tasks/api-fix-plan.md` §0 or edit CLAUDE.md's Round G paragraph: design §17 assigns those record updates to the stage's records work (see the open points).
- The live confirmation that a delegated `system_agent` shell command raises a card is Task L, step 5, on the rebuilt daemon — not this task.

---

---

## Fix F2 — the lead reads the head and the tail of each subagent answer; a failed answer is kept whole

Draft for Stage 0 of `tasks/storage-design/design.md` (spec row F2 in `plan-drafts/spec-stage0.md`; code facts A2, A3 and further findings 2 and 3 in `plan-drafts/facts-stage0.md`). Line numbers are as of `d569885b` and were re-read for this draft; they may drift by a few lines.

### What is wrong today, in one paragraph

`wait_for_subagents` returns `tracker.summary()` (`crates/openalpaca_core/src/runner/lead_agent/tools.rs:1187`, `:1204`, `:1237`), and the summary shows a completed subagent's first 500 `char`s with no notice that anything was dropped (`runner/lead_agent/tracker.rs:109` `content.chars().take(500)`). A failed subagent's answer is not kept at all: the spawn path stores `"Agent '{}' failed: {:?}. Last output: {}"` with the first 500 chars inside the error string (`tools.rs:926-934`), and `SubagentStatus::Failed { error: String }` (`tracker.rs:10`) has no field for the answer, so `check_subagent_status` (`tools.rs:1116-1118`) can only return that prefix. Its description (`tools.rs:1131-1134`) and the lead prompt's line (`runner/lead_agent/prompt.rs:157-158`) say the tool shows a status, not that it returns the answer. Live consequence (CLAUDE.md, Round G): a workflow's report named `notes/tea.md` instead of the artifact's absolute path because the path sat past the 500th character.

### The shape of the fix

Five tasks, in an order that leaves every commit showing **at least** what today shows (a commit that stored a failed answer before the summary showed it would hide the 500 chars the lead sees today):

1. **Task F2.1** — the head+tail helper moves from `runner/agentic_loop/tool_helpers.rs:19` (`pub(super)`, unreachable from `runner/lead_agent`: `tool_helpers` is a private module of the private `agentic_loop`) to `crate::utils::text::head_tail`. Moved, not widened; the old body is deleted and its two call sites and two tests updated.
2. **Task F2.2** — `summary()` shows each completed answer as head and tail within a per-answer byte budget derived from the loop's inline bound; the bound reaches the tracker as a constructor argument.
3. **Task F2.3** — `SubagentStatus::Failed { error, output }`: the spawn path keeps the whole final content beside a short error; the summary's failed arm shows the error (cut to 512 bytes if long) and the output's head and tail under the same budget.
4. **Task F2.4** — `check_subagent_status`'s failed path returns the error and the whole kept output.
5. **Task F2.5** — the two tool descriptions, the lead prompt's two lines and the compose golden's verbatim copy. (The CLAUDE.md Round G clause is the stage's Task R, which replaces it by its present wording — see *After the five tasks*.)

### The budget (decided here; the spec leaves it open)

All numbers are **bytes** (the loop's bound is bytes: `runner/agentic_loop/mod.rs:312` `result_text.len() <= config.tool_result_inline_bytes`). Constants live in `tracker.rs`:

| Constant | Value | Why |
|---|---|---|
| `SUMMARY_HEADER_RESERVE_BYTES` | 1,024 | `wait_for_subagents` prepends a header: the interrupted return at `tools.rs:1179-1188` is the longest (180 bytes with two-digit counts); 1 KiB leaves room for all three. |
| `SUMMARY_ENTRY_OVERHEAD_BYTES` | 768 | Per entry: the line prefix `- **<run_id>**: completed — ` (a run id is `<template>::<8 hex>`, built at `tools.rs:624-628` from the registry's instance id, `agent/registry/mod.rs:219-220`, so under 60 bytes), up to two head/tail notices (`[... N of M bytes elided; the tail follows ...]` plus four newlines, about 60 bytes each), ` Last output: ` (14), the join `\n`, and a failed entry's error cut to `SUMMARY_ERROR_MAX_BYTES` (512). 512 + 60 + 132 + 15 = 719 ≤ 768. |
| `SUMMARY_ANSWER_CAP_BYTES` | 8,192 | One answer never takes more than 8 KiB of the lead's context, however few subagents ran — the lead reads the whole answer with `check_subagent_status` when it needs it. |
| `SUMMARY_ANSWER_FLOOR_BYTES` | 512 | Below this an entry says nothing useful; with many entries the summary is allowed to exceed the bound and the loop's own spill/cut (`model_visible_result`, `agentic_loop/mod.rs:276-293`) takes over, as it does for any oversized tool result. |
| `SUMMARY_ERROR_MAX_BYTES` | 512 | `LoopFinishReason::Error(String)` (`agentic_loop/config.rs:424`; built at `agentic_loop/mod.rs:943` and `:1476` from a provider error's `to_string()`) and a plugin's error (`tools.rs:811-814`) are unbounded, so the error text is head+tail-cut too, or the per-entry overhead would be fiction. Introduced in **Task F2.3**, where its first use is — defined in Task F2.2 it would be an unused private `const` and a `dead_code` warning in that commit. |

**Formula** — `answer_budget(inline_bytes, entries)`:

```
per_entry = (inline_bytes − 1024) / entries          // saturating, entries ≥ 1
budget    = clamp(per_entry − 768, 512, 8192)         // saturating subtraction
```

`entries` is `map.len()` — every subagent the tracker holds, queued and running ones included (a queued line costs about 60 bytes, so counting it only under-uses the bound; the tracker is per lead run and grows across spawn waves, so `entries` can exceed 8). Each answer is cut by `head_tail(answer, budget)`: head = `budget / 2` bytes rounded **down** to a character boundary, tail = `budget / 2` bytes rounded **up** to a boundary (so never longer), one notice between them. **Head:tail = 1:1**, the same split the loop already uses for an `Err` result, because the live failure was a path at the very end of an answer and the opening of an answer is where the subagent states what it did.

**The three numbers for the default bound (32,768) and a batch of 8:** per-answer budget **3,200 bytes**, head **1,600 bytes**, tail **1,600 bytes**. Worst case `8 × (3,200 + 768) + 1,024 = 32,768 = the bound` by construction; the test in Task F2.2 measures 26,343 bytes for eight 20 KB answers. Other points on the default bound: 1 or 2 entries → 8,192 (capped); 4 → 7,168; 16 → 1,216; 24 → 554; 25 and above → 512 (floor), where the summary may exceed the bound from about 25 entries on. Today's 500 `char`s are at most 2,000 bytes, so for n ≤ 8 no script sees less than today. At the clamp **minimum** `tool_result_inline_bytes = 4096` (`daemon_config/validation.rs:193-198`) a batch of 8 cannot fit: the budget floors at 512 and the summary (about 11 KB worst case) is cut by the loop — recorded as an open point, not solved here.

### How the bound reaches the tracker

A private field `inline_bytes: usize` on `SubagentTracker`, set by a new constructor `SubagentTracker::with_inline_bytes(n)`; `SubagentTracker::new()` keeps its signature and uses the loop's default `MAX_TOOL_RESULT_SIZE` (32 KiB, `agentic_loop/tool_helpers.rs:4`), so the 25 `SubagentTracker::new()` calls in `lead_agent/tests.rs` are untouched, and the private field breaks no struct literal — `SubagentTracker { .. }` is written nowhere in the tree but its own definition (`grep -rn "SubagentTracker {" crates apps`). `new()` has one production caller, `runner/lead_agent/mod.rs:237`, which becomes the one `with_inline_bytes` call, from the same config read the lead's `LoopConfig` takes at `:578-582`. A parameter on `summary()` was rejected: it would touch all three call sites in `tools.rs` and every `WaitForSubagentsTool { .. }` literal in the tests.

### Repository rules that bite in these tasks (copied for the implementer)

- Never cut a string with `&s[..n]`. Use `str::floor_char_boundary` / `str::ceil_char_boundary` (both used already at `crates/openalpaca_core/src/memory/learning/window.rs:179-180`), or `crate::utils::text::byte_prefix` (`utils/text.rs:62`). Indexing with a boundary those return is what they are for: `head_tail`'s one slice, `&text[tail_start..]`, starts at a `ceil_char_boundary`, exactly as `byte_prefix` (`text.rs:63`) and `cut_middle` (`window.rs:184-185`) index — do not "fix" it. In a test, print a cut of a long string through `byte_prefix`, never through a slice.
- Test names are sentences in snake_case (`a_failed_subagents_whole_answer_is_kept`).
- Tests are run per task as `cargo test -p openalpaca_core <filter>`, one cargo process at a time. The filters below are module-path substrings: `lead_agent::tests::<name>`, `agentic_loop::tests::<name>`, `utils::text::tests::<name>`.
- Commit messages: conventional prefix (`feat:`, `fix:`, `test:`, `docs:`), a line naming what was verified, and the `Co-Authored-By:` line the implementing session was given (this draft writes `Co-Authored-By: <the line the implementing session is given>`, the line this session has; substitute yours if it differs).
- `summary()` iterates a `HashMap` (`tracker.rs:100`): no test may assume entry order. Every multi-entry assertion below is on `contains`, a count, or the total length; the exact-string assertions use one entry.
- `std::env::set_var` is not needed by any task here.

---

### Task F2.1: `head_tail` moves to `crate::utils::text`

**Files:** Create: nothing. Modify: `crates/openalpaca_core/src/utils/text.rs` (after `byte_prefix`, `:62-64`, and the test module's import at `:452-456`); `crates/openalpaca_core/src/runner/agentic_loop/tool_helpers.rs:12-41` (delete); `crates/openalpaca_core/src/runner/agentic_loop/mod.rs:17-19` and `:287`; `crates/openalpaca_core/src/runner/agentic_loop/tests.rs:3776` and `:3787`. Test: `crates/openalpaca_core/src/utils/text.rs` (module `tests`).

**Interfaces:** Consumes: `str::floor_char_boundary(&self, usize) -> usize`, `str::ceil_char_boundary(&self, usize) -> usize` (std, stable, used at `memory/learning/window.rs:179-180`); `pub(crate) fn byte_prefix(s: &str, max_bytes: usize) -> &str` (`utils/text.rs:62`). Produces: `pub(crate) fn head_tail(text: &str, limit: usize) -> String` in `crate::utils::text` (`utils` is `pub mod` at `lib.rs:22`, `text` is `pub mod` in `utils/mod.rs`), whose notice is byte-for-byte the old helper's (`[... {dropped} of {total} bytes elided; the tail follows ...]`), because `agentic_loop/tests.rs:3784` asserts `contains("the tail follows")`. Not reused: `memory::learning::window::cut_middle` (`window.rs:175`, `pub(crate)`) is the same cut with separate head and tail sizes and another notice (`[… N bytes omitted …]`), which that pinned test would reject.

- [ ] **Step 1: Write the failing test**

In `crates/openalpaca_core/src/utils/text.rs`, change the test module's import (`:452-456`) to include `head_tail`:

```rust
    use super::{
        Escaped, byte_prefix, cut_notice, escape_controls, escape_hidden, escape_hidden_line,
        escape_hidden_within, head_tail, hidden_notice, is_default_ignorable, is_hidden,
        is_private_use, is_unassigned, needs_escape, needs_escape_in_line, one_line,
    };
```

and add, at the end of the module (after `byte_prefix_rounds_down_to_a_character_boundary`, before the closing `}` at `:1137`):

```rust
    /// The cut is in bytes and lands on character boundaries at both ends:
    /// the head rounds down, the tail rounds up, so neither is ever longer
    /// than half the limit and no character is split. Every row but the
    /// ASCII one panicked under `&s[..n]`.
    #[test]
    fn head_tail_keeps_whole_characters_at_both_cuts() {
        // 2,000 three-byte characters = 6,000 bytes under a 3,200-byte limit:
        // half is 1,600, which is not a multiple of 3. The head rounds down
        // to 1,599 bytes (533 characters); the tail starts at 4,400, rounded
        // up to 4,401, so it is 1,599 bytes (533 characters) too.
        let cjk = "日".repeat(2000);
        assert_eq!(
            head_tail(&cjk, 3200),
            format!(
                "{}\n\n[... 2802 of 6000 bytes elided; the tail follows ...]\n\n{}",
                "日".repeat(533),
                "日".repeat(533)
            )
        );

        // Four-byte characters under a 10-byte limit: half is 5, inside the
        // second emoji at both ends — one whole emoji each side.
        let emoji = "🙂".repeat(100);
        assert_eq!(
            head_tail(&emoji, 10),
            "🙂\n\n[... 392 of 400 bytes elided; the tail follows ...]\n\n🙂"
        );

        // ASCII: an exact byte cut, the behaviour the loop's `Err` path had.
        let ascii = format!("HEAD{}TAIL", "m".repeat(1000));
        assert_eq!(
            head_tail(&ascii, 8),
            "HEAD\n\n[... 1000 of 1008 bytes elided; the tail follows ...]\n\nTAIL"
        );
    }

    /// Under the limit nothing is touched — the whole text comes back,
    /// whatever its script, and an empty text under a zero limit is empty.
    #[test]
    fn head_tail_leaves_a_text_within_the_limit_alone() {
        assert_eq!(head_tail("small", 1024), "small");
        assert_eq!(head_tail("日本語", 9), "日本語");
        assert_eq!(head_tail("", 0), "");
    }
```

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test -p openalpaca_core utils::text::tests::head_tail`
Expected: FAIL to compile — `error[E0432]: unresolved import \`super::head_tail\`` (the function does not exist yet; rustc reports the import once and says nothing more about the two uses).

- [ ] **Step 3: Write the minimal implementation**

In `crates/openalpaca_core/src/utils/text.rs`, directly after `byte_prefix` (`:62-64`):

```rust
/// A text too long for `limit` bytes, shown as its **head and its tail** with
/// one notice naming the bytes between them — the shape the agentic loop
/// gives an oversized `Err` result (§5.4: a compiler or test failure sits at
/// the tail) and, since Stage 0's F2, the shape `wait_for_subagents` gives
/// each subagent's answer.
///
/// Bytes, on character boundaries: the head is `limit / 2` bytes rounded
/// down to a boundary, the tail `limit / 2` bytes rounded up to one, so
/// neither exceeds its half and no character is split. A text within the
/// limit comes back whole. The notice (about 66 bytes) is added on top of
/// `limit`; a caller that must stay under a bound budgets for it.
pub(crate) fn head_tail(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let half = limit / 2;
    let head = byte_prefix(text, half);
    let tail_start = text.ceil_char_boundary(text.len() - half);
    format!(
        "{}\n\n[... {} of {} bytes elided; the tail follows ...]\n\n{}",
        head,
        tail_start - head.len(),
        text.len(),
        &text[tail_start..]
    )
}
```

(`text.len() - half` cannot underflow: `text.len() > limit >= half`. `tail_start >= text.len() - half > half >= head.len()`, so the two halves never overlap and the subtraction is sound.)

In `crates/openalpaca_core/src/runner/agentic_loop/tool_helpers.rs`, **delete** lines 12-41 (the doc comment, the whole `pub(super) fn head_tail_tool_result`, and the blank line after it — line 11 is blank too, so deleting 12-40 would leave two). The file then goes from `truncate_tool_result` (`:6-10`), one blank line, straight to `truncate_tool_result_to`'s doc comment.

In `crates/openalpaca_core/src/runner/agentic_loop/mod.rs`, replace lines 17-19

```rust
use tool_helpers::{
    format_tool_error, format_tool_error_with_hint, head_tail_tool_result, truncate_tool_result_to,
};
```

with

```rust
use crate::utils::text::head_tail;
use tool_helpers::{format_tool_error, format_tool_error_with_hint, truncate_tool_result_to};
```

and replace line 287

```rust
        return head_tail_tool_result(result_text.to_string(), config.tool_result_inline_bytes);
```

with

```rust
        return head_tail(result_text, config.tool_result_inline_bytes);
```

(`result_text` is already `&str` there, `mod.rs:278`.)

In `crates/openalpaca_core/src/runner/agentic_loop/tests.rs`, replace line 3776

```rust
    let cut = head_tail_tool_result(text.clone(), limit);
```

with

```rust
    let cut = head_tail(&text, limit);
```

and line 3787

```rust
    assert_eq!(head_tail_tool_result("small".to_string(), limit), "small");
```

with

```rust
    assert_eq!(head_tail("small", limit), "small");
```

(The tests module opens with `use super::*;` (`tests.rs:1`), which brings the parent's private `use crate::utils::text::head_tail;` into scope exactly as it brought `head_tail_tool_result` before.)

- [ ] **Step 4: Run the test to see it pass**

Run: `cargo test -p openalpaca_core utils::text::tests::head_tail`
Expected: PASS, 2 tests (`head_tail_keeps_whole_characters_at_both_cuts`, `head_tail_leaves_a_text_within_the_limit_alone`).

Run: `cargo test -p openalpaca_core agentic_loop::tests::an_error_result_keeps_its_head_and_its_tail`
Expected: PASS — the loop's own head+tail test, now through the moved function; its `contains("the tail follows")` holds because the notice wording is unchanged.

Run: `cargo test -p openalpaca_core agentic_loop::tests::a_loop_without_a_session_log_is_unchanged`
Expected: PASS (a neighbour that exercises `model_visible_result`'s `Ok` arm; nothing there changed).

- [ ] **Step 5: Commit**

```
git add crates/openalpaca_core/src/utils/text.rs crates/openalpaca_core/src/runner/agentic_loop/tool_helpers.rs crates/openalpaca_core/src/runner/agentic_loop/mod.rs crates/openalpaca_core/src/runner/agentic_loop/tests.rs
git commit -m "feat(core): head_tail moves to utils::text so the lead's tracker can use it (F2, 1/5)

The loop's head+tail cut for an Err result was pub(super) in a private
module of agentic_loop, unreachable from runner/lead_agent. It is now
crate::utils::text::head_tail, rewritten on floor/ceil_char_boundary with
the same notice; the loop's call site and its two tests follow it.

Verified: cargo test -p openalpaca_core utils::text::tests::head_tail (2
passed) and agentic_loop::tests::an_error_result_keeps_its_head_and_its_tail
(passed).

Co-Authored-By: <the line the implementing session is given>"
```

---

### Task F2.2: `summary()` shows each completed answer as head and tail within the inline bound

**Files:** Create: nothing. Modify: `crates/openalpaca_core/src/runner/lead_agent/tracker.rs` (imports `:1-2`, struct `:16-20`, `new` `:29-34`, `summary` `:94-127`, new constants and `answer_budget`); `crates/openalpaca_core/src/runner/agentic_loop/mod.rs:16-21` (re-export `MAX_TOOL_RESULT_SIZE`); `crates/openalpaca_core/src/runner/mod.rs:12`; `crates/openalpaca_core/src/runner/lead_agent/mod.rs:237`. Test: `crates/openalpaca_core/src/runner/lead_agent/tests.rs` (append after `test_tracker_summary_with_queued`, `:247-256`).

**Interfaces:** Consumes: `pub(crate) fn head_tail(text: &str, limit: usize) -> String` (Task F2.1); `pub(crate) const MAX_TOOL_RESULT_SIZE: usize = 32 * 1024` (`agentic_loop/tool_helpers.rs:4`); `daemon_config.load().orchestrator.sessions.tool_result_inline_bytes: usize` (`daemon_config/orchestrator.rs:100`, the read the lead already makes at `lead_agent/mod.rs:578-582`); `WaitForSubagentsTool { tracker: Arc<SubagentTracker>, steering: Option<Arc<SteeringInbox>> }` (`tools.rs:1158-1162`, `pub(super)` fields, built by struct literal in the tests). Produces: `SubagentTracker::with_inline_bytes(inline_bytes: usize) -> Self`; `pub(super) fn answer_budget(inline_bytes: usize, entries: usize) -> usize` in `tracker.rs` (`mod tracker;` is private at `lead_agent/mod.rs:12` and `mod tests;` is its sibling at `:792`, so `pub(super)` — visible in `lead_agent` and its descendants — lets the tests reach it as `super::tracker::answer_budget`); four of the five constants in the table above (`SUMMARY_ERROR_MAX_BYTES` is Task F2.3's, with its first use); `crate::runner::MAX_TOOL_RESULT_SIZE` (a `pub(crate)` re-export). The tests reach `SubagentTracker`, `SubagentStatus`, `WaitForSubagentsTool`, `CheckSubagentStatusTool`, the two `*_tool_definition` functions and `build_lead_agent_prompt_from_templates` through `use super::*;` and the `pub use` lines at `lead_agent/mod.rs:15-24`, `Arc` through `:47`, and `BuiltInTool` through the `#[cfg(test)]` import at `:53-54`.

- [ ] **Step 1: Write the failing test**

Append to `crates/openalpaca_core/src/runner/lead_agent/tests.rs` after `test_tracker_summary_with_queued` (`:256`):

```rust
// ── F2: the summary shows each answer's head and tail within the bound ──

/// The per-answer budget is a function of the loop's inline bound and the
/// number of tracked entries: `(bound − 1 KiB) / entries − 768`, floored
/// at 512 and capped at 8 KiB. The default bound and a batch of 8 give
/// 3,200 bytes an answer; one answer alone gets the cap; at the clamp
/// minimum 4,096 a batch of 8 sits on the floor.
#[test]
fn the_summarys_per_answer_budget_follows_the_bound_and_the_entry_count() {
    assert_eq!(super::tracker::answer_budget(32 * 1024, 8), 3_200);
    assert_eq!(super::tracker::answer_budget(32 * 1024, 1), 8_192);
    assert_eq!(super::tracker::answer_budget(32 * 1024, 2), 8_192);
    assert_eq!(super::tracker::answer_budget(32 * 1024, 4), 7_168);
    assert_eq!(super::tracker::answer_budget(32 * 1024, 16), 1_216);
    assert_eq!(super::tracker::answer_budget(32 * 1024, 25), 512);
    assert_eq!(super::tracker::answer_budget(4_096, 8), 512);
    // Zero entries never divide by zero (the empty summary returns earlier).
    assert_eq!(super::tracker::answer_budget(32 * 1024, 0), 8_192);
}

/// Eight 20 KB answers through the tool itself — header included — stay
/// under the default 32 KiB bound, and every answer's opening and closing
/// words reach the lead.
#[tokio::test]
async fn wait_for_subagents_keeps_eight_twenty_kilobyte_answers_under_the_inline_bound() {
    let tracker = Arc::new(SubagentTracker::new());
    for i in 1..=8 {
        let run_id = format!("run-{i}");
        tracker.register(&run_id);
        tracker.complete(
            &run_id,
            format!("HEAD-{i} {} TAIL-{i}", "x".repeat(20 * 1024)),
            true,
        );
    }
    let tool = WaitForSubagentsTool {
        tracker,
        steering: None,
    };
    let result = tool.execute(&serde_json::json!({})).await.unwrap();

    assert!(
        result.len() <= 32 * 1024,
        "the whole summary must fit the inline bound, got {} bytes",
        result.len()
    );
    assert!(
        result.starts_with("All subagents finished. 8 completed, 0 failed.\n\n"),
        "got: {}",
        crate::utils::text::byte_prefix(&result, 80)
    );
    for i in 1..=8 {
        assert!(result.contains(&format!("HEAD-{i} ")), "head {i} missing");
        assert!(result.contains(&format!(" TAIL-{i}")), "tail {i} missing");
    }
    assert_eq!(result.matches("the tail follows").count(), 8);
}

/// A CJK answer is cut on character boundaries: with one entry the budget
/// is the 8,192-byte cap, half is 4,096, not a multiple of 3, so the head
/// rounds down to 4,095 bytes (1,365 characters) and the tail rounds up to
/// start at 10,905, 4,095 bytes too. The whole line is pinned.
#[test]
fn a_cjk_answer_is_cut_on_character_boundaries_in_the_summary() {
    let tracker = SubagentTracker::new();
    tracker.register("run-1");
    tracker.complete("run-1", "語".repeat(5000), true);

    assert_eq!(
        tracker.summary(),
        format!(
            "- **run-1**: completed — {}\n\n[... 6810 of 15000 bytes elided; the tail follows ...]\n\n{}",
            "語".repeat(1365),
            "語".repeat(1365)
        )
    );
}

/// An answer within its budget is shown whole — 3,000 bytes under the
/// 8,192-byte cap one entry gets.
#[test]
fn an_answer_within_its_budget_is_shown_whole_in_the_summary() {
    let tracker = SubagentTracker::new();
    tracker.register("run-1");
    let answer = format!("Report: {}", "r".repeat(2992));
    tracker.complete("run-1", answer.clone(), true);

    assert_eq!(tracker.summary(), format!("- **run-1**: completed — {answer}"));
}

/// The bound is the tracker's, set at construction: an 8 KiB bound gives
/// one entry (8192 − 1024) − 768 = 6,400 bytes, head 3,200 and tail 3,200.
#[test]
fn a_tracker_built_with_a_smaller_bound_cuts_to_it() {
    let tracker = SubagentTracker::with_inline_bytes(8 * 1024);
    tracker.register("run-1");
    tracker.complete("run-1", "y".repeat(10_000), true);

    assert_eq!(
        tracker.summary(),
        format!(
            "- **run-1**: completed — {}\n\n[... 3600 of 10000 bytes elided; the tail follows ...]\n\n{}",
            "y".repeat(3200),
            "y".repeat(3200)
        )
    );
}

/// Review focus — more entries than the bound can hold (60 here, past the
/// 25 where the budget floors at 512 bytes): the summary is still produced,
/// still names every entry and shows each answer's opening and end, and is
/// longer than the bound, which the loop's own cut or spill then handles as
/// for any oversized tool result. Nothing is dropped silently.
#[test]
fn a_batch_past_the_bound_still_lists_every_entry() {
    let tracker = SubagentTracker::new();
    for i in 1..=60 {
        let run_id = format!("run-{i}");
        tracker.register(&run_id);
        tracker.complete(&run_id, format!("BEGIN-{i} {} END-{i}", "z".repeat(4000)), true);
    }
    assert_eq!(super::tracker::answer_budget(32 * 1024, 60), 512);
    let summary = tracker.summary();
    // Each entry is about 595 bytes (prefix, 256-byte head, notice, 256-byte
    // tail, newline): 60 of them are about 35,700 bytes, past the bound.
    for i in 1..=60 {
        assert!(summary.contains(&format!("- **run-{i}**: completed — BEGIN-{i} ")), "entry {i} missing");
        assert!(summary.contains(&format!(" END-{i}")), "tail {i} missing");
    }
    assert!(summary.len() > 32 * 1024, "past the floor the summary may exceed the bound: {} bytes", summary.len());
}
```

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test -p openalpaca_core lead_agent::tests::the_summarys_per_answer_budget`
Expected: FAIL to compile — `error[E0425]: cannot find function answer_budget in module tracker` and `error[E0599]: no function or associated item named with_inline_bytes found for struct SubagentTracker`.

- [ ] **Step 3: Write the minimal implementation**

`crates/openalpaca_core/src/runner/agentic_loop/mod.rs` — replace the import block from `pub(crate) use context::…` through `#[cfg(test)] use tool_helpers::{MAX_TOOL_RESULT_SIZE, truncate_tool_result};` (lines 16-20 after Task F2.1) with the five lines below, so `MAX_TOOL_RESULT_SIZE` is re-exported once, unconditionally, and the `#[cfg(test)]` import no longer names it (two imports of one name collide):

```rust
pub(crate) use context::{compress_context, estimate_messages_tokens, estimate_tools_tokens};
pub(crate) use tool_helpers::MAX_TOOL_RESULT_SIZE;
use crate::utils::text::head_tail;
use tool_helpers::{format_tool_error, format_tool_error_with_hint, truncate_tool_result_to};
#[cfg(test)]
use tool_helpers::truncate_tool_result;
```

(The agentic loop's tests keep seeing `MAX_TOOL_RESULT_SIZE` through `use super::*;`; the import is now visible in every build, and the tracker below is its non-test user, so no `unused_imports` warning.)

`crates/openalpaca_core/src/runner/mod.rs:12` — add the constant to the crate-visible re-export:

```rust
pub(crate) use agentic_loop::{
    MAX_TOOL_RESULT_SIZE, compress_context, estimate_messages_tokens, estimate_tools_tokens,
};
```

`crates/openalpaca_core/src/runner/lead_agent/tracker.rs` — imports (`:1-2`):

```rust
use crate::runner::MAX_TOOL_RESULT_SIZE;
use crate::utils::text::head_tail;
use std::collections::HashMap;
use std::sync::Mutex;
```

After the `SubagentStatus` enum (`:11`) and before `SubagentTracker` (`:13`), the constants and the budget:

```rust
/// What `wait_for_subagents` prepends to the summary (the interrupted return
/// is the longest, about 180 bytes).
const SUMMARY_HEADER_RESERVE_BYTES: usize = 1024;
/// One entry's fixed cost: `- **<run_id>**: completed — `, up to two
/// head/tail notices, ` Last output: `, the join, and a failed entry's error
/// cut to 512 bytes.
const SUMMARY_ENTRY_OVERHEAD_BYTES: usize = 768;
/// One answer never takes more of the lead's context than this, however few
/// subagents ran — `check_subagent_status` returns the whole answer.
const SUMMARY_ANSWER_CAP_BYTES: usize = 8 * 1024;
/// Below this an entry says nothing useful; from about 25 entries on the
/// summary may exceed the bound and the loop's own spill or cut takes over.
const SUMMARY_ANSWER_FLOOR_BYTES: usize = 512;

/// How many bytes of each answer `summary` shows: the loop's inline bound,
/// less the header, shared by every tracked entry, less each entry's fixed
/// cost — floored and capped. For the default 32 KiB bound and a batch of
/// 8: 3,200 bytes an answer, 1,600 head and 1,600 tail, so the worst case
/// `8 × (3,200 + 768) + 1,024` is exactly the bound.
pub(super) fn answer_budget(inline_bytes: usize, entries: usize) -> usize {
    let per_entry = inline_bytes.saturating_sub(SUMMARY_HEADER_RESERVE_BYTES) / entries.max(1);
    per_entry
        .saturating_sub(SUMMARY_ENTRY_OVERHEAD_BYTES)
        .clamp(SUMMARY_ANSWER_FLOOR_BYTES, SUMMARY_ANSWER_CAP_BYTES)
}
```

The struct (`:16-20`) gains the bound:

```rust
pub struct SubagentTracker {
    pub statuses: Mutex<HashMap<String, SubagentStatus>>,
    /// Notifies waiters when a subagent completes or fails.
    pub notify: tokio::sync::Notify,
    /// `[orchestrator.sessions] tool_result_inline_bytes` of the loop that
    /// reads `summary()` — the bound the whole summary is sized to stay under.
    inline_bytes: usize,
}
```

The constructors (`:29-34`):

```rust
    /// A tracker sized to the loop's default inline bound (32 KiB) — tests
    /// and any caller with no `LoopConfig` of its own.
    pub fn new() -> Self {
        Self::with_inline_bytes(MAX_TOOL_RESULT_SIZE)
    }

    /// A tracker whose `summary()` stays under `inline_bytes` for a batch of
    /// eight: the lead passes its own `tool_result_inline_bytes`.
    pub fn with_inline_bytes(inline_bytes: usize) -> Self {
        Self {
            statuses: Mutex::new(HashMap::new()),
            notify: tokio::sync::Notify::new(),
            inline_bytes,
        }
    }
```

`summary()` (`:94-127`) — compute the budget once and cut the completed answer with it (the failed arm is unchanged in this task):

```rust
    pub fn summary(&self) -> String {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        if map.is_empty() {
            return "No subagents have been spawned yet.".to_string();
        }
        let budget = answer_budget(self.inline_bytes, map.len());
        let mut parts = Vec::new();
        for (id, status) in map.iter() {
            match status {
                SubagentStatus::Queued => {
                    parts.push(format!("- **{}**: queued (waiting for execution slot)", id));
                }
                SubagentStatus::Running => {
                    parts.push(format!("- **{}**: still running", id));
                }
                SubagentStatus::Completed { content, success } => {
                    parts.push(format!(
                        "- **{}**: {} — {}",
                        id,
                        if *success {
                            "completed"
                        } else {
                            "completed (partial)"
                        },
                        head_tail(content, budget)
                    ));
                }
                SubagentStatus::Failed { error } => {
                    parts.push(format!("- **{}**: failed — {}", id, error));
                }
            }
        }
        parts.join("\n")
    }
```

`crates/openalpaca_core/src/runner/lead_agent/mod.rs:237` — the production tracker takes the lead's bound, the same read `:578-582` makes for its `LoopConfig`:

```rust
    // 4. Build coordination tools with shared SubagentTracker, sized to the
    //    inline bound the lead's own loop applies to every tool result, so a
    //    batch of eight answers reaches the lead as one in-bound summary (F2).
    let tracker = Arc::new(SubagentTracker::with_inline_bytes(
        daemon_config
            .load()
            .orchestrator
            .sessions
            .tool_result_inline_bytes,
    ));
```

- [ ] **Step 4: Run the test to see it pass**

Run: `cargo test -p openalpaca_core lead_agent::tests::the_summarys_per_answer_budget`
Expected: PASS.

Run: `cargo test -p openalpaca_core lead_agent::tests::wait_for_subagents_keeps_eight`
Expected: PASS (the result measures 26,343 bytes).

Run: `cargo test -p openalpaca_core lead_agent::tests::a_cjk_answer_is_cut`
Expected: PASS.

Run: `cargo test -p openalpaca_core lead_agent::tests::an_answer_within_its_budget`
Expected: PASS.

Run: `cargo test -p openalpaca_core lead_agent::tests::a_tracker_built_with_a_smaller_bound`
Expected: PASS.

Run: `cargo test -p openalpaca_core lead_agent::tests::a_batch_past_the_bound`
Expected: PASS.

Run: `cargo test -p openalpaca_core lead_agent::tests::test_tracker`
Expected: PASS, all 10 pre-existing tracker tests (`test_tracker_summary` still finds "Research result" whole — 15 bytes is within any budget).

Run: `cargo test -p openalpaca_core agentic_loop::tests::`
Expected: PASS — the loop's tests still see `MAX_TOOL_RESULT_SIZE` and `truncate_tool_result` through the glob.

- [ ] **Step 5: Commit**

```
git add crates/openalpaca_core/src/runner/lead_agent/tracker.rs crates/openalpaca_core/src/runner/lead_agent/mod.rs crates/openalpaca_core/src/runner/lead_agent/tests.rs crates/openalpaca_core/src/runner/agentic_loop/mod.rs crates/openalpaca_core/src/runner/mod.rs
git commit -m "feat(core): wait_for_subagents shows each answer's head and tail within the inline bound (F2, 2/5)

The summary showed a completed subagent's first 500 chars with no notice,
which cut a report's path off its end. Each answer is now head and tail,
in bytes on character boundaries, under a per-answer budget derived from
the lead's tool_result_inline_bytes and the number of tracked entries
(3,200 bytes an answer for the default bound and a batch of 8, 1,600 head
and 1,600 tail; capped at 8 KiB, floored at 512). The tracker takes the
bound at construction.

Verified: cargo test -p openalpaca_core lead_agent::tests (the five new
tests and the ten tracker tests pass; eight 20 KB answers through the tool
measure 26,343 bytes) and agentic_loop::tests (pass).

Co-Authored-By: <the line the implementing session is given>"
```

---

### Task F2.3: a failed subagent's whole answer is kept beside its error

**Files:** Create: nothing. Modify: `crates/openalpaca_core/src/runner/lead_agent/tracker.rs` (`SubagentStatus::Failed` `:10`, `fail` `:51-56`, the failed arm of `summary`); `crates/openalpaca_core/src/runner/lead_agent/tools.rs:926-934` (the spawn path's failure) and `:1116-1118` (pattern only; Task F2.4 changes the behaviour); `crates/openalpaca_core/src/runner/lead_agent/tests.rs:121-135` (`test_tracker_fail`'s pattern). Test: `crates/openalpaca_core/src/runner/lead_agent/tests.rs`.

**Interfaces:** Consumes: `LoopResult { final_content: String, finish_reason: LoopFinishReason, .. }` (`agentic_loop/config.rs:325-331`; `LoopFinishReason` derives `Debug`, it is already formatted with `{:?}` at `tools.rs:931`; `result` is the owned binding of `tools.rs:827`, read for the last time at `:922`/`:932`); `head_tail` (Task F2.1); `answer_budget` (Task F2.2). Produces: `SubagentStatus::Failed { error: String, output: String }`; `SubagentTracker::fail_with_output(&self, run_id: &str, error: String, output: String)`; `fail(&self, run_id, error)` unchanged in signature, storing an empty `output`; `const SUMMARY_ERROR_MAX_BYTES: usize = 512` in `tracker.rs`.

Every pattern on the variant must change with it: `tracker.rs` (`summary`'s arm; `all_done` `:75` and `status_counts` `:88` already use `Failed { .. }`), `tools.rs:1116`, `tests.rs:129`. The other two `tracker.fail(...)` callers keep `fail`: `tools.rs:650-653` (cancelled before starting — no output exists) and `:811-814` (a plugin loop's `Failed { error, .. }` carries no content, `runner/plugin_agent.rs:76-79`).

- [ ] **Step 1: Write the failing test**

In `crates/openalpaca_core/src/runner/lead_agent/tests.rs`, replace `test_tracker_fail` (`:121-135`) with:

```rust
#[test]
fn test_tracker_fail() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    tracker.fail("run-1", "Some error".to_string());

    match tracker.get("run-1") {
        Some(SubagentStatus::Failed { error, output }) => {
            assert_eq!(error, "Some error");
            assert!(output.is_empty(), "fail() keeps no output: {output:?}");
        }
        other => panic!("Expected Failed, got {:?}", other),
    }
    assert!(tracker.all_done());
}
```

and append, after `a_tracker_built_with_a_smaller_bound_cuts_to_it` (Task F2.2):

```rust
// ── F2: a failed subagent's answer is kept whole ──

/// The tracker keeps a failed subagent's whole final content beside its
/// error — 1,221 characters here, past the 500 the old error string held,
/// in a script where 500 chars were 1,500 bytes.
#[test]
fn a_failed_subagents_whole_answer_is_kept() {
    let tracker = SubagentTracker::new();
    tracker.register("run-1");
    let answer = format!("{}\n最後の行: /tmp/report.md", "見出し".repeat(400));
    assert!(answer.chars().count() > 500);

    tracker.fail_with_output(
        "run-1",
        "Agent 'writer' failed: MaxRounds.".to_string(),
        answer.clone(),
    );

    match tracker.get("run-1") {
        Some(SubagentStatus::Failed { error, output }) => {
            assert_eq!(error, "Agent 'writer' failed: MaxRounds.");
            assert_eq!(output, answer);
        }
        other => panic!("Expected Failed, got {:?}", other),
    }
    assert!(tracker.all_done());
}

/// The summary shows a failed entry's error, then the answer's head and tail
/// under the same budget a completed answer gets: one entry, the 8,192-byte
/// cap, head 4,096 and a tail of 4,096 that ends on the path.
#[test]
fn the_summary_shows_a_failed_answers_error_and_its_head_and_tail() {
    let tracker = SubagentTracker::new();
    tracker.register("run-1");
    let output = format!("{}/tmp/report.md", "A".repeat(10_000));
    tracker.fail_with_output(
        "run-1",
        "Agent 'writer' failed: MaxRounds.".to_string(),
        output,
    );

    assert_eq!(
        tracker.summary(),
        format!(
            "- **run-1**: failed — Agent 'writer' failed: MaxRounds. Last output: {}\n\n[... 1822 of 10014 bytes elided; the tail follows ...]\n\n{}/tmp/report.md",
            "A".repeat(4096),
            "A".repeat(4082)
        )
    );
}

/// A failure with no output reads as it did before, and a long error text
/// is cut to 512 bytes — 256 head, 256 tail — so a provider's or a plugin's
/// message cannot eat the batch's budget.
#[test]
fn a_failed_entry_with_no_output_shows_its_error_alone_and_a_long_error_is_cut() {
    let tracker = SubagentTracker::new();
    tracker.register("run-1");
    tracker.fail(
        "run-1",
        "Cancelled before starting (parent task was cancelled)".to_string(),
    );
    assert_eq!(
        tracker.summary(),
        "- **run-1**: failed — Cancelled before starting (parent task was cancelled)"
    );

    let tracker = SubagentTracker::new();
    tracker.register("run-2");
    tracker.fail("run-2", "E".repeat(2000));
    let summary = tracker.summary();
    assert_eq!(
        summary,
        format!(
            "- **run-2**: failed — {}\n\n[... 1488 of 2000 bytes elided; the tail follows ...]\n\n{}",
            "E".repeat(256),
            "E".repeat(256)
        )
    );
    assert!(!summary.contains("Last output:"));
}
```

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test -p openalpaca_core lead_agent::tests::a_failed_subagents_whole_answer_is_kept`
Expected: FAIL to compile — `error[E0026]: variant SubagentStatus::Failed does not have a field named output` (at the rewritten `test_tracker_fail` and the new tests) and `error[E0599]: no method named fail_with_output found`.

- [ ] **Step 3: Write the minimal implementation**

`crates/openalpaca_core/src/runner/lead_agent/tracker.rs` — the variant (`:10`):

```rust
    /// `error` says why; `output` is the subagent's whole final content when
    /// its loop produced one (empty for a plugin loop's failure and for a run
    /// cancelled before it started). Kept whole, as a completed answer is:
    /// the lead reads it with `check_subagent_status`.
    Failed { error: String, output: String },
```

`fail` (`:51-56`) becomes a wrapper, and `fail_with_output` the one writer:

```rust
    /// A failure that carries no answer — cancelled before it started, or a
    /// plugin loop's error.
    pub fn fail(&self, run_id: &str, error: String) {
        self.fail_with_output(run_id, error, String::new());
    }

    /// A failure with the subagent's whole final content beside it.
    pub fn fail_with_output(&self, run_id: &str, error: String, output: String) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(run_id.to_string(), SubagentStatus::Failed { error, output });
        drop(map);
        self.notify.notify_waiters();
    }
```

The fifth constant, after `SUMMARY_ANSWER_FLOOR_BYTES` (Task F2.2) and before `answer_budget`:

```rust
/// A failed entry's error text, which `LoopFinishReason::Error(String)` and a
/// plugin's error make unbounded.
const SUMMARY_ERROR_MAX_BYTES: usize = 512;
```

The failed arm of `summary()`:

```rust
                SubagentStatus::Failed { error, output } => {
                    let mut line = format!(
                        "- **{}**: failed — {}",
                        id,
                        head_tail(error, SUMMARY_ERROR_MAX_BYTES)
                    );
                    if !output.is_empty() {
                        line.push_str(" Last output: ");
                        line.push_str(&head_tail(output, budget));
                    }
                    parts.push(line);
                }
```

`crates/openalpaca_core/src/runner/lead_agent/tools.rs:925-935` — the spawn path keeps the whole content and a short error; the `chars().take(500)` cut goes:

```rust
            } else {
                // F2: the error says why; the answer travels whole beside it,
                // as a completed one does, so the lead can read it with
                // `check_subagent_status` and the summary can show its tail.
                tracker.fail_with_output(
                    &run_id_clone,
                    format!("Agent '{}' failed: {:?}.", agent_id_owned, result.finish_reason),
                    result.final_content,
                );
            }
```

(`result` is owned here and nothing reads it after this `if`; the success arm at `:922` already moves `result.final_content`.)

`crates/openalpaca_core/src/runner/lead_agent/tools.rs:1116` — the pattern must compile; the behaviour is Task F2.4's:

```rust
            Some(SubagentStatus::Failed { error, .. }) => {
                Err(format!("Subagent '{}' failed: {}", run_id, error))
            }
```

- [ ] **Step 4: Run the test to see it pass**

Run: `cargo test -p openalpaca_core lead_agent::tests::a_failed_subagents_whole_answer_is_kept`
Expected: PASS.

Run: `cargo test -p openalpaca_core lead_agent::tests::the_summary_shows_a_failed_answers_error`
Expected: PASS.

Run: `cargo test -p openalpaca_core lead_agent::tests::a_failed_entry_with_no_output`
Expected: PASS.

Run: `cargo test -p openalpaca_core lead_agent::tests::test_tracker`
Expected: PASS, all 10 (`test_tracker_fail` with its new pattern, `test_tracker_summary` still finds "Timeout" whole).

Run: `cargo test -p openalpaca_core lead_agent::tests::test_check_subagent_status_tool`
Expected: PASS, 2 tests (the completed and queued paths; the failed path is not yet tested — Task F2.4).

Run: `grep -n "take(500)" crates/openalpaca_core/src/runner/lead_agent/tools.rs crates/openalpaca_core/src/runner/lead_agent/tracker.rs`
Expected: exactly one line, `tools.rs:319` (`"objective": objective.chars().take(500)` — the spawn record's objective preview, not an answer cut). The spawn path itself needs a model and has no unit test; this grep is the check that its cut is gone.

- [ ] **Step 5: Commit**

```
git add crates/openalpaca_core/src/runner/lead_agent/tracker.rs crates/openalpaca_core/src/runner/lead_agent/tools.rs crates/openalpaca_core/src/runner/lead_agent/tests.rs
git commit -m "fix(core): a failed subagent's whole answer is kept beside its error (F2, 3/5)

The spawn path stored the first 500 chars of a failed subagent's answer
inside the error string and the tracker had no field for the rest, so
neither the summary nor check_subagent_status could show its end.
SubagentStatus::Failed carries the whole output now; the summary shows the
error (cut to 512 bytes if long) and the answer's head and tail under the
same budget a completed answer gets.

Verified: cargo test -p openalpaca_core lead_agent::tests (three new tests
and the tracker tests pass); grep take(500) over tracker.rs and tools.rs
finds only the objective preview at tools.rs:319.

Co-Authored-By: <the line the implementing session is given>"
```

---

### Task F2.4: `check_subagent_status` returns a failed subagent's whole answer

**Files:** Create: nothing. Modify: `crates/openalpaca_core/src/runner/lead_agent/tools.rs:1116-1118` (the failed arm of `CheckSubagentStatusTool::execute`). Test: `crates/openalpaca_core/src/runner/lead_agent/tests.rs`.

**Interfaces:** Consumes: `SubagentStatus::Failed { error, output }` (Task F2.3); `CheckSubagentStatusTool { tracker: Arc<SubagentTracker> }` (`tools.rs:1085-1087`, built by struct literal in the tests); `BuiltInTool::execute(&self, &serde_json::Value) -> Result<String, String>`. Produces: on the failed path, `Err("Subagent '<run_id>' failed: <error>\nIts last output follows, whole:\n<output>")` when `output` is non-empty, else today's `Err("Subagent '<run_id>' failed: <error>")`.

**The path stays `Err` — a decision, recorded in the open points.** The loop hands an `Err` to the model as head and tail at `tool_result_inline_bytes` with **no `result_ref`** (`agentic_loop/mod.rs:286-288` `model_visible_result`: `if !ok { return head_tail(..) }`), although the bytes are spilled to the session's `results/` (`spill_plan`, `:307-320`, ignores `ok`). An `Ok` would give a long failed answer the 2 KB stub + `result_ref` path, but would change the loop's accounting of the call (`ok = !result_text.starts_with("[tool_error]")`, `:1181`; `last_tool_error`, `:1186`; the `tool_executed` event's success flag). Minimal impact keeps `Err`; the description in Task F2.5 says what each path returns.

- [ ] **Step 1: Write the failing test**

Append to `crates/openalpaca_core/src/runner/lead_agent/tests.rs` after `a_failed_entry_with_no_output_shows_its_error_alone_and_a_long_error_is_cut` (Task F2.3):

```rust
// ── F2: check_subagent_status returns a failed subagent's whole answer ──

/// The tool returns the kept output whole — 3,015 characters, 9,015 bytes
/// here — after the error; the loop, not the tool, bounds what the model
/// sees.
#[tokio::test]
async fn check_subagent_status_returns_a_failed_subagents_whole_answer() {
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-f");
    let output = format!("{}\n/tmp/report.md", "結果".repeat(1500));
    tracker.fail_with_output(
        "run-f",
        "Agent 'writer' failed: MaxRounds.".to_string(),
        output.clone(),
    );
    let tool = CheckSubagentStatusTool {
        tracker: tracker.clone(),
    };

    let err = tool
        .execute(&serde_json::json!({"subagent_run_id": "run-f"}))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        format!(
            "Subagent 'run-f' failed: Agent 'writer' failed: MaxRounds.\nIts last output follows, whole:\n{output}"
        )
    );
}

/// A failure that carries no answer names the error alone, as before.
#[tokio::test]
async fn check_subagent_status_on_a_failure_with_no_output_names_the_error_alone() {
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-g");
    tracker.fail(
        "run-g",
        "Cancelled before starting (parent task was cancelled)".to_string(),
    );
    let tool = CheckSubagentStatusTool {
        tracker: tracker.clone(),
    };

    let err = tool
        .execute(&serde_json::json!({"subagent_run_id": "run-g"}))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "Subagent 'run-g' failed: Cancelled before starting (parent task was cancelled)"
    );
}
```

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test -p openalpaca_core lead_agent::tests::check_subagent_status_returns_a_failed`
Expected: FAIL — `assertion left == right failed`: the left side is `Subagent 'run-f' failed: Agent 'writer' failed: MaxRounds.` with no output after it. (`check_subagent_status_on_a_failure_with_no_output_names_the_error_alone` already passes; it pins the unchanged path.)

- [ ] **Step 3: Write the minimal implementation**

`crates/openalpaca_core/src/runner/lead_agent/tools.rs:1116-1118`:

```rust
            Some(SubagentStatus::Failed { error, output }) => {
                // F2: the kept answer, whole. An `Err` reaches the model as
                // head and tail at the loop's inline bound; the tool itself
                // cuts nothing.
                if output.is_empty() {
                    Err(format!("Subagent '{}' failed: {}", run_id, error))
                } else {
                    Err(format!(
                        "Subagent '{}' failed: {}\nIts last output follows, whole:\n{}",
                        run_id, error, output
                    ))
                }
            }
```

- [ ] **Step 4: Run the test to see it pass**

Run: `cargo test -p openalpaca_core lead_agent::tests::check_subagent_status`
Expected: PASS, 2 tests (`check_subagent_status_returns_a_failed_subagents_whole_answer`, `check_subagent_status_on_a_failure_with_no_output_names_the_error_alone`).

Run: `cargo test -p openalpaca_core lead_agent::tests::test_check_subagent_status_tool`
Expected: PASS, 2 tests (completed and queued paths unchanged).

- [ ] **Step 5: Commit**

```
git add crates/openalpaca_core/src/runner/lead_agent/tools.rs crates/openalpaca_core/src/runner/lead_agent/tests.rs
git commit -m "fix(core): check_subagent_status returns a failed subagent's whole answer (F2, 4/5)

The failed path returned the error string alone, which until the previous
commit held a 500-char prefix of the answer. It now returns the error and
the whole kept output; the loop bounds the model's copy as head and tail.

Verified: cargo test -p openalpaca_core lead_agent::tests::check_subagent_status
(2 new tests pass) and ::test_check_subagent_status_tool (2 pass).

Co-Authored-By: <the line the implementing session is given>"
```

---

### Task F2.5: the descriptions, the lead prompt and the record say what the tools return

**Files:** Create: nothing. Modify: `crates/openalpaca_core/src/runner/lead_agent/tools.rs:1131-1134` and `:1245-1248` (the two descriptions); `crates/openalpaca_core/src/runner/lead_agent/prompt.rs:157-161` (the two `<tools>` lines); `crates/openalpaca_core/src/compose/tests.rs:2801-2805` (the golden's verbatim copy of those lines). Test: `crates/openalpaca_core/src/runner/lead_agent/tests.rs`. Not `CLAUDE.md`: its Round G clause is the stage's Task R (see *After the five tasks*).

**Interfaces:** Consumes: `pub fn check_subagent_status_tool_definition() -> ToolDefinition` (`tools.rs:1128`), `pub fn wait_for_subagents_tool_definition() -> ToolDefinition` (`:1242`), `ToolDefinition { description: String, .. }`; `pub fn build_lead_agent_prompt_from_templates(compose_engine: &ComposeEngine, base_persona: &str, templates: &[AgentTemplate], model_window: usize, user_document: Option<&UserDocument>, user_budget: Option<usize>) -> String` (`prompt.rs:86-93`) — called in the tests as `build_lead_agent_prompt_from_templates(&engine, "Test persona", &[], 200_000, None, None)` (`lead_agent/tests.rs:594-595`). Produces: the two description strings and the two prompt lines below, pinned.

**About the golden.** The grep for a pinned prompt test finds one: `compose/tests.rs:2665-2969` `test_golden_lead_agent_byte_identical` (doc comment from `:2665`, `fn` at `:2681`, closing brace `:2969`). It is a **self-copy**, not a pin: the `<tools>` block at `:2794-2808` is a local string used on *both* sides of its own assertion (`PromptBuilder` at `:2823-2836` and the engine's `raw_blocks_for_engine` at `:2873-2877`), and the test never calls `build_lead_agent_prompt_from_templates`. Changing `prompt.rs` therefore does not fail it; the copy is updated below only so it stays verbatim, as its comment at `:2761-2762` promises. The failing test for the prompt half is new (Step 1). `lead_agent/tests.rs:592-604` pins "spawn_subagents_batch" and "3+ independent subagents", both untouched.

- [ ] **Step 1: Write the failing test**

Append to `crates/openalpaca_core/src/runner/lead_agent/tests.rs` after `check_subagent_status_on_a_failure_with_no_output_names_the_error_alone` (Task F2.4):

```rust
// ── F2: the descriptions and the prompt say what the tools return ──

#[test]
fn check_subagent_status_description_says_it_returns_the_whole_answer() {
    assert_eq!(
        check_subagent_status_tool_definition().description,
        "Check one spawned subagent by its run_id. Queued or running: says so. \
         Completed: returns the subagent's whole answer — a long one arrives as a stub \
         naming a result_ref; page it with read_result. Failed: returns the error and the \
         subagent's whole last output, as its head and tail when long. wait_for_subagents \
         shows only the head and the tail of each answer; call this for the one you need \
         in full."
    );
}

#[test]
fn wait_for_subagents_description_says_it_shows_each_answers_head_and_tail() {
    assert_eq!(
        wait_for_subagents_tool_definition().description,
        "Wait for ALL previously spawned subagents to complete, including any that are \
         queued for execution. Returns a summary of all results: each subagent's answer as \
         its head and its tail, with a notice naming the bytes between them, sized so that \
         a batch of eight fits one tool result. Read one answer in full with \
         check_subagent_status. Use this after spawning all subagents to collect all \
         outputs before synthesizing a response."
    );
}

#[test]
fn the_lead_prompt_says_check_subagent_status_returns_the_whole_answer() {
    let engine = crate::compose::ComposeEngine::new(16);
    let prompt =
        build_lead_agent_prompt_from_templates(&engine, "Test persona", &[], 200_000, None, None);
    assert!(
        prompt.contains(
            "check_subagent_status: Poll a single subagent by run_id. Shows whether the subagent is \
             queued, running, completed, or failed, and returns a finished subagent's whole answer \
             — a long one arrives as a stub to page with read_result."
        ),
        "{prompt}"
    );
    assert!(
        prompt.contains(
            "wait_for_subagents: Block until ALL spawned subagents finish, including any that are \
             queued for execution. Returns a summary of all results: each answer's head and tail, \
             with a notice of what lies between — read one in full with check_subagent_status. \
             Call this after spawning all subagents."
        ),
        "{prompt}"
    );
}
```

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test -p openalpaca_core description_says`
(The filter is a substring of the full test path; `lead_agent::tests::description_says` would match nothing, since `check_subagent_status_` and `wait_for_subagents_` sit between `tests::` and `description_says`. No other test in the crate contains `description_says`.)
Expected: FAIL, 2 tests — `assertion left == right failed`, the left sides being today's descriptions ("Check the status of a previously spawned subagent. Returns whether the subagent is queued (waiting for an execution slot), running, completed, or failed." and "Wait for ALL previously spawned subagents to complete, including any that are queued for execution. Returns a summary of all results. Use this after spawning all subagents to collect all outputs before synthesizing a response.").

Run: `cargo test -p openalpaca_core lead_agent::tests::the_lead_prompt_says_check_subagent_status`
Expected: FAIL — the first `assert!` panics and prints the prompt, whose `<tools>` block still reads "Shows whether the subagent is queued, running, completed, or failed.\n".

- [ ] **Step 3: Write the minimal implementation**

`crates/openalpaca_core/src/runner/lead_agent/tools.rs:1131-1134` (inside `check_subagent_status_tool_definition`):

```rust
        description: "Check one spawned subagent by its run_id. Queued or running: says so. \
                      Completed: returns the subagent's whole answer — a long one arrives as a stub \
                      naming a result_ref; page it with read_result. Failed: returns the error and the \
                      subagent's whole last output, as its head and tail when long. wait_for_subagents \
                      shows only the head and the tail of each answer; call this for the one you need \
                      in full."
            .to_string(),
```

`tools.rs:1245-1248` (inside `wait_for_subagents_tool_definition`):

```rust
        description: "Wait for ALL previously spawned subagents to complete, including any that are \
                      queued for execution. Returns a summary of all results: each subagent's answer as \
                      its head and its tail, with a notice naming the bytes between them, sized so that \
                      a batch of eight fits one tool result. Read one answer in full with \
                      check_subagent_status. Use this after spawning all subagents to collect all \
                      outputs before synthesizing a response."
            .to_string(),
```

(In a Rust string literal a `\` at the end of a line swallows the newline and the next line's leading whitespace, so each line must end with the space that separates its last word from the next line's first — as the existing descriptions do.)

`crates/openalpaca_core/src/runner/lead_agent/prompt.rs:157-161` — replace

```rust
         check_subagent_status: Poll a single subagent by run_id. Shows whether the subagent is \
         queued, running, completed, or failed.\n\
         wait_for_subagents: Block until ALL spawned subagents finish, including any that are \
         queued for execution. Returns a summary of all results. Call this after spawning all \
         subagents.\n\
```

with

```rust
         check_subagent_status: Poll a single subagent by run_id. Shows whether the subagent is \
         queued, running, completed, or failed, and returns a finished subagent's whole answer \
         — a long one arrives as a stub to page with read_result.\n\
         wait_for_subagents: Block until ALL spawned subagents finish, including any that are \
         queued for execution. Returns a summary of all results: each answer's head and tail, \
         with a notice of what lies between — read one in full with check_subagent_status. \
         Call this after spawning all subagents.\n\
```

`crates/openalpaca_core/src/compose/tests.rs:2801-2805` — the same replacement, byte for byte, inside `tool_patterns_text` (the copy the golden's comment at `:2761-2762` calls verbatim).

(No `CLAUDE.md` edit here. The Round G clause at `CLAUDE.md:64` — "because `wait_for_subagents` hands the lead the first 500 characters of each subagent's answer (`runner/lead_agent/tracker.rs:109`), which cut the path; recorded, not built." — is replaced by the stage's Task R, which finds it by that wording; rewriting it in this task would take Task R's anchor away. The "Code Navigation" row for `runner/lead_agent/` needs no change either; `head_tail` lives beside `byte_prefix` in `utils/text.rs`, which the Gotchas bullet on approval surfaces already names.)

- [ ] **Step 4: Run the test to see it pass**

Run: `cargo test -p openalpaca_core description_says`
Expected: PASS, 2 tests.

Run: `cargo test -p openalpaca_core lead_agent::tests::the_lead_prompt_says_check_subagent_status`
Expected: PASS.

Run: `cargo test -p openalpaca_core lead_agent::tests::test_lead_agent_prompt_includes_batch_spawn_instruction`
Expected: PASS (its two pins, "spawn_subagents_batch" and "3+ independent subagents", are untouched).

Run: `cargo test -p openalpaca_core compose::tests::test_golden_lead_agent_byte_identical`
Expected: PASS (it compares its own two copies; it passed before the edit too — the edit keeps the copy verbatim).

Run: `cargo test -p openalpaca_core lead_agent::tests::test_lead_agent_registry_contains_coordination_tools`
Expected: PASS (the definitions still register under their names).

- [ ] **Step 5: Commit**

```
git add crates/openalpaca_core/src/runner/lead_agent/tools.rs crates/openalpaca_core/src/runner/lead_agent/prompt.rs crates/openalpaca_core/src/compose/tests.rs crates/openalpaca_core/src/runner/lead_agent/tests.rs
git commit -m "fix(core): check_subagent_status and wait_for_subagents say what they return (F2, 5/5)

The description and the lead prompt said check_subagent_status shows a
status; it returns a finished subagent's whole answer, a long one as a
stub to page with read_result, and wait_for_subagents shows each answer's
head and tail. Both descriptions are pinned, and the compose golden's
verbatim copy of the prompt lines follows them.

Verified: cargo test -p openalpaca_core lead_agent::tests (the three new
tests pass; test_lead_agent_prompt_includes_batch_spawn_instruction passes)
and compose::tests::test_golden_lead_agent_byte_identical (passes).

Co-Authored-By: <the line the implementing session is given>"
```

---

### After the five tasks

- The stage's one full run (`cargo test --workspace --exclude openalpaca_gui`, then clippy) happens at the end of the stage, not here. Nothing in these tasks touches a route, a migration, a config key, an event variant or the GUI.
- `tasks/storage-design/design.md` rows that read F2 as unbuilt (`:70`, `:101`, `:636`, `:659`) are the stage close's to update, not this fix's. So is `CLAUDE.md:64`'s Round G clause ("`wait_for_subagents` hands the lead the first 500 characters … recorded, not built"): the stage's Task R replaces it, with F2's commit range, and finds it by its present wording — leave it as it is through the five tasks.
- This draft lives under `tasks/storage-design/plan-drafts/`, which `.gitignore:127` ignores; it is not committed.

### Open points (could not be settled from the code alone)

1. **`check_subagent_status`'s failed path stays `Err`.** An `Err` reaches the model as head and tail at the inline bound with no `result_ref` (`agentic_loop/mod.rs:286-288`), so a long failed answer cannot be paged with `read_result` from the model's copy, although the bytes are in the session's `results/`. Returning `Ok` would give it the stub + `result_ref` path but would change the loop's error accounting (`:1181`, `:1186`, the `tool_executed` event). The draft keeps `Err` for minimal impact and words the description accordingly; the owner may prefer `Ok`.
2. **Queued and running entries count toward `entries`** in `answer_budget` (`map.len()`), under-using the bound by about 60 bytes each but keeping the formula one line. Counting finished entries only would give a partly finished batch more per answer; not done.
3. **The bound's clamp minimum.** At `tool_result_inline_bytes = 4096` (`validation.rs:195`) a batch of 8 floors at 512 bytes an answer and the summary (about 11 KB worst case) exceeds the bound; the loop's own cut or spill takes over, as for any oversized tool result. No separate handling is proposed.
4. **The 1:1 head:tail split** is chosen to match the loop's `Err` cut; a 2:1 split would favour the answer's opening. Either is a one-constant change in `head_tail`'s callers only if the helper grows a split parameter, which this draft does not add.
5. **The spawn path (`tools.rs:926-934`) has no unit test** — it needs a model. Its change is checked by the grep in Task F2.3 Step 4 and by reading; a live check (a subagent that exits `MaxRounds` with a long answer, then `check_subagent_status`) belongs to the stage's acceptance run.

---

## Fix F3 — the master key leaves the daemon's environment

Stage 0 of `tasks/storage-design/design.md` (row F3, design.md:102; the gate at design.md:586: "a spawned shell shows no master key"). Facts: `facts-stage0.md` A4, A5, further findings 4 and 5. Line numbers below are as of commit `d569885b` (the facts file's `032f6ae2` numbers hold for every file named here; I re-read each one).

### What is wrong today, in one paragraph

`apps/openalpacad/src/main.rs:146-151` takes the master key from `state/.master_key` (`KeyEncryptor::ensure_at`) and then **exports it**: `unsafe { std::env::set_var("OPENALPACA_MASTER_KEY", &hex_key); }`. The only reader of that variable in the tree is `KeyEncryptor::from_env_opt` (`crates/openalpaca_llm/src/keys/key_encryption/mod.rs:66-71`), inside the daemon's own process — but a process environment is inherited by every child, and no spawn site in the tree clears or filters it (A5: `tools/platform.rs:14-16` the shell, `tools/builtins/mod.rs:551-559` skill scripts, `openalpaca_mcp/src/transport/stdio.rs:64-71` MCP servers, `openalpaca_plugins/src/process_pool.rs:36-48` plugins, `openalpaca_llm/src/cli_backend/mod.rs:93-95` and `:193-195` the CLI model backends). So every program the daemon starts holds the key that decrypts `llm.toml`'s secrets. Removing the `set_var` alone is not enough (further finding 4): both launchers pass their own environment through (`apps/openalpaca/src/manager.rs:219`, `:224`; `apps/openalpaca-gui/src-tauri/src/lib.rs:343-344`), so a variable the owner exported in the launching shell — a documented setup, `scripts/eval/steer_routing/README.md:50` — would stay in the daemon's environment and still reach every child.

### The fix

1. `openalpaca_llm` gets a **process-local master key** — `static PROCESS_KEY: OnceLock<String>` in `key_encryption/mod.rs` — with a safe installer `KeyEncryptor::install_process_key(hex)` and an `unsafe` one, `install_process_key_and_clear_env(hex)`, that installs and then `remove_var`s `OPENALPACA_MASTER_KEY`. `from_env_opt` reads the installed key **first**, then the environment as the fallback the CLI and the tests keep using.
2. The daemon's preamble keeps `ensure_at` (the file wins over an inherited variable, exactly as today: `ensure_at` never reads the environment) and calls `install_process_key_and_clear_env` where it used to `set_var`.
3. One integration test in its own process proves it: started with the variable set, a spawned `env` shows no key after the install, and `from_env()` still works with the installed key (an encrypt/decrypt round trip, and the inherited key's ciphertext does **not** open).
4. The one manual sentence the fix falsifies (`docs/Daemon_Manual.md:183`, "The daemon exports it as `OPENALPACA_MASTER_KEY`") is corrected; the doc comments in `mod.rs` are rewritten with the code in Task 1.

**The verifier's reading of "the test fills no process-wide value".** The clause comes from further finding 5, whose concern is a `OnceLock` filled inside the daemon's **shared** test binary (every `#[cfg(test)]` module on parallel threads, `apps/openalpacad/src/test_util.rs:45-51` supplying the key through the very variable), where one test's install would change the key under services other tests already built. The test below is an integration file holding exactly one `#[test]` — its own process, shared with nothing — and it does install a key, because that is the only shape that drives the function the daemon actually calls; a clear-only helper the test called instead would add a third function the row does not ask for and prove nothing about the install. The orchestrator can overrule this reading (it is listed as unresolved).

**Choices the spec left open, each with its reason.**
- The shape is `OnceLock<String>` holding the 64 hex characters, as the task expected: that is what `ensure_at` returns and what `from_hex` takes, so the installed value and the environment value go through the one existing `from_hex`, and the hex is validated at install so the later read cannot fail.
- Installing the **same** key twice is `Ok(())`; a **different** key is `Err`: `OnceLock::set` would otherwise refuse a benign repeated call, while a second key in one process would silently split what encrypts from what decrypts.
- The variable's name becomes `pub const MASTER_KEY_ENV` so the test and `mod.rs` share one spelling; the two test helpers that already spell the literal (`apps/openalpacad/src/test_util.rs:47-60`, `apps/openalpaca/src/test_util.rs:20`, `:51`) are left as they are — minimal impact.
- The integration test is `#![cfg(unix)]`: it spawns `env`, CI's cargo jobs are `ubuntu-latest`, and the one `windows-latest` job runs only Pester (`.github/workflows/ci.yml`, CLAUDE.md → *Build & Development*).

### Repository rules that bite here (copy into the implementing session)

- In edition 2024 `std::env::set_var` and `std::env::remove_var` are `unsafe` and are called only in a single-threaded preamble inside `unsafe { }` with a `// SAFETY:` comment — in `main.rs` before the tokio runtime, and in the integration test on its single test thread. Inside an `unsafe fn`, the call still needs its own `unsafe { }` block (edition 2024's `unsafe_op_in_unsafe_fn` is warn by default) and the function a `# Safety` doc section (clippy's `missing_safety_doc` is warn by default and CI runs `cargo clippy --workspace --exclude openalpaca_gui --all-targets`).
- Test names are sentences in snake_case.
- `apps/openalpacad` is a binary with no lib target; its unit tests are `#[cfg(test)]` modules in one binary on parallel threads (`test_util.rs:7-13`). Nothing in that binary may ever call the installer.
- Never cut a string with `&s[..n]` (not needed here; nothing is cut).
- Tests are run per task as `cargo test -p <crate> <filter>`, one cargo process at a time. The full workspace run happens at the end of the stage.
- Commit messages use conventional prefixes and **end with the Co-Authored-By line the implementing session is given**; every `git commit -m` below is written without it — append it as the final `-m` line. `git status` shows `M .gitignore` and `?? tasks/deps-upgrade/` that are not this work: add the explicit paths named in each Step 5, never `-A`.

---

### Task F3.1: `openalpaca_llm` — a process-local master key that `from_env` reads first

**Files:** Create: `crates/openalpaca_llm/tests/master_key_leaves_the_environment.rs`; Modify: `crates/openalpaca_llm/src/keys/key_encryption/mod.rs:1-71` (the `use` lines, `PREFIX`, and `impl KeyEncryptor` from `from_env` through `from_env_opt`; nothing from `from_hex` at `:73` down changes); Test: `crates/openalpaca_llm/tests/master_key_leaves_the_environment.rs`

**Interfaces:**
- Consumes (all in `crates/openalpaca_llm/src/keys/key_encryption/mod.rs`, unchanged): `pub fn ensure_at(dir: &Path) -> Result<String, String>` (`:94`, returns the 64-char hex); `fn from_hex(hex_key: &str, source: &str) -> Result<Self, String>` (`:73`, trims, decodes, requires 32 bytes); `pub fn encrypt(&self, plaintext: &str) -> Result<String, String>` (`:176`); `pub fn decrypt(&self, encrypted: &str) -> Result<String, String>` (`:196`). Re-exported: `crates/openalpaca_llm/src/keys/mod.rs:7` `pub use key_encryption::*;`, so `openalpaca_llm::keys::key_encryption::KeyEncryptor` and `openalpaca_llm::keys::KeyEncryptor` both resolve today; the new const and functions ride the same glob (grep: `MASTER_KEY_ENV`, `PROCESS_KEY`, `install_process_key` appear nowhere in `crates/` or `apps/` today, so the glob re-export collides with nothing).
- Produces: `pub const MASTER_KEY_ENV: &str = "OPENALPACA_MASTER_KEY";` `pub fn KeyEncryptor::install_process_key(hex_key: &str) -> Result<(), String>`; `pub unsafe fn KeyEncryptor::install_process_key_and_clear_env(hex_key: &str) -> Result<(), String>`; `fn from_env_opt()` (private, unchanged signature) now reading the installed key first. Task F3.2 calls `install_process_key_and_clear_env`.

- [ ] **Step 1: Write the failing test**

Create `crates/openalpaca_llm/tests/master_key_leaves_the_environment.rs` with exactly this content (the crate has no `tests/` directory yet — create it; Cargo discovers `tests/*.rs` by default and `crates/openalpaca_llm/Cargo.toml` sets no `autotests = false`; the test needs only `std` and the crate, so its `[dev-dependencies]` (`tempfile`, `tokio`) need no change; `git check-ignore -q crates/openalpaca_llm/tests/master_key_leaves_the_environment.rs` exits 1 — the path is not ignored):

```rust
//! F3: the master key leaves the daemon's environment.
//!
//! The daemon's boot preamble takes the key from `state/.master_key`, installs
//! it as this crate's process-local master key and removes
//! `OPENALPACA_MASTER_KEY` from its environment, so no child it spawns — a
//! shell command, a skill script, an MCP server, a plugin, a CLI model backend
//! — receives the key. `KeyEncryptor::install_process_key_and_clear_env` is
//! that step; this test drives it the way the daemon does and watches a child.
//!
//! **Its own process, one test.** The installed key is a process-wide
//! `OnceLock`, and the environment is process-wide too: inside a crate's unit
//! test binary (parallel threads; the daemon's `test_util.rs` sets the very
//! variable) either would change what every other test reads. An integration
//! test file is its own binary, and this one holds exactly ONE `#[test]`, so
//! the test thread is the only thread touching the environment. A second test
//! in this file breaks that argument — put it in a file of its own.
#![cfg(unix)]

use openalpaca_llm::keys::key_encryption::{KeyEncryptor, MASTER_KEY_ENV};
use std::process::Command;

/// What a launching shell might have exported — the value the daemon used to
/// overwrite, and now removes.
const INHERITED_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
/// What `state/.master_key` holds — the key the daemon installs.
const FILE_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

/// Whether a child spawned now is handed the variable: `env` prints one
/// `NAME=value` line per variable it inherited. The command's environment is
/// deliberately not cleared — inheritance is the point.
fn a_child_inherits_the_master_key() -> bool {
    let out = Command::new("env").output().expect("spawn `env`");
    assert!(out.status.success(), "`env` exited {:?}", out.status.code());
    let prefix = format!("{MASTER_KEY_ENV}=");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line.starts_with(&prefix))
}

#[test]
fn a_child_spawned_after_the_install_sees_no_master_key_and_the_installed_key_still_decrypts() {
    // The premise: the daemon was started by a shell that exported the key.
    // SAFETY: this file's one test runs on the only thread that touches the
    // environment (libtest's main thread only waits on it).
    unsafe { std::env::set_var(MASTER_KEY_ENV, INHERITED_KEY) };
    assert!(
        a_child_inherits_the_master_key(),
        "premise: before the install a child inherits {MASTER_KEY_ENV}"
    );

    // Before any install the environment is what `from_env` reads — the
    // CLI's and the test harnesses' path, which must keep working.
    let inherited = KeyEncryptor::from_env().expect("the environment is the fallback");
    let sealed_by_inherited = inherited.encrypt("sk-from-the-shell").unwrap();

    // Review focus — a malformed key is refused before anything is installed,
    // and the environment fallback still answers afterwards.
    assert!(
        KeyEncryptor::install_process_key("not-hex").is_err(),
        "a key that is not 64 hex characters is refused"
    );
    KeyEncryptor::from_env().expect("a refused install leaves the fallback in place");

    // The daemon's step: the file's key in, the variable out.
    // SAFETY: as above — one thread.
    unsafe { KeyEncryptor::install_process_key_and_clear_env(FILE_KEY) }
        .expect("installing the file's key");

    assert!(
        std::env::var_os(MASTER_KEY_ENV).is_none(),
        "{MASTER_KEY_ENV} must be gone from this process's environment"
    );
    assert!(
        !a_child_inherits_the_master_key(),
        "a child spawned after the install must not see {MASTER_KEY_ENV}"
    );

    // Every reader built on `from_env` now gets the installed key, with no
    // variable anywhere...
    let installed = KeyEncryptor::from_env().expect("the installed key answers with no variable");
    let sealed = installed.encrypt("sk-secret").unwrap();
    assert_eq!(
        KeyEncryptor::from_env().unwrap().decrypt(&sealed).unwrap(),
        "sk-secret",
        "a second from_env reads the same installed key"
    );
    // ...and it is the FILE's key, not the inherited one: what the shell's
    // key sealed does not open. `ensure_at` never reads the environment, so
    // the daemon's file wins over an inherited variable, as it always has.
    assert!(
        installed.decrypt(&sealed_by_inherited).is_err(),
        "the inherited key must not be the one installed"
    );

    // The same key again is a no-op; a different one is refused.
    KeyEncryptor::install_process_key(FILE_KEY).expect("re-installing the same key is fine");
    assert!(
        KeyEncryptor::install_process_key(INHERITED_KEY).is_err(),
        "a second, different master key in one process is refused"
    );
}
```

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test -p openalpaca_llm --test master_key_leaves_the_environment`

Expected: FAIL to compile, every error in the new test file and none in the crate: an unresolved import of `MASTER_KEY_ENV` from `openalpaca_llm::keys::key_encryption` (E0432, the `use` line), and the associated functions `install_process_key_and_clear_env` and `install_process_key` not found for `KeyEncryptor` (E0599, one per call site). The codes are what rustc gives those two shapes; neither the count of diagnostics nor the surrounding wording is pinned here. The claim that matters is that the test does not compile before Step 3. If you want to watch the real assertion go red rather than a missing symbol: add only the `pub const MASTER_KEY_ENV`, `install_process_key` with the body `Ok(())`, and `install_process_key_and_clear_env` with a body that does **only** the `unsafe { std::env::remove_var(MASTER_KEY_ENV) }; Ok(())` — no `OnceLock` yet — and run again. The premise, the fallback read, the removal and the child check all pass, and the test panics at `the installed key answers with no variable` with today's `from_env` error text (`OPENALPACA_MASTER_KEY is not set: the process must load the master key first …`) — the proof that `from_env_opt` does not yet read an installed key. (A stub whose unsafe half does nothing fails earlier, at `must be gone from this process's environment`, which proves less.) Then finish Step 3.

- [ ] **Step 3: Write the minimal implementation**

In `crates/openalpaca_llm/src/keys/key_encryption/mod.rs`, replace lines 1–71 — from the module doc comment through the closing brace of `from_env_opt` — with the following. Everything from `fn from_hex` (`:73`) to the end of the file stays byte for byte.

```rust
//! AES-256-GCM encryption for API key secrets at rest.

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit, OsRng, rand_core::RngCore},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::path::Path;
use std::sync::OnceLock;

const PREFIX: &str = "aes256:";

/// The environment variable a process may carry its master key in — the
/// fallback [`KeyEncryptor::from_env`] reads when no key was installed with
/// [`KeyEncryptor::install_process_key`]. The CLI and the test harnesses use
/// it; the daemon does not (F3: it installs the key and removes the variable,
/// so no process it spawns receives the key).
pub const MASTER_KEY_ENV: &str = "OPENALPACA_MASTER_KEY";

/// The master key this process was handed, as the 64 hex characters
/// [`KeyEncryptor::ensure_at`] returns — validated before it is stored, so a
/// read of it cannot fail. Filled once, by
/// [`KeyEncryptor::install_process_key`]; never by a unit test, since every
/// test in a binary shares it.
static PROCESS_KEY: OnceLock<String> = OnceLock::new();

/// Encrypts and decrypts API key secrets using AES-256-GCM.
pub struct KeyEncryptor {
    key: Key<Aes256Gcm>,
}

impl KeyEncryptor {
    /// The encryptor for this process's master key: the one installed with
    /// [`Self::install_process_key`], else the one in [`MASTER_KEY_ENV`].
    ///
    /// This crate sits below `openalpaca_storage` in the dependency graph, so it
    /// never resolves a path of its own: a process that wants a key file says
    /// which directory holds it ([`Self::load_or_generate_at`], with
    /// `store::master_key_dir()`). The daemon does that once in its boot
    /// preamble and installs the key here — in process memory, never in its
    /// environment, so no program it spawns receives it (F3) — and everything
    /// downstream of it — router construction, keychain migration, the settings
    /// service — reads the same key without knowing where it lives. The
    /// environment read is the fallback the CLI and the test harnesses use.
    pub fn from_env() -> Result<Self, String> {
        Self::from_env_opt()?.ok_or_else(|| {
            format!(
                "no master key: the process must install one first \
                 (KeyEncryptor::install_process_key, as the daemon's boot preamble does) \
                 or carry it in {MASTER_KEY_ENV}"
            )
        })
    }

    /// Hands this process its master key: what [`Self::from_env`] and every
    /// reader built on it use from now on, before any environment variable.
    ///
    /// `hex_key` is validated first (64 hex characters, 32 bytes), so a later
    /// read cannot fail. Installing the **same** key again is a no-op; a
    /// **different** one is refused — one process with two master keys would
    /// silently split what it encrypts from what it can decrypt.
    pub fn install_process_key(hex_key: &str) -> Result<(), String> {
        Self::from_hex(hex_key, "the master key to install")?;
        match PROCESS_KEY.set(hex_key.trim().to_string()) {
            Ok(()) => Ok(()),
            Err(rejected) if PROCESS_KEY.get() == Some(&rejected) => Ok(()),
            Err(_) => {
                Err("a different master key is already installed in this process".to_string())
            }
        }
    }

    /// [`Self::install_process_key`], then [`MASTER_KEY_ENV`] removed from the
    /// process environment — inherited from the launching shell or not — so no
    /// child this process spawns receives the key (F3). The daemon's boot
    /// preamble calls this once; so does the integration test that proves it
    /// (`tests/master_key_leaves_the_environment.rs`).
    ///
    /// # Safety
    ///
    /// Calls [`std::env::remove_var`]: the caller must be the only thread that
    /// can be reading or writing the environment — in the daemon, the
    /// single-threaded preamble before the tokio runtime starts.
    pub unsafe fn install_process_key_and_clear_env(hex_key: &str) -> Result<(), String> {
        Self::install_process_key(hex_key)?;
        // SAFETY: the caller guarantees no other thread touches the environment
        // (this function's contract, above).
        unsafe { std::env::remove_var(MASTER_KEY_ENV) };
        Ok(())
    }

    /// The encryptor for the master key kept in `dir`, generating one if `dir`
    /// has none. A key this process installed, or one in [`MASTER_KEY_ENV`],
    /// still wins when there is one (`Self::from_env_opt`'s order). The daemon
    /// and the CLI agree because both name the same file, `state/.master_key`
    /// — the daemon no longer exports it (F3).
    pub fn load_or_generate_at(dir: &Path) -> Result<Self, String> {
        if let Some(encryptor) = Self::from_env_opt()? {
            return Ok(encryptor);
        }
        Self::from_hex(&Self::ensure_at(dir)?, "generated master key")
    }

    /// The encryptor for a master key that **already exists** in `dir` — the
    /// read-only twin of [`Self::load_or_generate_at`], for paths that only
    /// *decrypt*. `Ok(None)` says there is no key yet.
    ///
    /// A decrypt path must not mint one: a freshly generated key cannot decrypt
    /// anything that was encrypted with the old one, so the caller's outcome is
    /// the same either way — and the key file is durable state in `state/`,
    /// which a read has no business creating. A key this process installed, or
    /// one in [`MASTER_KEY_ENV`], still wins when there is one, as in
    /// [`Self::load_or_generate_at`].
    pub fn load_at(dir: &Path) -> Result<Option<Self>, String> {
        if let Some(encryptor) = Self::from_env_opt()? {
            return Ok(Some(encryptor));
        }
        let key_path = dir.join(".master_key");
        match std::fs::read_to_string(&key_path) {
            Ok(contents) => Self::from_hex(contents.trim(), "master key").map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("Failed to read {}: {e}", key_path.display())),
        }
    }

    /// The installed key first, then the environment; `Ok(None)` when the
    /// process has neither.
    fn from_env_opt() -> Result<Option<Self>, String> {
        if let Some(hex_key) = PROCESS_KEY.get() {
            return Self::from_hex(hex_key, "the installed master key").map(Some);
        }
        match std::env::var(MASTER_KEY_ENV) {
            Ok(hex_key) => Self::from_hex(&hex_key, MASTER_KEY_ENV).map(Some),
            Err(_) => Ok(None),
        }
    }
```

What changed, line by line against today's file: `use std::sync::OnceLock;` is new (`:9`); `MASTER_KEY_ENV` and `PROCESS_KEY` are new; `from_env`'s doc (today `:18-26`: "The encryptor for the master key in `OPENALPACA_MASTER_KEY`… The daemon does that once in its boot preamble and exports the key here") and its error string (today `:29-30`: `"OPENALPACA_MASTER_KEY is not set: the process must load the master key first (KeyEncryptor::load_or_generate_at(store::master_key_dir()?))"` — grep over `crates/` and `apps/` finds no test pinning it) are rewritten; `install_process_key` and `install_process_key_and_clear_env` are new; the sentence "`OPENALPACA_MASTER_KEY` still wins when it is set, so a daemon and a CLI in the same environment agree." in `load_or_generate_at`'s doc (today `:36-37`) and `load_at`'s (today `:52-53`) is replaced as shown; the two function bodies are unchanged; `from_env_opt` gains the `PROCESS_KEY.get()` branch and spells the variable through the const. `Err(rejected) if PROCESS_KEY.get() == Some(&rejected)` compares `Option<&String>` with `Option<&String>` — `OnceLock::set` hands the rejected value back, and after a failed `set` the lock is filled, so `get()` is `Some`.

- [ ] **Step 4: Run the test to see it pass**

Run: `cargo test -p openalpaca_llm --test master_key_leaves_the_environment`

Expected: PASS —

```
running 1 test
test a_child_spawned_after_the_install_sees_no_master_key_and_the_installed_key_still_decrypts ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Then run: `cargo test -p openalpaca_llm key_encryption`

Expected: the 13 tests of `crates/openalpaca_llm/src/keys/key_encryption/tests.rs` still pass — `test result: ok. 13 passed` in the lib target's run (the filter matches the module path `keys::key_encryption::tests::*` and, by grep, no other module path in the crate contains `key_encryption`); the integration test binary then reports `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out`, its one test's name not containing `key_encryption`. Two of them, `load_or_generate_at_uses_the_directory_it_is_given` and `load_at_never_mints_a_master_key`, rely on `from_env_opt` returning `Ok(None)`; it does, because nothing in the unit test binary installs a key and `OPENALPACA_MASTER_KEY` is unset in the shell that runs the tests (if it is exported in your shell, those two tests behaved the same way before this change — not a regression of this task).

- [ ] **Step 5: Commit**

```
git add crates/openalpaca_llm/src/keys/key_encryption/mod.rs crates/openalpaca_llm/tests/master_key_leaves_the_environment.rs
git commit -m "fix(llm): the master key is a process-local value from_env reads before the environment (F3)" -m "KeyEncryptor::install_process_key hands a process its master key (validated hex in a OnceLock; the same key again is a no-op, a different one is refused) and install_process_key_and_clear_env removes OPENALPACA_MASTER_KEY from the environment after installing — the step the daemon's preamble takes next, so no child it spawns receives the key. from_env_opt reads the installed key first and the environment as the fallback the CLI and the tests use; MASTER_KEY_ENV names the variable once.

Verified: cargo test -p openalpaca_llm --test master_key_leaves_the_environment (1 passed, in its own process: a spawned env shows the key before the install and not after, and the installed key decrypts while the inherited one's ciphertext does not); cargo test -p openalpaca_llm key_encryption (13 passed)."
```

---

### Task F3.2: the daemon installs the key and clears its environment

**Files:** Modify: `apps/openalpacad/src/main.rs:51` (one `use`), `:128-130` (comment), `:145-156` (the `Ok` arm of the `ensure_at` match), `:173` (comment); `apps/openalpacad/src/test_util.rs:43-44` (doc comment only); Test: none of its own — see Step 1.

**Interfaces:**
- Consumes: `pub fn KeyEncryptor::ensure_at(dir: &Path) -> Result<String, String>` (`crates/openalpaca_llm/src/keys/key_encryption/mod.rs:94`, unchanged — reads `dir/.master_key` or generates it; it never reads the environment, which is why the file wins over an inherited variable); `pub unsafe fn KeyEncryptor::install_process_key_and_clear_env(hex_key: &str) -> Result<(), String>` (Task F3.1); `store::master_key_dir()` (`main.rs:143`, unchanged); `error!`/`info!` from `tracing` (`main.rs:58`).
- Produces: nothing new. After this task the daemon's environment carries no `OPENALPACA_MASTER_KEY`, and `KeyEncryptor::from_env()` inside the daemon answers from the installed key.

- [ ] **Step 1: The test for this step is Task F3.1's integration test; why none lives here**

No new test in this task, stated plainly rather than invented: (a) a test of the preamble inside the daemon's test binary would have to call the installer, which fills the `OnceLock` process-wide and changes the key under every `LlmSettingsService` another test built through `set_with_master_key` (further finding 5) — `hot_reload/llm_dedup_tests.rs:57` and `routes/settings/provider_enabled_tests.rs:74`; (b) a spawned-daemon test in `apps/openalpacad/tests/` (the isolation pattern of `tests/daemon_args.rs:34-46` — `CARGO_BIN_EXE_openalpacad` with `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` in a tempdir) cannot observe the removal: Linux's `/proc/<pid>/environ` and macOS's `ps -E` read the environment block as it was at `exec`, not the effect of `unsetenv`, so only a child the daemon itself spawns shows it. Two children exist: a tool call (a model's `shell_execute`), which is exactly the stage's live gate (design.md:586: from the rebuilt app, in an isolated store, `env | grep OPENALPACA` prints no master key), and a stdio MCP server declared and enabled in the isolated store's `mcp.toml`, which the supervisor's first `reconcile_all` spawns at boot with no model (`apps/openalpacad/src/services/mcp.rs:59` calls it; the method is `apps/openalpacad/src/managers/mcp.rs:2185`; the spawn is `crates/openalpaca_mcp/src/transport/stdio.rs:64-71`) — a possible `cargo test` shape, not built here because the row asks for one test and Task F3.1 holds it. The function the daemon calls is the one Task F3.1's test drives with the same arguments shape, so the code path is covered; what this task adds is the wiring, verified by compilation and by the daemon tests that depend on the fallback still working (Step 4).

- [ ] **Step 2: Record the baseline**

Run: `cargo test -p openalpacad llm_dedup_tests`

Expected: PASS, `7 passed` (the `hot_reload::llm_dedup_tests` module; the filter matches that module path and nothing else in the binary — `tests/daemon_args.rs`'s tests are filtered out, `0 failed`). This is the fallback path — `set_with_master_key` → `from_env` — and it must read the same after Step 3.

- [ ] **Step 3: Write the minimal implementation**

In `apps/openalpacad/src/main.rs`:

(a) After line 50 (`};` closing the `use openalpaca_core::{…}` block) and before line 51 (`use openalpaca_storage::{Database, discovery, store};`), insert one line so the group stays alphabetical (`openalpaca_core`, `openalpaca_llm`, `openalpaca_storage`, `openalpaca_wake`):

```rust
use openalpaca_llm::keys::key_encryption::KeyEncryptor;
```

(b) Replace lines 128–130, today

```rust
    // Resolve config dir and set master key BEFORE spawning any threads.
    // std::env::set_var is unsafe in multi-threaded contexts (Rust 2024 edition),
    // so we do it here in the single-threaded preamble.
```

with

```rust
    // Resolve config dir and take the master key BEFORE spawning any threads.
    // The key step removes OPENALPACA_MASTER_KEY from the environment (F3), and
    // std::env::remove_var is unsafe in multi-threaded contexts (Rust 2024
    // edition), so it happens here in the single-threaded preamble.
```

(c) Replace lines 145–156, today

```rust
    // D6+D7: ensure_at is race-safe; on failure, fail fast.
    match openalpaca_llm::keys::key_encryption::KeyEncryptor::ensure_at(&master_key_dir) {
        Ok(hex_key) => {
            // SAFETY: No other threads exist yet — tokio runtime has not started.
            unsafe {
                std::env::set_var("OPENALPACA_MASTER_KEY", &hex_key);
            }
            info!(
                "Master key loaded from {}",
                master_key_dir.join(".master_key").display()
            );
        }
```

with

```rust
    // D6+D7: ensure_at is race-safe; on failure, fail fast.
    // F3: the key stays in this process. It is installed as openalpaca_llm's
    // process-local master key — what `KeyEncryptor::from_env` reads first —
    // and `OPENALPACA_MASTER_KEY` is removed from the environment, inherited
    // or not, so no child this daemon spawns (a shell command, a skill script,
    // an MCP server, a plugin, a CLI model backend) receives it. The file wins
    // over an inherited variable, as it always has: `ensure_at` never reads
    // the environment.
    match KeyEncryptor::ensure_at(&master_key_dir) {
        Ok(hex_key) => {
            // SAFETY: No other threads exist yet — the tokio runtime has not
            // started and no signal handler is installed — so nothing else can
            // be reading or writing the environment.
            if let Err(e) = unsafe { KeyEncryptor::install_process_key_and_clear_env(&hex_key) } {
                error!("FATAL: Cannot install the master key: {e}");
                std::process::exit(1);
            }
            info!(
                "Master key loaded from {} and kept in process memory (not exported)",
                master_key_dir.join(".master_key").display()
            );
        }
```

The `Err(e)` arm (today `:157-163`: `error!("FATAL: Cannot ensure master key at {}: {e}", …); std::process::exit(1);`) and the closing brace stay as they are. `install_process_key_and_clear_env` cannot fail here in practice — `ensure_at` already validated the hex and this is the process's first install — but a fatal exit is the same answer the `ensure_at` failure gets, and silently continuing without a key would leave every `from_env` reader failing later.

(d) Replace line 173, today

```rust
    // Start the tokio runtime AFTER env vars and signal handlers are set.
```

with

```rust
    // Start the tokio runtime AFTER the master key is installed (and the
    // environment cleaned of it) and the signal handlers are set.
```

In `apps/openalpacad/src/test_util.rs`, replace lines 43–44, today

```rust
    /// As [`Self::set`], plus a throwaway `OPENALPACA_MASTER_KEY` for the tests
    /// that build an `LlmSettingsService`.
```

with

```rust
    /// As [`Self::set`], plus a throwaway `OPENALPACA_MASTER_KEY` for the tests
    /// that build an `LlmSettingsService`. The variable is what
    /// `KeyEncryptor::from_env` falls back to: nothing in this binary calls
    /// `KeyEncryptor::install_process_key` (the daemon's preamble does, in
    /// `main`), and nothing may — the installed key is process-wide and would
    /// change the key under every service another test already built (F3).
```

Run `cargo fmt -p openalpacad` after editing: the `if let Err(e) = unsafe { … }` line is at rustfmt's width limit and it may rewrap it.

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo check -p openalpacad`

Expected: `Finished` with no warnings from `main.rs` (an unused-import warning here would mean the `use` in (a) was added but the match in (c) still spells the full path — do both).

Run: `cargo test -p openalpacad llm_dedup_tests`

Expected: PASS, `7 passed` — unchanged from Step 2: the daemon's test binary never installs a key, so `set_with_master_key`'s variable is still what `from_env` reads.

Run: `cargo test -p openalpacad provider_enabled_tests`

Expected: PASS, `11 passed` (the `routes::settings::provider_enabled_tests` module, `apps/openalpacad/src/routes/settings/provider_enabled_tests.rs`, which builds its `LlmSettingsService` at `:82` through the same fallback).

Neighbouring tests that must still pass at the stage's full run: `apps/openalpacad/tests/daemon_args.rs` (the `--help`/`--version` runs return before the preamble reaches the key — `main.rs:79-94` — so this change cannot touch them).

- [ ] **Step 5: Commit**

```
git add apps/openalpacad/src/main.rs apps/openalpacad/src/test_util.rs
git commit -m "fix(daemon): the master key leaves the daemon's environment — installed in process, OPENALPACA_MASTER_KEY removed (F3)" -m "The preamble still takes the key from state/.master_key with ensure_at (the file wins over an inherited variable), then calls KeyEncryptor::install_process_key_and_clear_env where it used to set_var: the key becomes openalpaca_llm's process-local value and OPENALPACA_MASTER_KEY is removed from the environment, inherited or not, so no shell command, skill script, MCP server, plugin or CLI model backend the daemon spawns receives it. The daemon's test binary never installs a key, and test_util.rs now says so.

Verified: cargo check -p openalpacad clean; cargo test -p openalpacad llm_dedup_tests (7 passed) and provider_enabled_tests (11 passed) — the environment fallback the daemon's tests build on is unchanged. The live proof — a shell_execute from the rebuilt app in an isolated store showing no master key — is the Stage 0 gate."
```

---

### Task F3.3: the manual sentence the fix falsifies

**Files:** Modify: `docs/Daemon_Manual.md:183`; Test: none (prose). The doc comments the drafting task named, `crates/openalpaca_llm/src/keys/key_encryption/mod.rs:36-41` (with their twin at `:52-53` and the `from_env` doc at `:18-26`), are in the same file as Task F3.1's code and were rewritten there — see Task F3.1 Step 3's "What changed" paragraph for the before/after. Two other documents that name the variable stay as they are, because the fix does not falsify them: `docs/Daemon_Manual.md:111` ("Resolve config directory, seed missing default configs, and ensure master key.") is still true, and `scripts/eval/steer_routing/README.md:50-51` ("A `secret_encrypted` value decrypts only when `OPENALPACA_MASTER_KEY` is exported.") is still exactly true for the harness — `crates/openalpaca_core/tests/steer_routing_eval.rs:204` builds its router with `openalpaca_llm::build_router` in its own process, which was never a child of the daemon and where nothing installs a key, so the environment is its only source, before and after F3.

**Interfaces:** Consumes: the behaviour Tasks F3.1–2 land. Produces: nothing.

- [ ] **Step 1: What it says today**

`docs/Daemon_Manual.md:183` today:

> - The AES-256-GCM master key lives at `~/.openalpaca/state/.master_key` (`store::master_key_dir()`). The daemon exports it as `OPENALPACA_MASTER_KEY` for its own process; startup fails hard if the key cannot be ensured.

— the second sentence becomes **false** after Task F3.2, and a manual that states the export the fix removes cannot stand (the facts file lists it, A4: "Docs that name it: docs/Daemon_Manual.md:183, …").

- [ ] **Step 2: Confirm the stale sentence is where the facts say**

Run: `grep -n "exports it as" docs/Daemon_Manual.md`

Expected: exactly one hit — `183:- The AES-256-GCM master key lives at … The daemon exports it as `OPENALPACA_MASTER_KEY` for its own process; …` (the `grep` is the "failing test" of a prose task: it finds the sentence that must change).

- [ ] **Step 3: Write the replacement**

`docs/Daemon_Manual.md:183` →

```
- The AES-256-GCM master key lives at `~/.openalpaca/state/.master_key` (`store::master_key_dir()`). The daemon reads it at boot and keeps it in its own process memory: it does not export `OPENALPACA_MASTER_KEY`, and removes one it inherited from the shell or launcher that started it, so no program it spawns — a shell command, a skill script, an MCP server, a plugin, a CLI model backend — receives the key. Startup fails hard if the key cannot be ensured. The variable is read only by the CLI and the test harnesses, where it overrides the file.
```

- [ ] **Step 4: Confirm**

Run: `grep -rn "exports it as\|exports the key" docs crates apps scripts --include='*.md' --include='*.rs'`

Expected: no output — no document or doc comment still says the daemon exports the key (Task F3.1 removed the one in `mod.rs:24`, this task the manual's; the README's `is exported.` at `:51` matches neither phrase and is true).

Run: `git diff --stat -- docs`

Expected: `docs/Daemon_Manual.md | 2 +-` and nothing else under `docs` (the tree already carries an unrelated `M .gitignore`, which is why the stat is scoped to `docs`).

- [ ] **Step 5: Commit**

```
git add docs/Daemon_Manual.md
git commit -m "docs: the daemon no longer exports OPENALPACA_MASTER_KEY (F3)" -m "The Daemon Manual's secrets section says the key is read from state/.master_key, kept in process memory and removed from the environment, inherited or not, so no spawned program receives it, and that the variable is read only by the CLI and the test harnesses. The sentence it replaces stated the export the fix removes.

Verified: grep for 'exports it as' / 'exports the key' over docs, crates, apps and scripts finds nothing."
```

---

### Reader audit (deliverable 4): every reader of the key still works

Every reader in the tree reaches the key through `from_env_opt` (facts A4; I re-read each site). After Task F3.1 that function answers the installed key first, else the environment. Per site:

| Reader | Via | Process | Before | After | Change |
|---|---|---|---|---|---|
| `crates/openalpaca_llm/src/config/settings_service.rs:125`, `:142` (`LlmSettingsService::new`, `new_with_secret_store`) | `from_env()` | daemon (`apps/openalpacad/src/services/llm.rs:143`) | env var set from the file | installed key — the same bytes `set_var` used to carry | none |
| `crates/openalpaca_llm/src/config/key_pool_builder.rs:97` (hot reload: `build_key_pool_from_provider_config`, `:39`, re-exported at `settings_service.rs:1291`, → `build_api_keys_from_provider_config`, `:61`) | `from_env()` | daemon (`apps/openalpacad/src/hot_reload.rs:496`) | env var | installed key | none |
| `crates/openalpaca_llm/src/config/llm_config/router_builder.rs:112` (router build) | `from_env()` | daemon; **also** the two live evals, `crates/openalpaca_core/tests/steer_routing_eval.rs:204`, `tests/memory_eval.rs:5944`, and the daemon test modules `hot_reload/llm_dedup_tests.rs:62`, `routes/settings/provider_enabled_tests.rs:79` | env var | daemon: installed key; evals and tests: env var (nothing installs in those processes — the fallback) | none |
| `crates/openalpaca_llm/src/config/llm_config/migration.rs:25`, `:64`, `:151` (keychain migration both ways) | `from_env()` | daemon (`apps/openalpacad/src/services/mod.rs:168`, `services/llm.rs:69`) | env var | installed key | none |
| `apps/openalpaca/src/commands/ai_config_helpers.rs:22` (`encryptor()`) | `load_or_generate_at(store::master_key_dir())` | CLI | env var if the user exported one, else the file | the same: nothing installs in a CLI process | none |
| `crates/openalpaca_plugins/src/manager.rs:2781` (`encryptor()`, the write path) | `load_or_generate_at` | daemon | env var (set from the file) | installed key (from the same file) | none |
| `crates/openalpaca_plugins/src/manager.rs:2795` (`existing_encryptor()`, the resolve path) | `load_at` | daemon | env var | installed key | none |

What *does* change, and is the point: a process the daemon spawns (A5's six spawn sites) no longer finds `OPENALPACA_MASTER_KEY` in its environment. No child in the tree read it (A4), so nothing downstream breaks. The daemon's test binary (`apps/openalpacad/src/test_util.rs:45-51`) and the CLI's (`apps/openalpaca/src/test_util.rs:16-21`, `:51`) keep supplying or clearing the variable exactly as today, and neither ever installs a key.

Documents that name the variable, after the three tasks: `crates/openalpaca_llm/src/keys/key_encryption/mod.rs` (rewritten, Task F3.1), `docs/Daemon_Manual.md:183` (Task F3.3), `apps/openalpacad/src/test_util.rs:43-48` (Task F3.2); unchanged because still true: `docs/Daemon_Manual.md:111` (the startup step "ensure master key"), `scripts/eval/steer_routing/README.md:50-51` (the harness's own process, where the variable is the only source), `apps/openalpaca/src/test_util.rs:19-20` (its comment "The CLI never sets this; an inherited value would mask the bug under test."), CLAUDE.md:76 (says only that `EnvSandbox` clears the variable).

### Stage-end gates this touches

`cargo clippy --workspace --exclude openalpaca_gui --all-targets` compiles the new integration test: the `pub unsafe fn` carries its `# Safety` section and its inner `unsafe {}` block, and the test's two `unsafe` blocks carry `// SAFETY:` comments. `python3 scripts/gen_api_docs.py --check` is unaffected (no route, migration or event changed). The live gate (design.md:586): on the rebuilt app in an isolated store, a `shell_execute` of `env | grep OPENALPACA` shows `OPENALPACA_HOME_STORE`/`OPENALPACA_CONFIG_DIR` as the launcher set them and **no** `OPENALPACA_MASTER_KEY` — start the daemon from a shell that has `export OPENALPACA_MASTER_KEY=…` set to a throwaway 64-hex value, so the inherited case is the one checked.

### Open points

1. **The documents Task F3.3 touches.** The spec row names no document, and its "Where" column is `main.rs:146-151` and `mod.rs:27-71`. Task F3.3 changes one sentence, `docs/Daemon_Manual.md:183`, because Task F3.2 makes it false; the verifier removed the draft's two other edits — the startup step at `:111` and `scripts/eval/steer_routing/README.md:50-51` — because both stay true after the fix (Task F3.3's **Files** says why). The drafting task had named the README; the orchestrator can restore that edit if a how-to for the harness is wanted, but it is not F3's.
2. **No daemon-level automated test of the preamble.** Stated in Task F3.2 Step 1: a unit test in the daemon binary would fill the `OnceLock` process-wide (further finding 5), and a spawned-daemon integration test cannot observe `remove_var` through `/proc/<pid>/environ` or `ps -E`, which read the exec-time block. The code path is covered by Task F3.1's integration test (the same function the daemon calls); the wiring by compilation, the two daemon test modules on the fallback, and the stage's live gate. A spawned daemon that itself spawns a child would show it: through a tool call, which needs a model and is the live gate, or through a stdio MCP server declared in the isolated store's `mcp.toml`, which the supervisor's first `reconcile_all` spawns at boot with no model (`apps/openalpacad/src/services/mcp.rs:59`, `managers/mcp.rs:2185`) — a possible `cargo test` shape, not built because the row asks for one test.
3. **`cargo test -p openalpaca_llm key_encryption` count.** 13 `#[test]` functions in `crates/openalpaca_llm/src/keys/key_encryption/tests.rs` (verifier: `grep -c '#\[test\]'` gives 13, no `#[tokio::test]`), and the filter matches that module alone: `keys/mod.rs:2 pub mod key_encryption;` is the only other place the word names a module path, no test function's name contains it, and `mod.rs` has no doc code block, so no doctest name carries the file's path. The implementer's Step 4 run is the check.
4. **Compile errors in Task F3.1 Step 2.** Settled by not depending on rustc's wording: Step 2 states the three errors by shape and code (E0432 for the unresolved import, E0599 for the two missing associated functions) and pins nothing else.
5. **CLAUDE.md.** Its "Keep this file current" rule: no sentence there describes the export, so nothing becomes false; the stage's documentation pass may still want one clause under *Storage* or *Gotchas* saying the daemon keeps the master key in process and out of its children's environment (F3). Not part of these three tasks.
6. **`env` on the CI runner.** Settled: `env` is a POSIX utility, present at `/usr/bin/env` on every Linux and macOS system, and `cargo test`'s `PATH` reaches it; the test is `#![cfg(unix)]`, and the one `windows-latest` job compiles no Rust at all (`.github/workflows/ci.yml:66-77`, Pester only). Nothing to hedge.

---

## Fix F4 — a follow-up and a steering message keep the turn's project

Plan draft for Stage 0 fix F4 (`spec-stage0.md` row F4; facts A6, D1, further findings 1 and 6 in `facts-stage0.md`). Line numbers were read at `d569885b` (the files below are unchanged since `032f6ae2`, the facts file's reference); they may drift by a few lines.

### What is wrong, in one paragraph

`lane_followups.workspace_path` and `SteeringMsg.workspace_path` are supposed to carry the project a re-entered turn runs in. Three writers fill them from `ToolContext.workspace_path` — the path **as the client sent it**, unresolved (`crates/openalpaca_core/src/tools/registry/mod.rs:49-52`, `:107-110`) — or from the handler's raw `workspace_path` argument: `QueueFollowupTool::execute_with_context` (`crates/openalpaca_core/src/runner/lead_agent/tools.rs:1491`), `SteerWorkflowTool` (`crates/openalpaca_core/src/tools/builtins/steer_workflow.rs:117`) and the deterministic `/steer` prefix (`crates/openalpaca_core/src/orchestrator/task_ops.rs:397`, fed by `orchestrator/handlers.rs:211`). The lead's `ToolContext` never carries `workspace_path` at all (`crates/openalpaca_core/src/runner/lead_agent/mod.rs:278-292`: "No request, principal, scope or workspace path is threaded to the lead"), so a follow-up a workflow's lead queues stores **NULL**, and the follow-up runner (`apps/openalpacad/src/followup.rs:44`) re-enters it with no project — the one consequence a user can see today. Every one of these contexts already carries the resolved root, `ToolContext.request_workspace_root` (`registry/mod.rs:76-87`; set by the main loop at `orchestrator/query_handler/simple_query_handler.rs:150`, by the lead at `runner/lead_agent/mod.rs:285`), and the `/steer` handler has the turn's `MemoryScopeContext` in scope (`handlers.rs:86`). The fix is to store that root everywhere, and nothing else.

### What stays the same at run time (state this in the commit messages, not as a hope)

- **A path under a marker.** `for_request(Some(<root>))` resolves a root to itself — the root directory carries the `.openalpaca`/`.git` marker (`memory/workspace.rs:64-77`, `memory/scope_context.rs:159-167`) — so the gateway (`gateway/router/mod.rs:379-383`) and the handler (`handlers.rs:86`) see the same project from a stored root as from the stored subdirectory.
- **A path under no marker, or one that resolves to the home store.** Today the raw path is stored and `for_request(Some(raw))` yields `request_workspace_root: None`; after F4 the row holds NULL and `for_request(None)` yields `request_workspace_root: None` too. Two differences, both inert: (1) `workspace_id` — NULL takes the CWD-derived branch (`scope_context.rs:168-176`), which folds to `None` when the daemon's working directory is the home store (both launchers start it there: `cmd.current_dir(runtime_dir)` at `apps/openalpaca/src/manager.rs:218`, `.current_dir(&app_dir)` at `apps/openalpaca-gui/src-tauri/src/lib.rs:342`; facts D10) and would name the checkout on a daemon run from a repository — and **no memory path reads `workspace_id`** (MD-5; `registry/mod.rs:33-43`); (2) the claim at `orchestrator/dispatcher/lead_agent.rs:1074-1077` turns a stored path into `Scope::Workspace { path }` and NULL into `Scope::Global`, and the only production match on the variants is `security/policy.rs:73`, an `if let Scope::Global` whose body does nothing. So "identical" is exact for `request_workspace_root` and for the gate, and true in every shipped launch for `workspace_id`.
- **Steering rows and records.** `SteeringMsg.workspace_path` reaches disk two ways — the `steering` session-log record (`runner/steering.rs:143`) and an `unprocessed_steering` row when the message is left undelivered (`dispatcher/lead_agent.rs:761-768`, `runner/steering.rs:162-169`) or recovered at boot (`apps/openalpacad/src/bootstrap/interrupted.rs:130-170`). Nothing reads an `unprocessed_steering` row's `workspace_path`: `FollowupRepository::claim_next` never claims that kind (`crates/openalpaca_storage/src/repository/followup/mod.rs:321`, doc at `:3-6`), and `take_unprocessed_steering_block` renders `row.content` only (`orchestrator/query_handler/unprocessed_steering.rs:96`). So Tasks F4.2 and 3 fix what the row and the record **record**; the only behaviour a test can observe is the drained `SteeringMsg`. A `steering` record written before this fix keeps its raw path: `session_log/recovery.rs:141-144` copies the field verbatim and `interrupted.rs:161` files it as written — with no run-time consequence, for the reason just given.

### Rules the implementer must obey (copied here because they bite)

- Test names are sentences in snake_case (`a_queue_followup_from_the_lead_stores_the_runs_project_root`). Only the two existing tests whose assertions change are renamed; the others keep their `test_` names.
- One `cargo` process at a time: `cargo test -p openalpaca_core --lib <filter>`. The workspace run is the stage's, not this task's.
- No test touches `~/.openalpaca`: every store and every project is a `tempfile` directory (`tempfile` is a regular dependency of `openalpaca_core`, `crates/openalpaca_core/Cargo.toml:34`). `MemoryScopeContext::for_request` compares a root against `store::home_root()` read-only (`scope_context.rs:43-58`), exactly as the existing test at `scope_context.rs:218` does; no `HomeStoreGuard` is needed because nothing here writes to a store.
- macOS `tempfile` paths live under `/var/folders/…`, and `workspace_id_from_root` canonicalises (`memory/workspace.rs:81-86`) to `/private/var/folders/…`. Every expected root below is computed from `.canonicalize()`, never from `dir.path()` as returned.
- No `&s[..n]` anywhere (nothing here cuts a string); no `std::env::set_var` (nothing here touches the environment).
- Commit messages: conventional prefix, a `Verified:` line naming the test command and its result, and the `Co-Authored-By` line the implementing session is given.
- No migration (the `lane_followups` columns are unchanged, `crates/openalpaca_storage/src/migrations/001_baseline.sql:269-282`), no `docs/api` regeneration (the generator reads route modules and migrations, not `ToolContext`), no new route, verb, config key or event variant.

---

### Task F4.1: `queue_followup` stores the resolved project root, from the main loop and from the lead

**Files:** Modify: `crates/openalpaca_core/src/runner/lead_agent/tools.rs:1357-1361` (the struct doc's two mode bullets), `:1479-1492` (`execute_with_context`, doc and body); `crates/openalpaca_core/src/tools/builtins/main_loop.rs:304-306` (comment). Test: `crates/openalpaca_core/src/runner/lead_agent/tests.rs:775-813` (replaced) and a new test inserted directly after it.

**Interfaces:** Consumes: `ToolContext { request_workspace_root: Option<String>, workspace_path: Option<String>, principal: Option<Principal>, .. }` (`tools/registry/mod.rs:58-158`, `#[derive(Default)]`); `QueueFollowupTool::new(db: Option<Database>, bus: EventBus, task_id: String, lane_key: String, created_by: String, unattended: bool) -> Self` (`tools.rs:1376-1394`) and `QueueFollowupTool::for_main_loop(db: Option<Database>, bus: EventBus, lane_key: String, created_by: String, unattended: bool) -> Self` (`:1396-1411`); `fn queue(&self, arguments: &serde_json::Value, principal: Principal, workspace_path: Option<&str>) -> Result<String, String>` (`:1413-1460`, private, unchanged); `FollowupRepository::new(&Database)` and `list_queued_by_lane(&self, lane_key: &str) -> Result<Vec<FollowupRecord>>` (`crates/openalpaca_storage/src/repository/followup/mod.rs:86`, `:293`); `FollowupRecord { workspace_path: Option<String>, source_task_id: Option<String>, principal_json: String, .. }` (`:20-43`); `MemoryScopeContext::for_request(workspace_path: Option<&str>) -> Self` with `pub request_workspace_root: Option<String>` (`memory/scope_context.rs:118`, `:84`), already imported in `tests.rs:2`. The registry hands a context to every builtin when one is present — `handler.execute_with_context(arguments, ctx)` at `tools/registry/mod.rs:768` — and the lead's sandbox always has one (`runner/lead_agent/mod.rs:278-292`), so the lead reaches this method, not `execute`. Produces: nothing new; the row's `workspace_path` column now holds a canonical project root or NULL on every writer.

- [ ] **Step 1: Write the failing tests.** Replace `tests.rs:775-813` (`test_queue_followup_main_loop_uses_ctx_identity_and_workspace_path`: the `#[tokio::test]` attribute at `:775` and the function `:776-813`) with the first test, and insert the second directly after it (before `test_queue_followup_ctx_without_principal_falls_back_to_created_by` at today's `:816`):

```rust
/// F4 — the row records the turn's project root, which is what the follow-up
/// runner hands back to the gateway (`apps/openalpacad/src/followup.rs:44`),
/// never the path the client sent. The turn sent a directory two levels
/// below the `.git` marker; the row holds the canonical root.
#[tokio::test]
async fn a_queue_followup_from_the_main_loop_stores_the_turns_resolved_project_root() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let sent = project.path().join("src").join("lib");
    std::fs::create_dir_all(&sent).unwrap();
    let sent = sent.to_string_lossy().to_string();
    // The root as the resolver spells it (`memory/workspace.rs:81-86`):
    // canonical, which on macOS turns `/var/folders/…` into
    // `/private/var/folders/…`.
    let root = project
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let tool = QueueFollowupTool::for_main_loop(
        Some(db.clone()),
        EventBus::default(),
        "junpei:cli".to_string(),
        "fallback-owner".to_string(),
        false,
    );
    // The context the main loop builds (`simple_query_handler.rs:150-163`):
    // the turn's scope context resolves the root; the path as sent rides
    // beside it.
    let scope = MemoryScopeContext::for_request(Some(sent.as_str()));
    assert_eq!(scope.request_workspace_root.as_deref(), Some(root.as_str()));
    let ctx = crate::tools::registry::ToolContext {
        owner_id: Some("junpei".to_string()),
        principal: Some(crate::security::policy::Principal::User {
            global_id: "junpei".to_string(),
        }),
        request_workspace_root: scope.request_workspace_root.clone(),
        workspace_path: Some(sent.clone()),
        ..Default::default()
    };
    tool.execute_with_context(&serde_json::json!({"description": "Do it later"}), &ctx)
        .await
        .unwrap();

    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("junpei:cli").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source_task_id, None);
    assert_eq!(
        rows[0].workspace_path.as_deref(),
        Some(root.as_str()),
        "the row must hold the resolved root, not the path as sent ({sent})"
    );
    let principal: crate::security::policy::Principal =
        serde_json::from_str(&rows[0].principal_json).unwrap();
    assert_eq!(
        principal,
        crate::security::policy::Principal::User {
            global_id: "junpei".to_string()
        }
    );
}

/// F4 — the lead's context carries no `workspace_path`
/// (`runner/lead_agent/mod.rs:278-292`) and does carry
/// `request_workspace_root`, the run's project. A follow-up a workflow queues
/// must run in that project; until this fix the row held NULL and the
/// follow-up turn ran with none.
#[tokio::test]
async fn a_queue_followup_from_the_lead_stores_the_runs_project_root() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let root = MemoryScopeContext::for_request(project.path().to_str())
        .request_workspace_root
        .expect("a directory carrying .git resolves to itself");
    assert_eq!(
        root,
        project
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string()
    );

    // The lead-agent constructor (`runner/lead_agent/mod.rs:342-350`):
    // identity from the task's `created_by`, the run as `source_task_id`.
    let tool = QueueFollowupTool::new(
        Some(db.clone()),
        EventBus::default(),
        "task-1".to_string(),
        "junpei:cli".to_string(),
        "junpei".to_string(),
        false,
    );
    // The context exactly as `run_lead_agent` builds it (`mod.rs:278-292`):
    // no principal, no scope, no workspace_path — and the run's root.
    let ctx = crate::tools::registry::ToolContext {
        agent_id: Some("lead_agent::abc".to_string()),
        agent_instance_id: Some("lead_agent::abc".to_string()),
        task_id: Some("task-1".to_string()),
        owner_id: Some("junpei".to_string()),
        workspace_id: Some(root.clone()),
        request_workspace_root: Some(root.clone()),
        lane_key: Some("junpei:cli".to_string()),
        source: Some("cli".to_string()),
        ..Default::default()
    };
    tool.execute_with_context(
        &serde_json::json!({"description": "Also update the docs"}),
        &ctx,
    )
    .await
    .unwrap();

    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("junpei:cli").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source_task_id.as_deref(), Some("task-1"));
    assert_eq!(
        rows[0].workspace_path.as_deref(),
        Some(root.as_str()),
        "a follow-up promised by a workflow must re-enter in the run's project"
    );
    // No principal on the lead's context: the constructor's `created_by` stands in.
    let principal: crate::security::policy::Principal =
        serde_json::from_str(&rows[0].principal_json).unwrap();
    assert_eq!(
        principal,
        crate::security::policy::Principal::User {
            global_id: "junpei".to_string()
        }
    );
}

/// Review focus — a turn whose path no longer exists (a CLI started in a
/// directory since deleted, a row from an older build): the resolver has
/// nothing to canonicalise and yields no root, and the row holds NULL —
/// the same as a path under no marker. Nothing panics, nothing is stored
/// as sent.
#[tokio::test]
async fn a_queue_followup_from_a_path_that_no_longer_exists_stores_no_project() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let gone = tempfile::tempdir().unwrap();
    let sent = gone.path().join("deleted").join("project");
    let sent = sent.to_string_lossy().to_string();
    drop(gone);
    assert!(!std::path::Path::new(&sent).exists());

    let tool = QueueFollowupTool::for_main_loop(
        Some(db.clone()),
        EventBus::default(),
        "junpei:cli".to_string(),
        "junpei".to_string(),
        false,
    );
    let scope = MemoryScopeContext::for_request(Some(sent.as_str()));
    assert_eq!(scope.request_workspace_root, None);
    let ctx = crate::tools::registry::ToolContext {
        owner_id: Some("junpei".to_string()),
        request_workspace_root: scope.request_workspace_root.clone(),
        workspace_path: Some(sent.clone()),
        ..Default::default()
    };
    tool.execute_with_context(&serde_json::json!({"description": "Later"}), &ctx)
        .await
        .unwrap();

    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("junpei:cli").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].workspace_path, None, "a vanished path is no project, not a stored string");
}
```

(The third test goes directly after the second. Before the change it fails too — the row holds the as-sent string — so the filter counts in Steps 2 and 4 are 7.)

- [ ] **Step 2: Run the tests to see them fail.** Run: `cargo test -p openalpaca_core --lib queue_followup` ; Expected: 7 tests run — the three existing `test_queue_followup_*` and `tools::builtins::steer_workflow::tests::test_steer_full_inbox_suggests_queue_followup`, whose name the filter also matches, pass — **3 fail** (the vanished-path test's row holds the as-sent string before the change):
  - `a_queue_followup_from_the_main_loop_stores_the_turns_resolved_project_root` fails at the `workspace_path` assertion: `left: Some("/var/folders/…/.tmpXXXXXX/src/lib")` (the path as sent; `/tmp/.tmpXXXXXX/src/lib` on Linux), `right: Some("/private/var/folders/…/.tmpXXXXXX")` (`/tmp/.tmpXXXXXX` on Linux), message `the row must hold the resolved root, not the path as sent (…)`.
  - `a_queue_followup_from_the_lead_stores_the_runs_project_root` fails at the `workspace_path` assertion: `left: None`, `right: Some("/private/var/…/.tmpXXXXXX")`, message `a follow-up promised by a workflow must re-enter in the run's project`.

- [ ] **Step 3: Write the minimal implementation.** In `tools.rs`, replace `:1479-1492` (the doc comment at `:1479-1481` and the whole `execute_with_context` function at `:1482-1492`, closing brace included) with:

```rust
    /// Context-aware variant — the one a sandboxed call reaches
    /// (`tools/registry/mod.rs:768` hands every builtin its context). The
    /// identity is the invocation's threaded principal, else the
    /// constructor's `created_by` (the lead's context threads none); the
    /// project is `request_workspace_root`, the turn's resolved root — the
    /// run's project on the lead's context, the turn's on the main loop's
    /// (F4). Never `workspace_path`: that was the path as the client sent
    /// it, which the lead's context never carried, so a follow-up a workflow
    /// promised re-entered with no project.
    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let principal = ctx
            .principal
            .clone()
            .unwrap_or_else(|| self.constructed_principal());
        self.queue(arguments, principal, ctx.request_workspace_root.as_deref())
    }
```

  Leave `execute` (`:1475-1477`) as it is: with no context there is no root, and `None` is right. In the struct doc, replace `:1357-1361` (from `/// Two construction modes:` through the main-loop bullet's second line) with:

```rust
/// Two construction modes:
/// - **Lead agent** (`new`): identity from the task's `created_by`, the
///   running task recorded as `source_task_id`.
/// - **Main loop** (`for_main_loop`): per-request, no source task — identity
///   comes from the invocation's `ToolContext`.
///
/// In both modes the row's `workspace_path` is the context's
/// `request_workspace_root`, the turn's resolved project root (F4), so the
/// turn the item later runs as re-enters in that project.
```

  In `main_loop.rs:304-306`, replace the comment with:

```rust
        // Main-loop queue_followup: identity and the turn's resolved project
        // root (`request_workspace_root`, F4) come from the invocation's
        // ToolContext; the constructor value is only the identity fallback.
```

- [ ] **Step 4: Run the tests to see them pass.** Run: `cargo test -p openalpaca_core --lib queue_followup` ; Expected: `test result: ok. 7 passed`. The neighbours that must still pass are in that filter: `test_queue_followup_inserts_row_and_publishes_event`, `test_queue_followup_system_owner_maps_to_system_principal` and `test_queue_followup_ctx_without_principal_falls_back_to_created_by` (its `assert_eq!(rows[0].workspace_path, None)` on a default context still holds, because `request_workspace_root` defaults to `None`). Then run `cargo test -p openalpaca_core --lib tools::builtins::main_loop` for the 11 tests of the module the comment edit sits in (the bare filter `main_loop` also matches a dozen orchestrator tests named `*main_loop*`, which take minutes and prove nothing here); Expected: `test result: ok. 11 passed`.

- [ ] **Step 5: Commit.** Run:
  ```
  git add crates/openalpaca_core/src/runner/lead_agent/tools.rs crates/openalpaca_core/src/runner/lead_agent/tests.rs crates/openalpaca_core/src/tools/builtins/main_loop.rs
  git commit -m "fix(core): queue_followup stores the turn's resolved project root, from the lead too (F4)

The row held ToolContext.workspace_path, the path as the client sent it,
which the lead's context never carries — so a follow-up a workflow's lead
queued stored NULL and re-entered with no project. Both construction modes
now store request_workspace_root, the resolved root every sandboxed
context carries. For a path under a marker the re-entry turn resolves the
same project as before; for a path under none, NULL and the raw path both
resolve to no request root.

Verified: cargo test -p openalpaca_core --lib queue_followup (7 passed,
exit 0); the two new tests fail before the change with the as-sent path
and with None.

Co-Authored-By: <the line the implementing session is given>"
  ```

---

### Task F4.2: `steer_workflow` stores the resolved project root

**Files:** Modify: `crates/openalpaca_core/src/tools/builtins/steer_workflow.rs:112-120` (the `SteeringMsg` literal); `crates/openalpaca_core/src/runner/steering.rs:56-67` (`SteeringMsg` doc and the `workspace_path` field's doc); comments only, because this commit removes `ToolContext.workspace_path`'s last reader: `crates/openalpaca_core/src/tools/registry/mod.rs:49-52` (the field's bullet in the struct doc) and `:107-109` (the field's own doc), `crates/openalpaca_core/src/orchestrator/query_handler/mod.rs:11-13` (the `MainLoop` variant's field doc), `crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs:156-158` (the comment above the match arm at `:159-164`). Test: `crates/openalpaca_core/src/tools/builtins/steer_workflow.rs:192-205` (`lane_ctx`) and `:216-256` (`test_steer_ok_queues_message_with_ctx_identity`, renamed).

**Interfaces:** Consumes: `SteeringMsg { text: String, request_id: Uuid, principal: Principal, scope: Scope, workspace_path: Option<String>, received_at: DateTime<Utc>, origin: SteeringOrigin }` (`runner/steering.rs:61-71`); `push_steering(shared_context: &SharedContext, bus: &EventBus, task_id: &str, lane_key: &str, msg: SteeringMsg, db: Option<&Database>) -> Result<usize, SteeringPushError>` (`steering.rs:102-109`); `SteeringInbox::drain_all()` (used by the existing test at `steer_workflow.rs:232`); `SteerWorkflowTool::new(shared_context: Arc<SharedContext>, bus: EventBus, db: Option<Database>) -> Self` (`steer_workflow.rs:33-43`). Produces: nothing new; `SteeringMsg.workspace_path` now means "the turn's resolved project root or `None`" on this producer.

- [ ] **Step 1: Write the failing test.** In the `tests` module of `steer_workflow.rs`, change `lane_ctx` (`:192-205`) so both fields are present and differ — the as-sent path a level below the resolved root:

```rust
    fn lane_ctx(lane_key: &str) -> ToolContext {
        ToolContext {
            owner_id: Some("user1".to_string()),
            lane_key: Some(lane_key.to_string()),
            source: Some("cli".to_string()),
            request_id: Some(Uuid::new_v4()),
            principal: Some(Principal::User {
                global_id: "user1".to_string(),
            }),
            scope: Some(Scope::Global),
            // The resolved root beside the path as sent, as the main loop
            // builds them (`simple_query_handler.rs:150-163`).
            request_workspace_root: Some("/ws/project".to_string()),
            workspace_path: Some("/ws/project/src".to_string()),
            ..Default::default()
        }
    }
```

  Then rename the test at `:216-217` and change its comment and assertion at `:231` and `:241`:

```rust
    /// F4 — the message carries the turn's resolved project root, never the
    /// path as sent. Literals, not a tempdir: this tool copies the field the
    /// context builder resolved; the resolution itself is tested against the
    /// filesystem in `runner/lead_agent/tests.rs` and `orchestrator/tests.rs`.
    #[tokio::test]
    async fn a_model_steer_carries_the_turns_resolved_project_root() {
        let (shared, bus, inbox) = setup_with_workflow(16);
        let mut rx = bus.subscribe();
        let tool = SteerWorkflowTool::new(shared, bus.clone(), None);

        let result = tool
            .execute_with_context(
                &serde_json::json!({"task_id": "task-1", "message": "focus on tests"}),
                &lane_ctx("user1:cli"),
            )
            .await
            .expect("push should succeed");
        assert!(result.contains("task-1"), "missing task id: {result}");

        // Message landed with the ctx identity and the resolved root.
        let drained = inbox.drain_all();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].text, "focus on tests");
        assert_eq!(
            drained[0].principal,
            Principal::User {
                global_id: "user1".to_string()
            }
        );
        assert_eq!(
            drained[0].workspace_path.as_deref(),
            Some("/ws/project"),
            "the steering message must carry the resolved root, not the path as sent"
        );

        // WorkflowSteered published by the shared push helper.
        let mut found = false;
        while let Ok(event) = rx.try_recv() {
            if let SystemEvent::WorkflowSteered {
                task_id, lane_key, ..
            } = event
            {
                assert_eq!(task_id, "task-1");
                assert_eq!(lane_key, "user1:cli");
                found = true;
            }
        }
        assert!(found, "WorkflowSteered was not published");
    }
```

  The other four tests in the module (`test_steer_full_inbox_suggests_queue_followup`, `test_steer_closed_inbox_reports_finished`, `test_steer_cross_lane_rejected`, `test_steer_requires_lane_context`) share `lane_ctx` and assert nothing about `workspace_path`; they are untouched.

- [ ] **Step 2: Run the test to see it fail.** Run: `cargo test -p openalpaca_core --lib tools::builtins::steer_workflow` ; Expected: 5 tests run, **1 fails** — `a_model_steer_carries_the_turns_resolved_project_root` at the `workspace_path` assertion: `left: Some("/ws/project/src")`, `right: Some("/ws/project")`, message `the steering message must carry the resolved root, not the path as sent`.

- [ ] **Step 3: Write the minimal implementation.** Replace `steer_workflow.rs:112-120` with:

```rust
        let msg = SteeringMsg {
            text: message.to_string(),
            request_id: ctx.request_id.unwrap_or_else(Uuid::nil),
            principal,
            scope,
            // F4 — the turn's resolved project root, so a message left
            // undelivered at the workflow's exit is filed
            // (`unprocessed_steering`) under the project the steer was aimed
            // at, as the HTTP steer route's row already is
            // (`apps/openalpacad/src/routes/tasks.rs:1012-1020`). Never
            // `workspace_path`, the path as sent.
            workspace_path: ctx.request_workspace_root.clone(),
            received_at: Utc::now(),
            origin: SteeringOrigin::User,
        };
```

  In `runner/steering.rs`, replace the struct doc at `:56-60` and give the field at `:67` its own doc:

```rust
/// A single interjection targeted at a running workflow.
///
/// Carries the originating principal, scope and project root so leftover
/// messages can re-enter the front door as a fresh turn (follow-up
/// conversion) — and [`SteeringOrigin`], which says whether that is allowed
/// at all.
#[derive(Debug, Clone)]
pub struct SteeringMsg {
    pub text: String,
    pub request_id: Uuid,
    pub principal: Principal,
    pub scope: Scope,
    /// The turn's resolved project root — `ToolContext.request_workspace_root`
    /// on the `steer_workflow` tool, the turn's `MemoryScopeContext` on the
    /// `/steer` prefix, the header's root else the run's own on the HTTP
    /// route — never the path as the client sent it (F4); `None` when the
    /// turn carried no project. Written as `workspace_path` to the `steering`
    /// session-log record and to the `unprocessed_steering` row a message
    /// left undelivered becomes.
    pub workspace_path: Option<String>,
    pub received_at: DateTime<Utc>,
    /// Who wrote it — see [`SteeringOrigin`].
    pub origin: SteeringOrigin,
}
```

  (The `/steer` prefix half of that sentence becomes true in Task F4.3; the two commits land together in this stage.)

  Four comments now describe a reader this commit removes. The field itself stays — `ToolContext.workspace_path` is still threaded by `handlers.rs:352` through `LoopOverrides::MainLoop` into the main loop's context (`simple_query_handler.rs:159-164`), and taking it out is the collapse `registry/mod.rs:27-28` already records as a follow-up, not something the F4 row asks for (open point 1) — so each comment says what is true from this commit on:
  1. `registry/mod.rs:49-52` — replace the `workspace_path` bullet with:
  ```rust
  /// - `workspace_path` — the path **as sent**, unresolved, threaded only on the
  ///   tool-mode main loop and read by **no tool** since F4: `steer_workflow`
  ///   (`tools/builtins/steer_workflow.rs`) and `queue_followup`
  ///   (`runner/lead_agent/tools.rs`) persist `request_workspace_root`, which
  ///   the lead's context — which never carried the as-sent path — has too.
  ```
  2. `registry/mod.rs:107-109` — the field's three doc lines become:
  ```rust
      /// Filesystem workspace path of the originating request (from GUI/CLI),
      /// when one was provided — as sent, unresolved. Read by no tool since F4:
      /// the steering and follow-up writers persist `request_workspace_root`.
  ```
  3. `query_handler/mod.rs:11-13` — the `MainLoop` variant's field doc becomes:
  ```rust
          /// Filesystem workspace path from the originating request, threaded
          /// into `ToolContext.workspace_path`, which no tool reads since F4
          /// (the steer/followup items persist `request_workspace_root`).
  ```
  4. `simple_query_handler.rs:156-158` — the comment above the match arm becomes:
  ```rust
              // Threaded only on the tool-mode main loop; read by no tool since
              // F4 (steer/followup items persist `request_workspace_root`), and
              // None elsewhere.
  ```
  The code under each comment is unchanged, and `/// **Three fields are three views of one input, and are to be collapsed**` (`registry/mod.rs:27`) stays true.

- [ ] **Step 4: Run the test to see it pass.** Run: `cargo test -p openalpaca_core --lib tools::builtins::steer_workflow` ; Expected: `test result: ok. 5 passed` — the run compiles the whole lib test binary, so a slip in one of the four comment edits (an unclosed doc line, a stray character outside `///`) fails here. Then run `cargo test -p openalpaca_core --lib runner::steering` ; Expected: `test result: ok.` with 0 failed — the record tests there (`a_steering_record_carries_what_a_recovered_followup_needs` `:589-629`, `a_steering_record_with_no_project_says_so` `:631-662`, `a_dropped_steering_record_is_filed_as_a_followup` `:699-766`) round-trip whatever `workspace_path` the message carries (`"/repo"`, `None`) and are unaffected by the doc change.

- [ ] **Step 5: Commit.** Run:
  ```
  git add crates/openalpaca_core/src/tools/builtins/steer_workflow.rs crates/openalpaca_core/src/runner/steering.rs crates/openalpaca_core/src/tools/registry/mod.rs crates/openalpaca_core/src/orchestrator/query_handler/mod.rs crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs
  git commit -m "fix(core): steer_workflow carries the turn's resolved project root (F4)

The message held ToolContext.workspace_path, the path as sent; it now
holds request_workspace_root, the resolved root, so an interjection left
undelivered at the workflow's exit is filed under the project the steer
was aimed at — as the HTTP steer route already files it. Nothing reads an
unprocessed_steering row's workspace_path today; this fixes what the row
and the session-log record say.

With this, no tool reads ToolContext.workspace_path. The field stays (its
removal is the collapse registry/mod.rs records as a follow-up); the four
comments that said steer/followup items persist it now say it is read by
no tool. Comments only, no code change in those three files.

Verified: cargo test -p openalpaca_core --lib tools::builtins::steer_workflow
(5 passed, exit 0); the renamed test fails before the change with the
as-sent path.

Co-Authored-By: <the line the implementing session is given>"
  ```

---

### Task F4.3: the deterministic `/steer` command carries the resolved project root

**Files:** Modify: `crates/openalpaca_core/src/orchestrator/handlers.rs:205-212` (the `handle_steer_prefix` call); `crates/openalpaca_core/src/orchestrator/task_ops.rs:361-374` (doc and signature of `handle_steer_prefix`) and `:392-400` (the `SteeringMsg` literal). Test: `crates/openalpaca_core/src/orchestrator/tests.rs`, a new test inserted after `test_steer_prefix_single_workflow_pushes_and_confirms` (which ends at `:2538`).

**Interfaces:** Consumes: `scope_ctx: MemoryScopeContext`, built at `handlers.rs:86` as `MemoryScopeContext::for_request(workspace_path.as_deref())` and in scope at `:205`; `Orchestrator::handle_steer_prefix(&self, request_id: Uuid, text: &str, principal: &Principal, scope: &Scope, lane_key: &str, workspace_path: Option<String>) -> Result<String, String>` (`task_ops.rs:366-374`, `pub(super)`, one caller); `HandleRequest { request_id, source, content, principal, scope, lane_key, workspace_path: Option<String>, stream_id, model_override, unattended, turn_sink }` (`gateway/router/mod.rs:122-147`); the test helpers `make_steering_orchestrator()` (`tests.rs:2451`), `SharedContext::task_registry.register(id, title)`, `register_steering_inbox(task_id, inbox)`, `register_workflow_for_lane(lane, task_id)` as the test at `:2502-2538` uses them. Produces: the parameter is renamed `request_workspace_root: Option<String>`; the message's `workspace_path` is the turn's resolved root.

- [ ] **Step 1: Write the failing test.** Insert after `:2538` in `orchestrator/tests.rs`:

```rust
/// F4 — the deterministic `/steer` carries the turn's resolved project root,
/// the value `handlers.rs` resolves for the turn, not the path the client
/// sent: a subdirectory is sent, the directory carrying `.git` is what the
/// message holds. That root is what a message left undelivered at the
/// workflow's exit is filed under (`dispatcher/lead_agent.rs:761-768`).
/// The orchestrator here has no database and no session log, so the drained
/// inbox is the whole observable.
#[tokio::test]
async fn a_steer_command_carries_the_turns_resolved_project_root() {
    let project = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let sent = project.path().join("src");
    std::fs::create_dir(&sent).unwrap();
    let root = project.path().canonicalize().unwrap();

    let orch = make_steering_orchestrator();
    orch.shared_context
        .task_registry
        .register("task-1".to_string(), "Build the report".to_string());
    let inbox = Arc::new(crate::runner::steering::SteeringInbox::default());
    orch.shared_context
        .register_steering_inbox("task-1", inbox.clone());
    orch.shared_context
        .register_workflow_for_lane("user1:cli", "task-1");

    let reply = orch
        .handle_message(HandleRequest {
            request_id: Uuid::new_v4(),
            source: "cli".to_string(),
            content: "/steer switch to staging".to_string(),
            principal: Principal::User {
                global_id: "user1".to_string(),
            },
            scope: Scope::Global,
            lane_key: "user1:cli".to_string(),
            workspace_path: Some(sent.to_string_lossy().to_string()),
            stream_id: None,
            model_override: None,
            unattended: false,
            turn_sink: None,
        })
        .await
        .unwrap();
    assert!(reply.contains("task-1"), "unexpected reply: {reply}");

    let queued = inbox.drain_all();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].text, "switch to staging");
    assert_eq!(
        queued[0].workspace_path.as_deref(),
        root.to_str(),
        "the steering message must carry the resolved root, not the path as sent ({})",
        sent.display()
    );
}
```

  (`HandleRequest`, `Principal`, `Scope`, `Uuid` and `Arc` are already imported at the top of `tests.rs` (`:1-14`, and `Arc` through `use super::*;`); `tempfile` is used elsewhere in the file, e.g. `:750`.)

- [ ] **Step 2: Run the test to see it fail.** Run: `cargo test -p openalpaca_core --lib steer_command` ; Expected: 1 test run, **FAIL** at the `workspace_path` assertion: `left: Some("/var/folders/…/.tmpXXXXXX/src")` (the path as sent; `/tmp/.tmpXXXXXX/src` on Linux), `right: Some("/private/var/folders/…/.tmpXXXXXX")` (`/tmp/.tmpXXXXXX` on Linux), message `the steering message must carry the resolved root, not the path as sent (…)`.

- [ ] **Step 3: Write the minimal implementation.** In `handlers.rs`, replace `:205-212` with:

```rust
            self.handle_steer_prefix(
                request_id,
                steer_text,
                &principal,
                &scope,
                &lane_key,
                // F4 — the turn's resolved root, the same value every tool
                // on this turn reads as `request_workspace_root`; never the
                // path as sent.
                scope_ctx.request_workspace_root.clone(),
            )
```

  In `task_ops.rs`, replace `:361-374` (the doc comment at `:361-364`, the attribute at `:365` and the signature at `:366-374`) with:

```rust
    /// Handle the deterministic `/steer <message>` prefix (Routing V2):
    /// guaranteed injection into the lane's sole running workflow, bypassing
    /// the model. Replies are deterministic; the reply is always `Ok` — a
    /// missing or ambiguous target is an answer, not a handler failure.
    ///
    /// `request_workspace_root` is the turn's resolved project root
    /// (`MemoryScopeContext::for_request`, `handlers.rs`), `None` when the
    /// turn carried no project; it is what the message records (F4).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_steer_prefix(
        &self,
        request_id: uuid::Uuid,
        text: &str,
        principal: &crate::security::policy::Principal,
        scope: &crate::security::policy::Scope,
        lane_key: &str,
        request_workspace_root: Option<String>,
    ) -> Result<String, String> {
```

  and in the literal at `:392-400` change the one field:

```rust
                let msg = SteeringMsg {
                    text: text.to_string(),
                    request_id,
                    principal: principal.clone(),
                    scope: scope.clone(),
                    workspace_path: request_workspace_root,
                    received_at: Utc::now(),
                    origin: SteeringOrigin::User,
                };
```

  The destructured `workspace_path` at `handlers.rs:54` stays in use at `:86` and `:352`, so no unused-variable warning appears.

- [ ] **Step 4: Run the test to see it pass.** Run: `cargo test -p openalpaca_core --lib steer_command` ; Expected: `test result: ok. 1 passed`. Then run `cargo test -p openalpaca_core --lib steer_prefix` ; Expected: `test result: ok. 5 passed` — `test_steer_prefix_no_active_workflow`, `test_steer_prefix_single_workflow_pushes_and_confirms`, `test_steer_prefix_full_inbox_explains_backlog`, `test_steer_prefix_multiple_workflows_asks_which`, `test_steer_prefix_flag_off_routes_unchanged`, all of which send `workspace_path: None` and are unaffected.

- [ ] **Step 5: Commit.** Run:
  ```
  git add crates/openalpaca_core/src/orchestrator/handlers.rs crates/openalpaca_core/src/orchestrator/task_ops.rs crates/openalpaca_core/src/orchestrator/tests.rs
  git commit -m "fix(core): the /steer command carries the turn's resolved project root (F4)

handle_steer_prefix was handed the request's workspace_path as sent; it
is now handed the turn's MemoryScopeContext.request_workspace_root, which
handlers.rs already resolves for every tool on the turn. A /steer from a
subdirectory records the project root, as the model-routed steer and the
HTTP route do.

Verified: cargo test -p openalpaca_core --lib steer_command (1 passed) and
--lib steer_prefix (5 passed), exit 0; the new test fails before the
change with the as-sent path.

Co-Authored-By: <the line the implementing session is given>"
  ```

---

### Task F4.4: the documents say what the rows hold

**Files:** Modify: `docs/agent-loop.md:275-278` (the Producers bullet) and `:336-338` (the Follow-up Runner paragraph's first sentence); `docs/tools/TECHNICAL.md:209` (the meaning cell of the `workspace_path` row of the `ToolContext` table); `docs/tools/DESIGN.md:156` (the third item of the "Which workspace" cell). Test: none — prose; the generated `docs/api/` is untouched (its `workspace_path` rows at `docs/api/apps/openalpacad.md:583` and `:609` are route request bodies, not `ToolContext`), so `python3 scripts/gen_api_docs.py --check` has nothing to say and need not be run here.

**Interfaces:** none.

- [ ] **Step 1: Write the failing check.** Run: `grep -n "originating principal/scope/workspace" docs/agent-loop.md; grep -n "The path as sent; persisted" docs/tools/TECHNICAL.md; grep -n "workspace_path. (as sent by the client)" docs/tools/DESIGN.md` ; Expected before the change: one line from each — `agent-loop.md:338` ("with the originating principal/scope/workspace. When a"), `TECHNICAL.md:209` (the `workspace_path` table row) and `DESIGN.md:156` (the "Which workspace" table row).

- [ ] **Step 2: Run the check to see it "fail".** The three greps print those lines.

- [ ] **Step 3: Write the edits.**
  1. `docs/agent-loop.md:275-278` — replace the Producers bullet with:
  ```
  - **Producers**: the `/steer ` prefix (deterministic) and the
    `steer_workflow` tool (model-routed). Both put the turn's resolved
    project root on the `SteeringMsg` (`request_workspace_root`, never the
    path as sent — F4), which a message left undelivered at exit keeps on
    its `unprocessed_steering` row. Push results: `Ok` reports queue
    depth; `Full` directs the model to offer `queue_followup`; `Closed`
    means the workflow already detached.
  ```
  2. `docs/agent-loop.md:336-338` — replace the first sentence of the Follow-up Runner paragraph with:
  ```
  `queue_followup` (lead-agent tool, plus a main-loop variant on
  workflow-attached lanes) writes `followup` rows into `lane_followups`
  with the originating principal and the turn's resolved project root
  (`ToolContext.request_workspace_root`, F4) as `workspace_path` — the
  lead's context carries the run's project, so a follow-up a workflow
  promised re-enters in that project; the main loop's carries the turn's.
  ```
  (The paragraph's remaining sentences, from "When a workflow finalizes…", are unchanged.)
  3. `docs/tools/TECHNICAL.md:209` — the row stays (the field does); its meaning cell, which reads "The path as sent; persisted with steering and follow-up items", becomes:
  ```
  The path as sent; threaded on the main loop and read by no tool since F4 — the steering and follow-up writers persist `request_workspace_root`
  ```
  4. `docs/tools/DESIGN.md:156` — in the "Which workspace" cell the third item stays; its parenthesis "(as sent by the client)" becomes "(as sent by the client; read by no tool since F4)".

- [ ] **Step 4: Run the check to see it pass.** Re-run the Step 1 greps ; Expected: no output (the `DESIGN.md` pattern ends in a literal `)`, which no longer follows "client" once the parenthesis carries the F4 clause). Then `grep -n "request_workspace_root" docs/agent-loop.md` ; Expected: the two new lines (the Producers bullet and the Follow-up Runner sentence).

- [ ] **Step 5: Commit.** Run:
  ```
  git add docs/agent-loop.md docs/tools/TECHNICAL.md docs/tools/DESIGN.md
  git commit -m "docs: a follow-up and a steering message keep the turn's project (F4)

agent-loop.md says both steering producers and queue_followup record the
turn's resolved project root; the tool-system docs say the ToolContext
field workspace_path is read by no tool since F4 (the field itself stays).

Verified: the three grep checks in the plan print nothing after the edit.

Co-Authored-By: <the line the implementing session is given>"
  ```

---

### Open points (for the lead; none blocks Tasks F4.1–3)

1. **`ToolContext.workspace_path` is dead after Tasks F4.1–3, and this plan leaves it in place.** Its two readers now persist `request_workspace_root`, so the field and its threading — `handlers.rs:352` → `LoopOverrides::MainLoop { workspace_path, .. }` (`query_handler/mod.rs:14`) → the match arm at `simple_query_handler.rs:159-164` — carry a value nothing reads. The F4 row asks for the writers and nothing else, and `registry/mod.rs:27-28` already records the collapse of the three workspace views as a follow-up, so Task F4.2 corrects the four comments and Task F4.4 the two doc rows instead of deleting the field. For the lead, the inventory read for that follow-up: `grep -rn "workspace_path" crates/openalpaca_core/src --include='*.rs'` finds, besides the `HandleRequest`/`GatewayRequest`/`SteeringMsg`/`FollowupItem`/`FollowupRecord` fields and the `scope_context.rs`/`persistence.rs` parameters (which stay), exactly two production sites naming the `ToolContext`/`LoopOverrides` field — `simple_query_handler.rs:159-164` and `handlers.rs:352` — plus the field (`registry/mod.rs:110`) and the variant's field (`query_handler/mod.rs:14`); the test literals that would go with it are the two Tasks F4.1 and 2 add (`workspace_path: Some(sent.clone()),` and `workspace_path: Some("/ws/project/src".to_string()),`); `crates/openalpaca_core/tests/memory_eval.rs:12626-12633` builds a `ToolContext` naming `request_workspace_root` only (its `workspace_path:` literals at `:3466` and `:7391` are `GatewayRequest`/`HandleRequest` fields), and no crate outside `openalpaca_core` builds a `ToolContext` literal (`grep -rn "ToolContext {" apps crates --include='*.rs'` outside core hits only `PersonaToolContext`, `apps/openalpacad/src/services/tools.rs:42`). The removal is compiler-checked; `a_turn_without_a_workspace_path_carries_no_request_root` and `a_turn_with_a_workspace_path_carries_the_resolved_root` (`orchestrator/tests.rs:732-758`) already pin that a `HandleRequest.workspace_path` still reaches the tools as `request_workspace_root`. The `TECHNICAL.md:209` row, the `DESIGN.md:156` item and the four comments Task F4.2 rewrites would go with the field.
2. **A steer from a turn with no project.** The HTTP steer route stores "the header's resolved root, else the run's own project" (`apps/openalpacad/src/routes/tasks.rs:1012-1020`: `header_workspace.or_else(|| task.workspace_id.clone())`). After F4 the chat producers — `steer_workflow` and `/steer` — store the turn's root or `None`, with no fallback to the run's project. Whether a message steered from a project-less chat turn into a project's run should be filed under the run's project is a design question this draft does not take; today it changes nothing observable (no reader of an `unprocessed_steering` row's `workspace_path`).
3. **Names.** `SteeringMsg.workspace_path`, `QueueFollowupTool::queue`'s `workspace_path` parameter and `FollowupItem.workspace_path` keep their names although they now hold a resolved root; the column is `lane_followups.workspace_path` and design R14 keeps rows keyed by path, so the draft leaves the names and fixes the docs. A rename is a separate, mechanical commit if wanted.
4. **`workspace_id` on a NULL row, on a daemon not started in the home store.** Stated above under "What stays the same": the one run-time difference between a stored markerless path and NULL is the CWD-derived `workspace_id` on re-entry, which no memory path reads. Recorded here so the verifier does not read "identical" as unqualified.

---

## The stage's end

### Task G: The stage gate — one workspace run

**Files:** none changed. **Interfaces:** none.

The gate runs ONCE, after the last code task is committed. Never run two cargo processes at the same time.

- [ ] **Step 1: The workspace tests, one run**

Run (from the repository root):

```bash
CARGO_TERM_COLOR=never cargo test --workspace --exclude openalpaca_gui 2>&1 | tee /tmp/stage0-test.log | tail -40
```

Expected: every binary ends `test result: ok`; the totals over the run are 4,496 passed at the start of this stage plus the tests this plan adds, 0 failed, 16 ignored. Then, because the MCP manager's hang tests leave orphaned `sleep 3600` children that hold a piped stdout open:

```bash
pkill -f "sleep 3600"; grep -c "test result: FAILED" /tmp/stage0-test.log
```

Expected: `0`.

- [ ] **Step 2: Clippy against the baseline of 80**

```bash
CARGO_TERM_COLOR=never cargo clippy --workspace --exclude openalpaca_gui --all-targets 2>&1 > /tmp/stage0-clippy.log; grep -c '^warning' /tmp/stage0-clippy.log; grep -c '^warning: .* generated [0-9]* warning' /tmp/stage0-clippy.log
```

Expected: the first count minus the second is **80** (the baseline, `CLAUDE.md`). A higher number means this stage added a warning: fix it in the task that introduced it.

- [ ] **Step 3: The generated API docs**

```bash
python3 scripts/gen_api_docs.py --check
```

Expected: exit 0, no drift reported. This stage changes no route and no migration; if the check reports drift, something outside the plan changed and the implementer stops and reports.

- [ ] **Step 4: The GUI backend compiles**

```bash
cargo check -p openalpaca_gui
```

Expected: `Finished`. CI compiles nothing under `src-tauri`, so this is that crate's only gate.

- [ ] **Step 5: The four bun gates are NOT run — and the plan says so**

No task of this stage changes a file under `apps/openalpaca-gui/`. If `git diff --stat <stage start>..HEAD -- apps/openalpaca-gui` is empty, skip them and write "bun gates not run: no GUI file changed" in the ledger. If it is not empty, run all four (`bun run check`, `bun run test`, `bun run format:check`, `bun run build` in `apps/openalpaca-gui`) — `bun run check` alone is not enough (`CLAUDE.md`, Gotchas).

- [ ] **Step 6: Record the figures** in `tasks/todo.md`: the commit the gate ran at, the test totals, the clippy count, and the two checks.

### Task L: The live check on the rebuilt daemon

**Files:** Create: `tasks/storage-design/stage0-live-report.md` (the report; whitelisted by `.gitignore`'s `!tasks/storage-design/*.md`).

**The surface.** The spec's §16 says "a live check on the rebuilt app"; this stage's three assertions are daemon behaviour, and the approval card is read through `openalpaca tasks confirmations watch`, so the CLI against the rebuilt daemon is the surface here — the app runs the same daemon. Task R amends §16 to say so.

**Standing rules for every live check (from `tasks/lessons.md` and the owner):**
- `~/.openalpaca` is the owner's real store. Never read it, write it, list it, or point any command at it.
- Both `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` point into a scratch directory, and the daemon's working directory is the scratch store root (where the packaged app's sidecar runs). The current directory is never the repository.
- The daemon runs under `scripts/eval/memory/loopback-only.sb` **from its first launch**, and the profile is proven before the first model turn. Nothing leaves the machine.
- The model is the local Ollama `qwen3.8:27b-mlx` (the owner's standing authorization for live checks).
- Never answer a macOS permission or keychain dialog; leave it on screen and say so in the report.

- [ ] **Step 1: Build once, then prepare the scratch home**

```bash
cargo build -p openalpacad -p openalpaca
export OA_SCRATCH="$(mktemp -d /tmp/stage0-live.XXXXXX)"
mkdir -p "$OA_SCRATCH/store" "$OA_SCRATCH/config" "$OA_SCRATCH/repo"
export OPENALPACA_HOME_STORE="$OA_SCRATCH/store" OPENALPACA_CONFIG_DIR="$OA_SCRATCH/config"
git -C "$OA_SCRATCH/repo" init -q
echo "$OA_SCRATCH"
```

Expected: the build finishes; the three directories exist; `repo/.git` exists (the project for the F4 check).

- [ ] **Step 2: Start the daemon sandboxed, from the store root, and prove the profile**

```bash
cd "$OPENALPACA_HOME_STORE" && (OPENALPACA_MASTER_KEY=1111111111111111111111111111111111111111111111111111111111111111 sandbox-exec -f /Users/tangyunpei/dev/OpenAlpaca/scripts/eval/memory/loopback-only.sb /Users/tangyunpei/dev/OpenAlpaca/target/debug/openalpacad > "$OA_SCRATCH/daemon.out" 2>&1 &) ; sleep 5; cat "$OPENALPACA_HOME_STORE/state/discovery.json" | head -c 300
```

The throwaway `OPENALPACA_MASTER_KEY` is set on the daemon's command line **only** — never `export`ed, because the CLI's `load_or_generate_at` reads the environment first and would encrypt with it — so the inherited-variable case F3 exists for (the fact check's finding 4) is the one checked: the daemon's own file key must win and the variable must not reach a child. Expected: `discovery.json` exists and names a port. Prove the profile with the same sandbox, before any model turn:

```bash
sandbox-exec -f /Users/tangyunpei/dev/OpenAlpaca/scripts/eval/memory/loopback-only.sb curl -sS -m 5 http://localhost:11434/api/tags | head -c 80; echo; sandbox-exec -f /Users/tangyunpei/dev/OpenAlpaca/scripts/eval/memory/loopback-only.sb curl -sS -m 5 https://example.com; echo "exit $?"
```

Expected: the first prints Ollama's model list; the second fails (a non-zero exit, no HTML). Write both outcomes in the report.

- [ ] **Step 3: Turn Ollama on and pin the model**

```bash
cd "$OA_SCRATCH" && /Users/tangyunpei/dev/OpenAlpaca/target/debug/openalpaca config set ai.ollama.enabled true && /Users/tangyunpei/dev/OpenAlpaca/target/debug/openalpaca config set ai.default_model qwen3.8:27b-mlx
```

Expected: both accepted (`ai.default_model` is the `[orchestrator] model` key; there is no per-provider model key). `GET /v1/status` shows `llm.effective_default_model` = `qwen3.8:27b-mlx` once discovery has run (a few seconds): read it with the CLI's status verb if it has one, else `curl -sS -H "Authorization: Bearer $(python3 -c 'import json;print(json.load(open("'"$OPENALPACA_HOME_STORE"'/state/discovery.json"))["auth"]["token"])')" http://127.0.0.1:<port>/v1/status` with the port from `discovery.json` (the token is read into the command, never printed).

- [ ] **Step 4: F3 — a shell command sees no master key**

From `$OA_SCRATCH` (not the repository), an attended CLI chat:

```bash
/Users/tangyunpei/dev/OpenAlpaca/target/debug/openalpaca chat --message 'run command: env | grep -c OPENALPACA_MASTER_KEY || echo ABSENT'
```

Approve the `shell_execute` card at the inline `[y/N]`. Expected: the answer reports `0` or `ABSENT`, although the daemon was started with the variable set (Step 2). The spawned command's environment is the proof and the only one: `ps -E`/`ps eww` and `/proc/<pid>/environ` show the environment block as it was at exec, so they would still show the inherited variable and prove nothing (F3.2 Step 1). Write PASS/FAIL with the answer text.

- [ ] **Step 5: F1 — `system_agent`'s shell command raises a card**

```bash
/Users/tangyunpei/dev/OpenAlpaca/target/debug/openalpaca chat --message 'Start a workflow: delegate to the system agent (template system_agent) one shell command, uname -a, and report its output verbatim.'
```

In a second terminal with the same two environment variables: `openalpaca tasks confirmations watch`. Expected: a pending confirmation for `shell_execute` whose agent is `system_agent` appears; approve it; the run finishes with the output. FAIL if the command runs with no confirmation. The model may delegate to another template — then say so and ask again naming the template; record every attempt.

- [ ] **Step 6: F2 — a delegated write's report names the artifact's path**

```bash
/Users/tangyunpei/dev/OpenAlpaca/target/debug/openalpaca chat --message 'Start a workflow: a writing agent writes notes/tea.md with three facts about green tea, then report exactly where the file is saved.'
```

Approve the `file_write` card. Expected: the completion report names the artifact's absolute path under `$OPENALPACA_HOME_STORE/artifacts/`, not `notes/tea.md` alone (Round G's W-a sample 2 failed exactly here: the lead saw 500 characters of the subagent's answer and the path was cut). PASS when the absolute path appears in the report.

- [ ] **Step 7: F4 — a follow-up queued by a lead keeps the project**

From the scratch git repository:

```bash
cd "$OA_SCRATCH/repo" && /Users/tangyunpei/dev/OpenAlpaca/target/debug/openalpaca chat --message 'Start a workflow: write a one-line README.md as an artifact, and queue a follow-up that will later summarise what was written.'
```

Expected: after the run finalises, `sqlite3 "$OPENALPACA_HOME_STORE/state/openalpaca.db" "select workspace_path from lane_followups order by id desc limit 3"` shows the canonical path of `$OA_SCRATCH/repo` on the follow-up the lead queued (before F4 it was NULL). The follow-up may auto-start; that is fine. If the lead queued no follow-up, ask once more naming `queue_followup`; record every attempt.

- [ ] **Step 8: The log, and stop**

```bash
grep -c ' ERROR ' "$OPENALPACA_HOME_STORE/state/logs/daemon.log"; grep ' WARN ' "$OPENALPACA_HOME_STORE/state/logs/daemon.log" | cut -c1-200 | sort | uniq -c | head; cd "$OA_SCRATCH" && /Users/tangyunpei/dev/OpenAlpaca/target/debug/openalpaca daemon stop
```

Expected: 0 ERROR lines (list every WARN in the report); the daemon stops within 15 s.

- [ ] **Step 9: Write the report and commit it**

`tasks/storage-design/stage0-live-report.md`: date, commit, model, the profile proof, one PASS/FAIL line per check with the answer text quoted, the WARN lines, anything left on screen. Then:

```bash
git add tasks/storage-design/stage0-live-report.md && git commit -m "docs(storage-design): Stage 0 live check report"
```

### Task R: Records

The `<commit>` and `<range>` tokens below are filled in at execution time from `git log` — they are not placeholders, since the commits do not exist until the tasks land.

**Files:** Modify: `tasks/storage-design/design.md` (§16, one clause); `CLAUDE.md` (the Round G paragraph; the *Owner decisions pending* bullet; the Gotchas bullet on `file_write`); `tasks/api-fix-plan.md` (§0: T34 to *Owner decisions taken* with its commit, a new T35 pending row, FW-2's row pointing at the design and Stage 2; a tenth *Owner decisions taken* subsection for ST-1…ST-22 pointing at `tasks/storage-design/design.md` §0.1; the rev line → 3.21); `tasks/todo.md` (the review section).

- [ ] **Step 0: `tasks/storage-design/design.md` §16** — in the Stage 0 paragraph, "a live check on the rebuilt app" becomes "a live check on the rebuilt daemon through the CLI (the app runs the same daemon)", with a rev 0.7 entry in the revision log naming this plan.

- [ ] **Step 1: `CLAUDE.md`** — in the Round G paragraph replace the two "recorded, not built" clauses: the 500-character cut (`wait_for_subagents hands the lead the first 500 characters … recorded, not built`) now reads "fixed by the storage design's Stage 0, F2, <commit>"; "**Found live and not built — owner decision T34:** …" now reads "**Found live; fixed by Stage 0, F1 (<commit>, owner decision T34 taken 2026-10-05 as part of the storage design):** …" keeping the description of what was wrong. Keep the FW-2 clause ("recorded, not built") but add "taken by the storage design (ST-10, `tasks/storage-design/design.md`), built in its Stage 2". In the *Owner decisions pending* bullet move T34 out of the pending list and add **T35** (an extension tool that does not declare itself harmless prompts; three options; not built) and the storage design's rows: "the storage design's **ST-1…ST-22** are taken (2026-10-04/05, `tasks/storage-design/design.md` §0.1); Stage 0 built <range>; Stages 1 and 2 pending". Add one sentence at the top of *Current Work*: "**The storage design is approved** (`tasks/storage-design/design.md` rev 0.6, ST-1…ST-22); Stage 0 landed as <range>; Stages 1 and 2 are next."

- [ ] **Step 2: `tasks/api-fix-plan.md`** — rev 3.21 changelog line; T34 row removed from the pending table and added under a new "### Owner decisions taken — `feat/markdown-memory`, 2026-10-05 (the storage design: ST-1…ST-22; T34 taken; Stage 0 built)" subsection with the commits of F1–F4; a **T35** pending row (text from the design §18); FW-2's pending row rewritten: "Taken as ST-10/ST-11/ST-12 of the storage design; built in its Stage 2"; the status line's list of pending rows updated.

- [ ] **Step 3: Commit**

```bash
git add tasks/storage-design/design.md CLAUDE.md tasks/api-fix-plan.md tasks/todo.md && git commit -m "docs: Stage 0 of the storage design recorded (T34 taken, F1–F4 built, T35 pending)"
```
