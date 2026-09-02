# Extension Enable — the two-axis design (N5)

**Status:** design of record, **rev 5** (critique round 3 repaired; anchors re-verified against source), accepted mechanism. **Residue for the reconciliation pass (rev 6):** one S4 blocker — the legacy `tools.allow` skill-resolution branch silently drops withdrawn tools (§6.2 #10 / §7.2, both call sites); one over-broad invariant sentence (`Disabling` can carry `enabled=true` on the deny and declaration-gone paths); 29 non-blocking notes; plus the Claude Code lessons (`tasks/research/claude-code-design-lessons.md`). Supersedes ADR-029 (flat per-tool disable list) entirely.
**Rev 5 in one line:** the T4b seal is checked at the **install point** inside `do_handshake` as well as at
`reconnect`'s entry, so a reconnect already in flight when the owner flips the switch cannot install a live child
into a sealed client (S2); the persisted bit is written **first** on both verbs (step W, before the CAS) and the
state invariant, §4.1 and §8 now say so consistently; the crash reaper carries the `generation` and re-checks the
record under the mutex, so it can never tear down the incarnation that replaced a crashed one; the MCP
"declaration gone" path runs T0–T4 with no file write, and every non-route persistence failure has a defined
outcome; §10 case 5's cron claim is re-grounded on `build_skill_frontmatter_from_info` and pinned by a test.
**Date:** 2026-09-01 · **Branch:** `feat/ui-rework`
**Answers:** `tasks/api-fix-plan.md` §0 **N5** (line 46) — the settled model S1–S4, mechanism now fixed.
**Every file:line below was read directly at authoring time.**

---

## 1. The model in one page

Two axes. They are different questions, they live in different places, and they compose by subtraction.

| | **ALLOW** — per agent | **ENABLE** — per extension |
|---|---|---|
| Question | *May this agent use this tool?* | *Is this integration running at all?* |
| Unit | one agent template / one skill | one MCP server / one plugin (**S1**) |
| Declared in | template `capabilities`, skill `requires_capabilities` | `config/mcp.toml` `[servers.<n>] enabled`, `.permissions.toml` `enabled` |
| Mechanism | capability index → `tools_for_capabilities*` (`registry/mod.rs:558`, `:584`) | extension loaded / unloaded (**S2**) |
| Exists today | **yes**, unchanged by this design | **no** — this is the work |
| Applies to builtins | **yes** (the only governance builtins have) | **never** |

**Builtins are not on the ENABLE axis, ever.** `extension_tool_defs` (`registry/mod.rs:627`) already draws the
line structurally — it matches `ToolBackend::Mcp { .. } | ToolBackend::Plugin(_)` and nothing else, so
`BuiltIn` and the `Http`/`Command` tools loaded from `config/tools/*.toml` cannot enter through it. This design
keeps that boundary and adds no enable field, no API field, and no GUI control for a builtin row.

### Worked example A — a tool travels from registration to an agent's surface

`config/mcp.toml` declares `[servers.github] transport = "stdio"`, `enabled` absent → `default_enabled()`
returns `true` (`tools/mcp/config.rs:44`).

1. **Boot.** `McpSupervisor::reconcile_all` reads the declaration, sees `enabled = true`, connects.
   `McpClient::connect` → `do_handshake` (`client.rs:107`) spawns the stdio child.
2. **Discovery + registration.** `list_tools` → `bridge::rmcp_tool_to_registered` (`tools/mcp/bridge.rs:20`)
   makes `github__create_issue`, with `provides_capabilities: vec!["github__create_issue"]` (`bridge.rs:59`),
   `author: "mcp:github"` (`bridge.rs:63`), `backend: ToolBackend::Mcp { client, remote_name, server_name }`.
   `ToolRegistry::register` (`registry/mod.rs:229`) inserts it and pushes its name under the capability key.
3. **Ledger.** The supervisor records `ExtensionState::Enabled` for `mcp:github` and the tool name in the
   ledger's `tool_names` (a `name → ExtensionId` map the ledger **keeps after a disable**, §3.2 T1 — it is what
   attributes a call to a tool that is no longer in the registry), and holds its own `Arc<McpClient>` — the
   ledger never holds a client. The record also carries the load's **`generation`** (§3.0 Fact 3): E0 handed the
   supervisor `1`, and the `ToolBackend::Mcp { generation: 1, .. }` it built at step 2 is how a later call proves
   it is talking to *this* load and not a previous one.
4. **Surface — lead agent.** `runner/lead_agent/mod.rs:154` calls `extension_tool_defs(..)`; the tool is on the
   lead's surface **regardless of the lead template's `capabilities`**. This is the honest asymmetry the
   owner's directive rests on: on the lead and main-loop surfaces the ALLOW axis does not apply today, so
   per-extension ENABLE is the *only* possible control there.
5. **Surface — subagent.** A template declaring `capabilities: ["github__create_issue"]` resolves it through
   `resolve_agent_tools` → `tools_for_capabilities_with_deny` (`registry/mod.rs:584`). This *is* the ALLOW axis.
6. **Call.** Model emits the tool call → `SandboxManager::execute_tool` (`security/sandbox/mod.rs:134`) →
   step 5 `registry.execute_with_context` (`sandbox/mod.rs:310`/`:314`) → `ToolBackend::Mcp` arm →
   `client.call_tool` (`registry/mod.rs:392`).

### Worked example B — the moment `github` is disabled

Owner flips the switch. `POST /v1/extensions/mcp/github/disable`:

0. **W — persist.** `enabled = false` is written to `config/mcp.toml` atomically (§2.1 writer) **before any
   transition is taken**. If the write fails the route returns `500` and nothing else happens — no CAS, no
   teardown, the server keeps running and the row still reads `enabled: true`. The bit therefore never
   disagrees with what the owner will see after a restart, whatever happens to the steps below.
1. **T0 — gate.** One CAS on the ledger entry: `Enabled → Disabling`. **From this instant** every call to
   `github__create_issue` is refused, whichever registry the caller holds: a **deep snapshot** captured before
   the toggle (the lead agent always takes one, `runner/lead_agent/mod.rs:231`; skills and the main loop take
   one only under the predicates in §3.0) still finds the entry and is stopped at the gate, because the ledger
   is `Arc`-shared through `Clone for ToolRegistry` (`registry/mod.rs:156`); the **live registry** (the
   ordinary skill case) stops finding the entry after T1 and is stopped on the **miss path** (§6.2 #1), which
   consults the ledger's retained `tool_names` and returns the same attributed refusal.
2. **T1 — withdraw.** Capabilities tombstoned (`github__create_issue → {mcp:github}` — a set, §7.2), the tool
   name kept in the ledger's `tool_names`, then `tool_registry.remove("github__create_issue")`
   (`registry/mod.rs:267`).
3. **T2–T5 — withdraw, drain, close, commit.** Contributions are withdrawn (T2, plugins); in-flight calls
   finish under a bounded drain (T3); the connection is closed and sealed (T4/T4b); the record is committed
   `Disabled` in memory and the transition is announced (T5). The file was already written at W.

What each consumer now sees:

| consumer | before | after |
|---|---|---|
| lead agent surface | tool present | absent (`extension_tool_defs` filters on state) |
| subagent declaring the capability | tool present | absent, **plus** an attributed warn (`Moment::SurfaceAssembly`) |
| skill with `requires_capabilities: ["github__create_issue"]` | runs with the tool | **refused**, naming the extension |
| model calls it anyway — from a stale snapshot **or** from the live registry after T1 | tool runs / `Tool 'github__create_issue' not found in registry` | `Err("tool 'github__create_issue' is unavailable: the MCP server 'github' is disabled…")` on **both** paths — returned as the tool result, so the model relays it; `warn!` + `ExtensionCapabilityWithheld` on both |
| cron skill depending on it | fires, runs toolless, fabricates | skipped per fire (warn + event); **one** notice written to the owner's default lane at the transition (§7.3) |
| stdio child process | running | **gone** (S2) |
| the tool's `capability_index` key | `["github__create_issue"]` | key removed entirely |

---

## 2. The toggle

**S1: the install unit is the toggle unit.** Two toggles exist and no others.

### 2.1 MCP server

- **Writes:** `[servers.<name>] enabled = <bool>` in `config/mcp.toml`.
- **The field already ships.** `#[serde(default = "default_enabled")] enabled: bool` on both the `Stdio`
  (`tools/mcp/config.rs:62`) and `Http` (`:75`) variants, `default_enabled() -> true` at `:44`, read via
  `is_enabled()` at `:80-84`. It is already honoured at boot (`services/mcp.rs:50`) and documented in the
  shipped `config/mcp.toml`. **This design gives that field runtime effect; it does not invent it.**
- **Actuates:** `McpSupervisor::set_enabled(name, bool)` → writes the file atomically (**step W**, §3.2/§3.3)
  → `reconcile(name)`. This is the body of the trait verbs of §3: `enable(id)` **is** `set_enabled(name, true)`
  and `disable(id)` **is** `set_enabled(name, false)` — one API, not two. The write comes **first**; a write that
  fails returns `500` to the route and takes **no** CAS (the same shape as `409 store_unreadable`, §4), so the
  in-memory state never runs ahead of the disk.
- **The writer** is `toml_edit`-based (surgical `doc["servers"][name]["enabled"] = …`), so every comment in the
  hand-authored file survives. Write is: acquire `<path>.lock` (`file-lock`, already at `Cargo.toml:107`) →
  read → edit → **re-parse the result with `McpConfig::load`'s own parser** → write to
  `<path>.<pid>.tmp` → `sync_all` → `rename`. A failed re-parse aborts the write with the file untouched.
  `McpConfig` still needs no `Serialize` impl. The lock + temp + re-parse + rename helper is
  `openalpaca_core::config_io::atomic_write_toml` — it lives in `openalpaca_core` because both writers need
  it and `openalpaca_core` is the one crate that `apps/openalpacad` **and** `crates/openalpaca_plugins` already
  depend on (CLAUDE.md dependency graph); `toml_edit` is therefore a dependency of `openalpaca_core`. It
  lands in C1.

### 2.2 Plugin

- **Writes:** a **new** `enabled: bool` field on `PermissionEntry`
  (`crates/openalpaca_plugins/src/permission_gate.rs:11-17`), `#[serde(default = "default_true")]`, in the
  single install-wide `<plugins root>/.permissions.toml` (`permission_gate.rs:34-38`).
- **Consent becomes tri-state in the same entry.** Today `approved: bool` and `approved_at: String` have no
  serde defaults (`permission_gate.rs:13-14`), so an entry cannot exist without a decision — and the design
  needs exactly that: a never-approved plugin whose toggle the owner pre-set to off must keep that bit across
  a restart (§4), which means writing an entry for a plugin that has **no** consent decision. Rev 3 required
  this in four places and made it impossible in a fifth (§5 said the consent fields keep their "unchanged
  shape"). Fixed: `approved: Option<bool>` and `approved_at: Option<String>`, both `#[serde(default)]`.
  `None` is *pending*. Existing files parse unchanged — a bare `approved = true` deserialises into
  `Some(true)`. Full shape and the reader/writer rules in §5.
- **It must be a second field, never the existing `approved`.** Today `disable_plugin` ends with
  `permission_gate.deny(name)` (`manager.rs:682`) and `enable_plugin` begins with
  `permission_gate.approve(name, &capabilities)` (`manager.rs:638`) — so **enable and consent are one bit**.
  Consequence: turning an integration off silently revokes its trust decision, and turning it back on silently
  re-grants consent **for whatever the manifest currently declares** (`manager.rs:631-638` re-reads
  `state.manifest.capabilities.provides`). Splitting the field is what closes that hole.
- **Actuates:** `PluginManager::set_enabled` → atomic write of `.permissions.toml` (same `atomic_write_toml`
  helper, **step W**, write-first with the same `500`-and-no-CAS rule) → `reconcile(name)`. As for MCP,
  `enable(id)`/`disable(id)` on the trait are this call with `true`/`false`. There is no new
  `PluginSupervisor` struct: the existing `PluginManager`
  (`crates/openalpaca_plugins/src/manager.rs`, already on `AppState` at `state.rs:39`) grows the
  `ExtensionSupervisor` methods (§3). Where this document says "the plugin supervisor" it means that object.

### 2.3 What is *not* a toggle

- No per-tool switch anywhere (S1). `execution.skill_defaults.global_tool_deny` is **purged** (§11).
- No builtin toggle. No `config/tools/*.toml` toggle — those are the owner's own declarations in the owner's
  own config dir, governed by ALLOW only, exactly as today.
- Connectors (`system_config` `<name>.enabled`, `managers/connector.rs:151-176`) and LLM providers
  (`llm.toml providers.<n>.enabled`) keep their existing stores. They are not tool providers; folding them in
  would widen `/v1/extensions` past "things that contribute tools" and re-open GAP-17's envelope question.
  `kind` is the extension point if a third tool-contributing kind ever appears.
  **Boundary, stated precisely:** a *plugin-contributed* connector or LLM provider is not a separate toggle — it
  is a contribution of the plugin and goes down with the plugin at T2 (§3.2). What stays outside
  `/v1/extensions` is the first-party connector/provider toggles, never a disabled plugin's residue.

---

## 3. Unload and reload

**Names, fixed once.** One trait, two implementations, one aggregator:

- `ExtensionSupervisor` — `enable(id)`, `disable(id)`, `reconcile(id)`, `reconcile_all()`, `list()`,
  `shutdown_all()`, plus the plugin-only `approve`/`deny`/`remove_orphan` on the plugin implementation.
  `enable`/`disable` are the two `set_enabled(name, bool)` actuators of §2.1/§2.2 seen through the trait: **write
  the bit, then reconcile** — not a second API beside them. The
  trait is declared in **`openalpaca_core::tools::extensions`** (C1) — its two implementors live in
  `openalpaca_plugins` and `apps/openalpacad`, and `openalpaca_core` is the only crate upstream of both
  (CLAUDE.md dependency graph), so nowhere else can hold it.
- `McpSupervisor` (**new**, `apps/openalpacad/src/managers/mcp.rs`) and the existing `PluginManager`
  (`crates/openalpaca_plugins/src/manager.rs`) implement it. Both hold the `Arc<ToolRegistry>` and reach the
  ledger through `tool_registry.extensions()`. **Between C2 and C6** the `Arc<McpSupervisor>` is parked on the
  services bundle `main.rs` already reads (`svcs.tool_registry` at `main.rs:337`), which is where the
  file-watcher reload arm (edge case 15) finds it and where the shutdown path calls its `shutdown_all()`
  directly; C6 folds it into `Extensions` and the direct calls move behind the aggregator.
- `Extensions { ledger: Arc<ExtensionLedger>, mcp: Arc<McpSupervisor>, plugins: Arc<PluginManager> }` is the
  aggregator; **`AppState.extensions: Arc<Extensions>`** is how routes reach it (§6.2 #15), and
  `Extensions::shutdown_all()` is the daemon-shutdown hook (§3.5). `ExtensionLedger` itself is pure
  bookkeeping in `openalpaca_core` — it never holds a client, a process, or a file path.

Every enable/disable runs inside the owning supervisor under a **per-extension `tokio::sync::Mutex` held
across the whole transition**, so two concurrent toggles serialise and a toggle never interleaves with a
reconcile or with the crash reaper (§3.6).

### 3.0 The facts everything rests on

There are three. The first two pull in opposite directions and the third is a consequence of the first; the
design has to hold all of them.

**Fact 1 — some runs hold a deep snapshot.** `Clone for ToolRegistry` (`registry/mod.rs:156-181`) is a
**deep** clone: it rebuilds the `tools` DashMap entry-by-entry, and each `RegisteredTool.backend` clone carries
a live `Arc<McpClient>` / `Arc<dyn PluginToolExecutor>`. Four production sites can take such a snapshot and
hold it for the whole run — but only **one** does so unconditionally:

```
crates/openalpaca_core/src/runner/lead_agent/mod.rs:231
    let lead_registry = (*tool_registry).clone();                       // ALWAYS
crates/openalpaca_core/src/orchestrator/skill/invocation.rs:593-596
    needs_clone = !scripts.is_empty() || !depends_on.is_empty()          // else :679 self.tool_registry.clone() — the Arc, LIVE
crates/openalpaca_core/src/orchestrator/skill/invoke_executor.rs:267-270
    same predicate                                                       // else the Arc, LIVE
crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs:625-631
    Some(set) if !set.instances.is_empty() => deep clone                 // else :631 self.tool_registry.clone() — LIVE
```

A lead agent that spawns 8 subagents and waits holds live extension backends for **minutes**. For that run,
**registry removal alone is not enforcement** — it is invisible to the snapshot.

**Fact 2 — the common skill run holds the live registry.** A file-based skill with
`requires_capabilities: ["github__create_issue"]` and no `scripts`/`depends_on` — the ordinary skill — runs
against the shared `Arc`. Disable the server mid-run and its next call reaches `execute_with_context`, whose
lookup short-circuits on the DashMap miss with `Tool 'x' not found in registry` (`registry/mod.rs:369-375`;
`execute` at `:305-311` says `Unknown tool`) **before any gate placed "after the lookup" could run**. That is
an unattributed refusal with no `warn!` and no event — S4 violated on the most common path. So the gate must
have **two arms**: on a **hit**, check the entry's extension (covers Fact 1); on a **miss**, ask the ledger
whether the name belongs to an extension it knows and is not `Enabled` (covers Fact 2). §6.2 #1 specifies
both. The ledger can answer the miss because T1 keeps each extension's `tool_names` after removal.

The shared-ledger field is what makes the hit arm work in every snapshot:

```rust
pub struct ToolRegistry {
    tools: DashMap<String, RegisteredTool>,
    capability_index: DashMap<String, Vec<String>>,
    extensions: Arc<ExtensionLedger>,   // NEW
    ...
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self { ..., extensions: Arc::clone(&self.extensions) }   // NEW — the whole design
    }
}
```

One `Arc::clone` covers all four snapshot sites and every future one, by construction. The live-registry
case needs no sharing at all — it *is* the registry the supervisor edits — which is why its hole is the miss
path, not staleness.

**Fact 3 — a snapshot can outlive an incarnation.** Fact 1's snapshot holds the *backend handle*, not just
the name. Disable and re-enable `github` while a lead agent is mid-run and three things are now true at once:
the ledger reads `Enabled`; the live registry holds a **new** `RegisteredTool` over a **new** `Arc<McpClient>`
(E4 is remove-then-register, and `connect` builds a fresh client); and the snapshot's own `tools` DashMap
still holds the **old** entry over the **old** client, which T4 disconnected and T4b sealed. A gate keyed on
`(extension → state)` alone passes that call, and what happens next is wrong on both kinds:

- **MCP.** The sealed client's `call_tool` finds `service == None` → `McpError::TransportClosed`
  (`client.rs:287`) → retriable → `reconnect()` (`:306`) → the T4b seal returns `TransportClosed` again →
  `?` propagates it. The registry's `Mcp` arm formats it as `MCP server 'github' tool 'create_issue' failed:
  transport closed unexpectedly` (`registry/mod.rs:396-402`) — no attribution to the toggle, no
  `ExtensionCapabilityWithheld`, and it repeats for every call the run makes for the rest of the run. S4,
  violated on the exact path §3.2 calls the normal shape of the feature and §3.3 E2 advertises as how a
  rotated credential takes effect mid-session.
- **Plugins — worse, a correctness bug.** The old `PluginToolProxy` (one is built per tool over
  `process.channel.clone()` at `manager.rs:831-834`) sends on a channel whose writer is gone →
  `PluginError::ChannelClosed` (`stdio_channel.rs:95`, `:120`). Rev 3's §3.6 item 2 told that proxy to call
  `mark_failed(ExtensionId::plugin(id), Crashed, ..)`, a CAS from `Enabled` — which **succeeds against the
  healthy new incarnation**: the reaper then runs T1→T2→T4 and kills the live process, the row reads
  `failed/crashed`, and the log says nothing (the registry's `Plugin` arm at `registry/mod.rs:333` returns the
  string verbatim with no `warn!`). Any stale snapshot could tear down a working plugin.

So the ledger needs a notion of **which load** a handle belongs to. **Decision: a per-extension
`generation: u64` on the ledger record, bumped at E0, stamped into every per-incarnation handle at its single
construction site, and checked by the gate and by `mark_failed`.** Rules:

1. `ledger.begin(ext, Enabling)` increments the record's `generation` and returns it; the supervisor threads
   that number into everything it builds for this load (E2–E4). A failed bring-up wastes a number; the reaper's
   crash path (§3.6) bumps nothing, because it creates no incarnation. `Disabled`, `Failed` and `Unapproved`
   records keep their last generation — it is the number the *next* load will exceed, and it is what a stale
   handle is compared against.
2. **Stamping** — one production literal per kind (§3.1): `ToolBackend::Mcp { .., generation }` at
   `tools/mcp/bridge.rs:46`, and `PluginToolProxy::new(plugin_id, channel, generation)` at `manager.rs:831`,
   surfaced through a `fn generation(&self) -> u64 { 0 }` **default method** on `PluginToolExecutor`
   (`crates/openalpaca_api/src/plugin_traits.rs:8-15`) so no other implementor changes. `PluginSkillBridge`
   (`manager.rs:419`) and `PluginAgentBridge` (`:452`) take the same number, because they too call
   `mark_failed`. `RegisteredTool::incarnation()` (§3.1) reads it back off any entry, live or snapshot.
3. **The hit arm compares.** `check(&ext, entry.incarnation(), ctx)` refuses when the state is not `Enabled`
   (as before) **and also** when the state is `Enabled` but the handle's generation is not the record's —
   `Blocked(Stale)`, with its own S4 string (§7.1): *"tool 'github__create_issue' belongs to a previous load of
   MCP server 'github' (this run started before it was re-enabled); the copy in this run is stale — it is
   available again on the next request"*. Same `warn!` + `ExtensionCapabilityWithheld`
   (`Moment::AttemptedUse`), deduped like any other. The live registry can never hold a stale entry (E4
   replaces it), so the ordinary skill is untouched; the miss arm needs no generation at all — a missing name
   has no handle. State is checked before generation, so a stale handle to an extension that is *again*
   disabled gets the disabled wording, which is the more useful of the two.
4. **`mark_failed(ext, generation, reason, detail)` is a no-op unless `generation` equals the record's current
   one** — stated beside its `Enabled`-only CAS in §3.6. A stale proxy's `ChannelClosed` therefore cannot flip
   the row of the incarnation that replaced it; it logs at `warn!` with `stale = true` and returns the `Stale`
   refusal instead. **The crash reaper is held to the same pair:** its message carries the generation and it
   re-checks `(state == Failed{Crashed}, generation == record's)` under the per-extension mutex before it
   touches anything (§3.6), so a reap queued behind a slow teardown cannot land on the incarnation an
   intervening `enable` built.

"Available again on the next request" is exact, not hopeful: every snapshot site in Fact 1 is per-request,
and the lead/main-loop surfaces are built from the live registry before the snapshot is taken (§6.2 #2).

### 3.1 Extension identity — no `RegisteredTool` struct change

The ledger keys on `ExtensionId { kind, name }`, **derived**, not stored on `RegisteredTool` (which has 79
struct-literal construction sites — `grep -rn 'RegisteredTool {' crates apps` returns 80 including the
`pub struct` line at `registry/mod.rs:121` — and no `Default`):

```rust
impl RegisteredTool {
    /// `None` for BuiltIn / Http / Command — those are never on the ENABLE axis.
    pub fn extension_id(&self) -> Option<ExtensionId> {
        match &self.backend {
            ToolBackend::Mcp { server_name, .. } => Some(ExtensionId::mcp(server_name)),
            // `author` is set exactly once, at manager.rs:843: format!("plugin:{}", name)
            ToolBackend::Plugin(_) => self.author.strip_prefix("plugin:").map(ExtensionId::plugin),
            _ => None,
        }
    }

    /// Which load of the extension this handle belongs to (§3.0 Fact 3). `None` for non-extension tools.
    pub fn incarnation(&self) -> Option<u64> {
        match &self.backend {
            ToolBackend::Mcp { generation, .. } => Some(*generation),
            ToolBackend::Plugin(executor) => Some(executor.generation()),
            _ => None,
        }
    }
}
```

Both identity producers are single-site and already exist: `bridge.rs:63` (`"mcp:{server}"`) and
`manager.rs:843` (`"plugin:{name}"`). Zero `RegisteredTool` construction-site edits.

**The one enum change, bounded.** `generation: u64` is added to the `ToolBackend::Mcp` variant
(`registry/mod.rs:96-100`), not to `RegisteredTool`. `grep -rn 'ToolBackend::Mcp {' crates apps` returns
eight sites: **one** production literal (`tools/mcp/bridge.rs:46`, which gains a `generation: u64` parameter
on `rmcp_tool_to_registered`; its single production caller in `services/mcp.rs` passes the E0 number from C2
on and `0` in C1, where no ledger record exists to compare it against), two destructuring arms that gain `..`
or the field (`registry/mod.rs:334`, `:384`), two `..` patterns that need nothing (`:634`, `:650`), and three
test literals (`main_loop.rs:486`, under the `#[cfg(test)]` at `:244`; `registry/tests.rs:1835`;
`lead_agent/tests.rs:1253`). Plugins need no enum change: the number rides inside the proxy behind
`Arc<dyn PluginToolExecutor>` and comes out through the trait's default method.

### 3.2 DISABLE — the exact sequence

**W — PERSIST (route path; before anything else).** `set_enabled(name, false)` writes `enabled = false`
through the §2.1/§2.2 atomic writer **before the CAS**. A failed write returns `500` with the current row and
takes no transition — the extension keeps running and the row still reads `enabled: true`, which is the truth.
The other three callers of this sequence never write here: on the **watcher** path the write *is* the trigger
(the owner's hand edit is already on disk, edge case 15); on the **declaration-gone** path there is nothing to
write into (the block is gone — see T5); on the **reaper** path (§3.6) the disposition does not change at all.
Writing first is what makes §3.4's "the owner's intent is durable" true for the enable side too, and it fixes
the persisted bit's meaning once: **the file always holds what the owner last asked for**, and the in-memory
state says how far reality has caught up.

**T0 — GATE (authoritative).** `ledger.begin(ext, Disabling)`, a CAS on the entry's state. From this instant
`ExtensionLedger::check(&ExtensionId, ..)` returns `Blocked` everywhere, in every snapshot. Everything after is
bookkeeping.

*Ordering inside `check`, so T3 can trust its counter:* `check` **increments the extension's in-flight counter
first, then reads the state**, and decrements again on `Blocked`. A caller that read `Enabled` an instant
before the T0 CAS is therefore already counted when T3 looks; the other order would let that call slip past
the drain and be torn down under.

> **Why T0 before T1, not after.** If deregistration ran first, a loop holding a snapshot would keep calling the
> tool **successfully** (the snapshot owns the backend) right up to teardown at T4, then start receiving raw
> `TransportClosed` / `ProcessCrashed` transport errors with no explanation. The owner flips a switch, the
> running workflow keeps using the integration for another minute, then fails incomprehensibly. With T0 first,
> the very next call after the toggle gets a clean, self-describing refusal — on a snapshot because the entry
> is still there and gated, on the live registry because the miss path is gated (§3.0 Fact 2). This is the
> normal shape of using the feature, not a contrived race.

**T1 — CAPABILITY WITHDRAWAL.** For each name in the ledger's `tool_names` for this extension:
1. record its `provides_capabilities` into the tombstone index (`capability → Set<ExtensionId>` — a **set**,
   because every tool a plugin registers carries the manifest's whole `capabilities.provides` list
   (`manager.rs:839`) and nothing stops two plugins declaring the same capability; §7.2),
2. `tool_registry.remove(name)` (`registry/mod.rs:267`) — which scrubs both string caps and provider-derived
   caps from `capability_index`.

3. **dependent scan + notice (§7.3).** Once every name is withdrawn, intersect the set just tombstoned (T1
   steps 1 and, for plugins, T2 step 1) with the agent registry's template `capabilities` and the skill
   catalog's `requires_capabilities`, emit the one un-deduplicated `warn!` + `ExtensionCapabilityWithdrawn`,
   and — if a cron-scheduled skill just became wholly unsatisfiable — the single owner notice. **This step is
   part of T1, not of the route handler**, so every path that runs T1 fires it: the route, the watcher's
   `reconcile_all` (edge case 15), and the §3.6 crash reaper. Its wording is keyed on the record's state at
   the time — `Disabling` → *"disabled"* (the route, watcher and declaration-gone paths), `Failed{Crashed}` →
   *"stopped running (crashed: <detail>)"* (the reaper) — because a crashed dependency of an unattended cron
   skill is the same failure with no human in the loop. Rev 3 put the scan "before returning the HTTP
   response", which left the crash path silent. **Step 3 fires only when the withdrawn set is non-empty**, so a
   second pass over an extension whose tools are already gone (T1–T4 are idempotent — §3.6 relies on this)
   announces nothing: one transition, one announcement (§7.3).

The `tool_names` entry is **not** cleared here. It is retained through `Disabled`/`Failed` and replaced
wholesale by the next **E5** `record_tools` (or dropped at restart, since the ledger is in-memory). Its two
consumers after T1 are the gate's miss arm (§6.2 #1) and the dependent scan of step 3. It is a different thing
from the API row's `tools` field, which reports what is **live** and is empty when not `Enabled` (§8) —
retention here is attribution, not a cache of what the extension would offer.

*Atomicity, stated precisely:* withdrawal is atomic **per tool**, not across the extension's N tools, and it
does not need to be. The authoritative gate is T0 — a single atomic store. A reader that observes T1 half-done
sees a shorter tool list in which **every remaining tool is already `Blocked`**. Worst case is a marginally
smaller prompt; never a call that should not have happened. Making it atomic across N tools would need a
registry-wide write lock on the *read* path, serialising every surface build in the daemon. Rejected on cost.

**T2 — CONTRIBUTION WITHDRAWAL (plugins).** A plugin contributes up to **five** things (`PluginState`,
`manager.rs:494-498`): tools, a capability provider, skills, agent templates, and — declared but not yet wired
into anything — a connector platform and an LLM provider with its model ids. All five are withdrawn here. Steps
1–3 are `unload_plugin`'s existing order (`manager.rs:514-577`), reused as-is; step 4 is what that function
explicitly declines to do:
1. `remove_capability_provider(handle)` (`manager.rs:523`) **and tombstone the plugin's virtual
   capabilities.** The provider's `derive_capabilities` returns `manifest.capabilities.virtual_.provides` for
   every tool whose `author` matches the plugin (`manager.rs:96-98`, built at `:470-474`) — a list **separate
   from** `capabilities.provides`, so T1's per-tool recording never sees it. Without this step a template
   naming a virtual capability would classify `unknown` (`debug!` only) at spawn — S4 Moment-2 silence for
   that case, even though an attempted call would still refuse. So T2 step 1 also calls
   `ledger.withdraw(ext, &manifest.capabilities.virtual_.provides)`, and E5's `restore(ext)` clears them
   with everything else. *Ordering relative to T1 is immaterial:* `remove()` scrubs provider-derived caps per
   tool (`registry/mod.rs:277-286`) and `remove_capability_provider` rebuilds the whole index from the tools
   that remain (`:477-535`, `rebuild_virtual_capability_index`), so either order converges; it is listed first
   only for parity with `unload_plugin`'s own sequence;
2. `skill_catalog.remove(skill_id)` per contributed skill (`manager.rs:546`);
3. `agent_registry.remove_template(agent_id)` per contributed template (`manager.rs:554`);
4. **connector + provider.** `unload_plugin` carries a NOTE (`manager.rs:559-561`) that *"connector
   deregistration from `ConnectorManager` and provider deregistration from `LlmRouter` are the daemon's
   responsibility, since `PluginManager` does not hold references to those subsystems"* — and nothing in the
   daemon performs them. Today that is a latent hole, not a live one: the bridge accessors
   `get_plugin_connector` (`manager.rs:750`) and `get_plugin_provider` (`manager.rs:761`) have **zero callers**
   outside `openalpaca_plugins` (grep across `apps/` and `crates/`, re-run for rev 3), so no plugin connector or
   provider is ever registered anywhere it would need removing from. Precisely: `ConnectorManager` is not
   missing a removal verb — it has `stop(name)` (`managers/connector.rs:225`) and `delete(name)` (`:179`) for
   first-party connectors — it has **no notion of a plugin-contributed platform at all** (`spawn_connector`,
   `:234`, matches first-party names only). S2 still has to hold by construction, so the rule is fixed now,
   before the bridges are wired: **whatever E4 registers, T2 deregisters, in the same supervisor.**
   Concretely: `PluginManager` already owns the `registered_connector` / `registered_provider` /
   `registered_models` triple (`manager.rs:494-496`); the `Disabled` record carries none of it (the API row's
   `connector`/`provider` fields are `null` unless `Enabled`, §8); when the provider bridge is wired, E4 calls
   `LlmRouter::register_*` and T2 calls the already-existing `LlmRouter::deregister_provider`
   (`crates/openalpaca_llm/src/routing/router/mod.rs:220`, zero callers today); when the connector bridge is
   wired, `ConnectorManager` gains a `register_platform`/`unregister_platform` pair — the second is the
   counterpart of a first that does not exist yet, not a fix for an absent removal path — and T2 calls it.
   Until then C3's guard test is the contract: after `disable`, `GET /v1/extensions` reports
   `connector: null, provider: null`, and `LlmRouter::list_models_for_provider(&provider_type)`
   (`crates/openalpaca_llm/src/routing/router/capacity.rs:145` — there is no `LlmRouter::list_models()`; that
   name is on `ModelRegistry`, `routing/model_registry/mod.rs:382`) returns nothing for the plugin's provider.

**T3 — DRAIN (bounded).** An extension has **two kinds** of in-flight work, and the drain must see both.

*(a) Tool calls.* `ExtensionLedger::check` returns, on success, an RAII `CallGuard` that increments a
per-extension in-flight `AtomicUsize`, decremented on drop. The guard is taken **inside `ToolRegistry`**, below
the sandbox, so it counts every tool call whichever sandbox instance issued it. (Rev 3 also claimed it covered
"direct `registry.execute*` callers" such as script tools and `invoke_skill:<dep>` backends. There are none in
production: every `registry.execute` / `.execute_with_context(` hit outside `registry/mod.rs` and
`sandbox/mod.rs:310`/`:314` is inside a `#[cfg(test)]` module or `tests/mcp_integration.rs`, and script tools
(`invocation.rs:203-217`) and `invoke_skill:<dep>` defs (`invoke_executor.rs:279-341`) are `BuiltIn`
registrations executed through the sandbox like everything else — and never extension tools. The claim was
unnecessary and is withdrawn; the gate's placement stands on §6.3.)

*(b) Out-of-process runs.* Two plugin paths never enter `ToolRegistry` for the run itself — only for the tool
calls the run makes:
- **plugin skills** — `PluginSkillBridge::invoke` (`crates/openalpaca_plugins/src/bridge/skill_bridge.rs:61-64`)
  issues `channel.call("skill/invoke", …)` straight over the plugin's stdio channel, then loops on
  `skill/invoke_continue`;
- **plugin agents** — `run_plugin_agent_loop` (`crates/openalpaca_core/src/runner/plugin_agent.rs:72`) drives
  `spawn` / `step` the same way, up to `MAX_PLUGIN_ITERATIONS`.

A drain that counted only (a) could read zero while a multi-minute `skill/invoke` is mid-flight, skip straight
to T4, and `process.kill()` under it (`manager.rs:564-569`); the caller would get a raw `plugin
X::skill/invoke: <channel error>` (`skill_bridge.rs:65`) — precisely the incomprehensible failure T0-before-T1
exists to prevent. So the guard is taken at a **second point**: the two in-process entry points into those
loops, which are the *only* two. For plugin skills that is `invoke_plugin_skill` (declared at
`invocation.rs:934`; its single caller is `invocation.rs:72`, and the executor's
`.invoke(query, &context, &callback)` is issued once inside it at `:1042`). For plugin agents it is the
`run_plugin_agent_loop` call at `lead_agent/tools.rs:513`, its one production caller. Both have the plugin
id in hand — `SkillSource::Plugin { plugin_id, .. }` and `AgentSource::Plugin { plugin_id, executor }`
(`agent/template/mod.rs:25-28`) — and both live in `openalpaca_core`, which owns the ledger. Both also have
the bridge's **`generation`** in hand (§3.0 rule 2: `PluginSkillBridge` and `PluginAgentBridge` carry the
load's number, exposed as `fn generation(&self) -> u64` on both), so the call is
`ledger.begin_run(ExtensionId::plugin(id), bridge.generation())` and it does three things:
1. **pre-flight** — returns the S4 refusal (§7.1 wording) if the extension is **recorded and not `Enabled`**
   (the same rule as §6.2a: an unrecorded id is `Allow`; only a test can reach that, since a plugin skill
   cannot exist without a supervisor record), **or** if it is `Enabled` but the bridge's generation is not the
   record's — the `Stale` refusal, before `spawn`/`skill/invoke` is ever sent. This matters for the agent
   path: an in-flight subagent holds a cloned executor (`lead_agent/tools.rs:447`), and a lead that spawns
   from that template after a disable → re-enable would otherwise pass pre-flight and fail only at its first
   RPC through the proxy's `Stale` path. So a run is never started against a `Disabling` plugin or against a
   previous load of one;
2. **holds a `CallGuard`** for the duration of the run, so the drain waits for it like any tool call;
3. **owns the exit** — the run future is awaited through `ledger.run_scoped(ext, fut)`: if the future returns
   `Err` and the ledger no longer reads `Enabled`, the error is *replaced* by the S4 refusal naming the
   extension. A run torn down at the deadline therefore fails with *"skill 'x' stopped: plugin 'notion' was
   disabled"*, not with a broken-pipe string.

The plugin-agent loop additionally consults `ledger.check()` at every step boundary, exactly where it already
checks the cancellation token (`plugin_agent.rs:106-116`), and on `Blocked` calls `executor.stop()` and returns
`Failed { error: <S4 refusal> }` — so it terminates deliberately at its next step rather than waiting for the
kill. The skill bridge cannot be interrupted between RPCs from outside (the loop is inside the plugin), so for
it the deadline **is** the mechanism: its tool callbacks are refused at the gate from T0, and the run ends
under `run_scoped` at the latest when T4 closes the channel.

*Deadline.* T3 awaits the combined counter reaching zero, bounded by **`[extensions] drain_timeout_secs`**
(`daemon.toml`, default **10 s**). There is no per-request `SandboxPolicy` at the supervisor level to take a
`max_tool_runtime_secs` from — policies are built per call site (§6.3 lists the seven production
`SandboxManager` constructions, six of them per-request) — so the knob is the only input. On expiry:
`warn!(ext, in_flight = n, "disable draining timed out; forcing teardown")` and proceed.

*In-flight calls are allowed to finish; we do not cancel them.* On a single-user daemon with no adversary the
risk worth engineering against is **corruption** — a half-written file, a duplicated API POST — not a 200 ms
exposure window. New calls are refused instantly at T0; existing tool calls complete under the sandbox's
per-call timeout (`sandbox/mod.rs:312-317`). **That bound is an invariant this design enforces, not a property
of the sandbox:** the wrapper is skipped entirely for `exempt_from_timeout` tools (`is_exempt` at
`sandbox/mod.rs:301`; the `:309-310` branch awaits them with no timeout). Both extension registrars happen to
set `exempt_from_timeout: false` today (`tools/mcp/bridge.rs:60`, `plugins/src/manager.rs:840`), but nothing
pins that. `ToolRegistry::register` /
`replace` therefore force `exempt_from_timeout = false` (with a `warn!`) for any tool whose `extension_id()` is
`Some`, and C1 carries the test `extension_tools_never_timeout_exempt`. Extension tools may never be exempt.

**T4 — TEARDOWN.**
- **Plugin:** `process.shutdown()` (3 s RPC) then `process.kill()` — `manager.rs:564-569` — **then
  `child.wait()` under a 2 s timeout.** `kill()` is `Child::start_kill` (`process_pool.rs:166-172`), which only
  *initiates* termination, and nothing today awaits the child; without the wait, "child gone" is a race the C3
  assertion would lose. On timeout: `warn!(plugin, "child did not exit after SIGKILL within 2s")` and proceed —
  the kernel will finish it, and the seal below already prevents reuse of the channel.
- **MCP:** `(*arc_client).clone().disconnect().await`.

  This works today and needs **no new method on `McpClient`**. `McpClient` is `#[derive(Clone)]` over
  `Arc<ClientInner>` (`crates/openalpaca_mcp/src/client.rs:48-51`, doc comment: *"Cheaply cloneable
  (`Arc<ClientInner>`); all clones share lifecycle state"*), and `disconnect(self)` (`client.rs:165-176`) takes
  the shared `inner.service` mutex, runs `close_with_timeout(5s)`, and sets `ConnectionState::Disconnected`
  **for every clone**. The supervisor holds its own `Arc<McpClient>` outside the registry, so teardown never
  depends on dropping the last registry Arc (`McpClient` has no `Drop` impl — an implicit drop performs no
  close at all).

  *The true bound on T4, stated.* `call_tool` holds the `inner.service` mutex **across its request await**
  (`client.rs:286-288`: `lock().await` … `running.call_tool(params).await` inside one guard scope), and
  `disconnect` takes the same mutex (`client.rs:166`). So if the drain expired with an MCP call still in
  flight, `disconnect` blocks until that call's future is dropped — by the sandbox timeout
  (`max_tool_runtime_secs`) or by `with_cancel_and_timeout`'s `request_timeout` (30 s, `client.rs:41`),
  whichever fires first. T4 therefore awaits `disconnect()` under
  `tokio::time::timeout(request_timeout + 1s)`. `tokio::time::timeout` **consumes** the future it timed out,
  so on expiry T4 spawns a **fresh** `(*arc).clone().disconnect()` detached (it completes when the straggler
  releases the lock — the `closed` seal below is already set, so nothing can reconnect meanwhile) and proceeds
  to T5 with a `"teardown pending: 1 call still holding the transport"` warning in the response. One clause so
  nobody expects more of that second call: if the timeout fired *inside* `close_with_timeout` rather than at
  the lock, `guard.take()` (`client.rs:167`) has already run, so the detached `disconnect` finds `None` and only
  flips `ConnectionState` — the close is already in progress in the first future's `RunningService`, which
  rmcp finishes on drop. The honest statement for §8 is: a disable returns within
  **`mutex_wait + drain_timeout_secs + min(max_tool_runtime_secs, request_timeout) + 1s`**, where `mutex_wait`
  is the time to acquire the per-extension mutex — zero unless an `Enabling` is in flight (§4.1 queues the
  `disable` behind it), and at most that server's `connect_timeout_secs` (default 30 s,
  `tools/mcp/config.rs:40`) when it is — and a `200` means *no new call can reach the extension and its
  teardown is either done or unstoppably in progress*. Plugins have no equivalent contention:
  `process.kill()` is non-blocking (`process_pool.rs:166-172`, `start_kill`).

  *How "the child is gone" is observed for MCP.* The daemon holds **no** handle on an MCP child. The
  `TokioChildProcess` is owned inside rmcp's `RunningService` (`transport/mod.rs:42`, built at
  `transport/stdio.rs:79`; the comment at `stdio.rs:120` says the wrapper does not expose it) and `McpClient`
  has no pid or handle accessor. When the `RunningService` is closed or dropped, rmcp's `ChildWithCleanup::drop`
  (`rmcp-0.16.0/src/transport/child_process.rs:48-58`) issues the kill from a **detached `tokio::spawn`**, so
  process exit is asynchronous and only externally observable — which is why C2's integration test observes
  it through the test server (a pidfile + liveness poll, §12), not through a `child.wait()` the daemon
  cannot perform. `child.wait()`/`try_wait` above are the **plugin** mechanism (`process_pool.rs:166-177`).

**T4b — SEAL THE CLIENT (mandatory; this is a live S2 hazard).**

After `disconnect`, `inner.service` is `None`, so a call from a surviving snapshot hits
`guard.as_ref().ok_or(McpError::TransportClosed)?` (`client.rs:287`). `McpError::TransportClosed` **is in the
retriable set** (`crates/openalpaca_mcp/src/error.rs:58-66`), and both `list_tools` (`client.rs:251`) and
`call_tool` (`client.rs:306`) respond to a retriable error with `self.reconnect().await`, which calls
`do_handshake()` (`client.rs:107`) and **spawns a fresh stdio child** — up to `max_reconnect_attempts` (3).
A disabled extension would resurrect itself.

**Two windows, so two checks.** A seal checked only at the entry of `reconnect()` closes the *post-teardown*
window but not the *in-flight* one. `reconnect()` checks nothing after its first line: it takes and
**releases** the service lock to drop the old service (`client.rs:210-212` — the guard lives for one
statement), sleeps the backoff (`:218`), then `do_handshake()` spawns the child (`transport.connect()`,
`:110`) and installs it with `*self.inner.service.lock().await = Some(running)` (`:137`) — with no seal check.
`disconnect` takes the same lock (`:166`), which is **free** during the sleep and the handshake. So if a
reconnect entered *before* the T0 CAS — the ordinary reason an owner flips the switch: a hung server times out
(`Timeout(_)` is retriable, `error.rs:58-66`, after `request_timeout` 30 s, `client.rs:41`) and `call_tool`
enters `reconnect()` — and the T3 drain expires while that reconnect is mid-handshake (a slow cold start, the
npx-style server, need only outlast the 10 s drain while staying under the sandbox timeout that would drop
the future — 60 s / 300 s defaults, `daemon_config/execution.rs:35`/`:68`), then T4's `disconnect` finds the
lock free and `service == None`, sets `closed`, returns; the handshake then completes and installs a **live
child into the sealed client**. Nothing closes it afterwards: the registry entry is gone (T1), the supervisor
never calls `disconnect` on that incarnation again, and `McpClient` has no `Drop`. The disabled server is
running. Guarantee (b) of §10 case 7 ("the gate refuses before either") does not cover this — the reconnect
was legitimately under way before the gate flipped.

The fix, ~15 lines in `openalpaca_mcp`, **checking the seal at both points**:

```rust
pub(crate) struct ClientInner {
    ...
    pub(crate) closed: AtomicBool,          // NEW
}

// in disconnect(), before taking the service lock:
self.inner.closed.store(true, Ordering::SeqCst);

// (1) first line of reconnect() — the cheap early exit:
if self.inner.closed.load(Ordering::SeqCst) {
    return Err(McpError::TransportClosed);   // terminal; no rebuild
}

// (2) in do_handshake(), at the INSTALL point, under the service lock — the one that closes the race.
//     Optionally also right after `transport.connect()` to skip the initialize round-trip; not sufficient alone.
let mut guard = self.inner.service.lock().await;
if self.inner.closed.load(Ordering::SeqCst) {
    drop(guard);
    let _ = running.close_with_timeout(Duration::from_secs(2)).await;   // kill what we just spawned
    return Err(McpError::TransportClosed);
}
*guard = Some(running);
```

Why (2) is sufficient: `disconnect` stores `closed` **before** taking the lock, and the install reads `closed`
**while holding** it. Either the install runs first — then `disconnect` takes the lock after it, finds
`Some(running)` and closes it normally — or `disconnect` runs first — then the install sees `closed == true`
and closes the child it just spawned. There is no interleaving in which a child survives. `connect()`
(`client.rs:91-104`) builds a fresh `ClientInner` with `closed == false`, so a re-enable is unaffected.

Belt and braces — the T0 gate already refuses before `call_tool` is reached — but S2 should hold by
construction, not only by gate coverage. A re-enable builds a **new** `McpClient` via `connect`, so the flag
is never reset — and that new client is stamped with the new `generation` (§3.0 Fact 3), so a snapshot still
holding the sealed one is refused at the gate as `Stale` before it can even reach the seal. C2's test for
this window is named in §12: start a disable while a reconnect is sleeping, let the drain expire, let the
handshake complete, assert no live child and that the sealed client's next call still returns
`TransportClosed`.

**T5 — COMMIT (in memory; no file I/O).** State `Disabled`; ledger clears this extension's warn-dedup
entries; emit `SystemEvent::ExtensionStateChanged` + `ServerEvent::ExtensionStateChanged` (both variants and
their bridge arm are introduced in **C2**, the first commit that has a transition to announce — §12; the two
*capability* events are C4's). The bit was written at W, so T5 cannot fail on disk and needs no rollback
story. (Rev 4 said "persisted" here and at E5 while §2 and §8 said the write came first; both verbs are now
write-first, end to end.)

**T5-gone (MCP, declaration deleted).** When `reconcile_all()` finds a server that was up but whose
`[servers.<n>]` block is no longer in `mcp.toml` (§4.1, last column; §5.1), it runs **T0–T4 exactly as above
and then, instead of T5: drops the record and emits `ExtensionStateChanged` with `state: "removed"`** (the
event's `state` field is a string; the row simply disappears from `GET /v1/extensions`). **No file write is
attempted.** It could not succeed: `toml_edit` index-assignment on a missing table would auto-create
`[servers.<n>]` holding only `enabled = false`, and the §2.1 writer's mandatory re-parse would reject it —
`McpServerConfig` is `#[serde(tag = "transport")]` (`tools/mcp/config.rs:46-47`) and the synthesized table has
no tag. The bit left with the block, which is the correct outcome for "the declaration is the toggle".

**Persistence failures off the route path — one rule.** Only the route has a requester to hand a `500` to.
Wherever a supervisor performs a write with no HTTP response behind it — the watcher's `reconcile_all`, the
reaper, the plugin `Orphaned` bookkeeping, `approve`/`deny` invoked internally — a failed write is
logged at `error!(ext, path, error, "extension store write failed; in-memory state kept")`, the in-memory
state is **kept as computed** (never rolled back to a state the disk does not match either), the row's
`detail` carries the error, and the write is **retried on the next `reconcile`/`reconcile_all`** of that
extension. Nothing is ever left in `Enabling`/`Disabling` because of a disk error: those two states are
exited by the CAS at E5/T5, which is memory-only under write-first.

**W-deny / T5-deny (plugins, `deny` from `Enabled` only).** Same write-first shape: at **W** the route writes
`approved = Some(false)`, `approved_at = Some(now)`, `capabilities = []` into the entry **leaving `enabled`
untouched** (a read-modify-write of the existing entry, §5; a failed write is `500` and no CAS), then runs the
same T0–T4, and at T5 commits state `Unapproved{Denied}` in memory and emits. The bit is deliberately not
cleared — a later `approve` restores the owner's last toggle position (§8). Every other `deny` cell in §4.1
starts from a state with nothing loaded and is W plus the memory commit alone.

### 3.3 ENABLE — the exact sequence

**W — PERSIST (route path; before E0).** `set_enabled(name, true)` writes `enabled = true` first, with the
same rule as §3.2 W: a failed write is `500` with the current row and no CAS. The boot path and the watcher
path skip W (the bit is already on disk — it is what they read). `approve` on an `Unapproved` row with the bit
set (§4.1) writes its consent entry first for the same reason, then enters E0.

**E0 — CAS.** `ledger.begin(ext, Enabling)` — the CAS, **and it increments the record's `generation` and
returns it** (§3.0 Fact 3); every handle E2–E4 build carries that number. `check()` still `Blocked`; nothing is
callable yet.
**Enable-on-already-Enabled is a CAS failure returning 200 with the current record — never a reload.**
This is not politeness: today a redundant `enable_plugin` re-runs `try_load_plugin`, which overwrites the map
entry with a fresh `PluginState { capability_provider_handle: None, .. }` (`manager.rs:262-278`), so
`remove_capability_provider` is never called for the old handle. For any plugin that declares **virtual
capabilities** — the provider is registered only when `manifest.capabilities.virtual_.provides` is non-empty
(`manager.rs:469-482`) — **every redundant enable permanently leaks a duplicate `PluginCapabilityProvider`**
into the registry; for a plugin without them it re-registers the tools and appends duplicate
`capability_index` edges (E4's remove-first rule is what stops that). Either way the old child dies only
incidentally via `kill_on_drop(true)` (`process_pool.rs:44`).

**E1 — CONSENT (plugins only).** `is_approved()` must be `Some(true)`; `None` parks at
`Unapproved{NeverSeen}`, `Some(false)` at `Unapproved{Denied}`. Then the **drift check**: compare
`manifest.capabilities.provides` against the list recorded at approval time (`permission_gate.rs:66` — written
since day one, **never read back**). If the manifest has grown, park at
`Unapproved { CapabilitiesGrew { added } }` and require a fresh approve. All three exits leave `Enabling` for
`Unapproved{..}` with **the disposition bit still `true`** (§4.1, `Enabling` row) — the owner asked for it on;
consent is the only thing missing, and the row says so. MCP servers have no consent gate: writing a server
into your own `config/mcp.toml` *is* the consent.

**E2 — BRING UP.** Plugin: config check (`manager.rs:316`) → spawn → `initialize`. MCP: **re-resolve**
`bearer_env` / `api_key_env` from process env (`services/mcp.rs:215-240` — resolved at boot only today, so a
re-enable is also how a rotated credential takes effect without a restart) → `McpClient::connect` under the
per-server timeout. On failure: classify (§4), set `Failed{..}`, **stop** — nothing has been published, so
there is nothing to unwind.

**E3 — DISCOVER.** `list_tools`; plugins additionally `skill/info`, `agent/info`.

**E4 — PUBLISH, remove-before-register.** For each tool: `tool_registry.remove(name)` **then** `register(tool)`.

> Remove-first is **mandatory**, not defensive. `register` (`registry/mod.rs:229`) overwrites `tools` but
> **appends** to `capability_index` (`:247-261`) with no dedupe, while only `remove` (`:267`) scrubs. An
> enable/disable/enable cycle that skips the remove leaks duplicate index edges — and reads survive it
> (`tools_for_capabilities` dedupes via a `seen` set at `:567`), so it would never fail a test, only rot.
> Codified as `ToolRegistry::replace(tool)` = remove-then-register, used on every reload path.

Then register the capability provider, the skills, the agent templates.

**E4b — PARTIAL-LOAD UNWIND.** `register` can still return `Err` on name validation (`registry/mod.rs:231-242`)
mid-loop, and discovery RPCs can fail after tools registered. If any step after the first successful
registration fails: remove every name registered in this attempt, remove any provider/skill/template already
added, tear down the process/connection, and fall into `Failed{..}`. **The registry never holds a half-loaded
extension.**

**E5 — PUBLISH STATE.** `ledger.restore(ext)` (removes this extension from every tombstone set — string
caps and plugin virtual caps alike, §7.2) and `ledger.record_tools(ext, names)` (replaces the retained
`tool_names` wholesale; the record's `generation` was already set at E0, so from here the hit arm accepts
exactly the handles this load built); state `Enabled`; emit. **No file I/O** — the bit was written at W.
Tools become callable only here — `E2 before E4` means we never publish a capability we cannot serve.

### 3.4 Re-enable that cannot connect

`enabled` **stays true** — it was written at W, before E0, so the owner's intent is durable and orthogonal to
whether the thing works; a restart reads `(approved, true)` and tries again. State becomes
`Failed { reason, detail, since }` with nothing registered.

**The route returns `200` with the resulting record, not a 4xx/5xx.** Recording the intent succeeded; the
connection outcome is a separate fact carried in the body. A 5xx would tell the GUI the toggle did not take and
the user would flip it again.

GUI renders: toggle **ON**, an `asks` or `warn` tag, the reason, and a CTA. There is **no automatic retry**
ladder in v1: there is no health poller anywhere today (`reconnect` is lazy and call-triggered only), and
building supervision means answering backoff, flapping and auto-disable-after-N — separate work. Recovery is
`POST …/enable` again, which is idempotent and re-runs E0–E5 from `Failed`. This is already an improvement over
`McpClient`'s own budget, where `ConnectionState::Failed { ReconnectExhausted }` (`client.rs:186-195`) has no
path back short of a daemon restart. What *is* built is detection of a running extension that dies — §3.6 —
which is lazy and needs no poller.

### 3.5 Daemon shutdown

`Extensions::shutdown_all()` calls each supervisor's `shutdown_all()`, which runs T2–T4 for every `Enabled`
extension on the shutdown path. This closes an existing leak: nothing calls `unload_plugin` at shutdown today,
and `kill_on_drop` does not fire on `process::exit`, so plugin children can outlive the daemon.

### 3.6 Runtime death — how `Enabled → Failed{Crashed}` actually happens

S3 asks for a `Crashed` state and §4.1 promises `Enabled/Enabling --process/conn dies--> Failed{Crashed}`.
Nothing in the code detects that today: `McpClient` exposes no state accessor (`client.rs` has `server_info`,
`protocol_version`, `ping`, `disconnect`, `list_tools`, `call_tool` and nothing that reads
`ConnectionState`, which is `pub(crate)`, `lifecycle.rs:10`); `PluginStatus::Crashed` is constructed only in
a test (`manager.rs:1072`, under `#[cfg(test)]` at `:1055`); `last_health` is written once at load
(`manager.rs:499`) and never read. **Decision: lazy detection at the two places a typed transport error is
already in hand, plus a non-blocking `try_wait` sweep on read — no poller.**

1. **MCP — the registry's `Mcp` arms** (`registry/mod.rs:334-355` in `execute`, `:384-403` in
   `execute_with_context`; `:333` is the `Plugin` arm). The `Err(e)` branch already has the typed `McpError`
   **and the backend's `generation`**. `call_tool`'s retriable branch (`client.rs:306`) runs `reconnect()` for
   a live client — that is the intended in-session recovery for an `Enabled` server (the `closed` seal of T4b
   is set only by `disconnect`, never here) — and gives up with `McpError::ReconnectExhausted(n)` once the
   attempt counter passes `max_reconnect_attempts` (`client.rs:180-195`). On **that** variant, and only that
   one, the arm calls `self.extensions.mark_failed(ExtensionId::mcp(server_name), *generation,
   FailureReason::Crashed, e.to_string())`. No string matching; the client's own terminal state is the trigger.

   *What `reconnect` really does for a stdio server, so nobody expects more.* Each `reconnect()` entry
   increments `attempt_counter`, drops the old service, sleeps one backoff and runs **one** `do_handshake()`
   (`client.rs:221`) — which for a stdio transport **spawns the child again**. A successful handshake resets the
   counter to zero (`:139`), and so does any successful call (`:240`, `:294`). Consequences: a child killed
   out-of-band while its command is still runnable is **transparently respawned** on the next call and the
   call succeeds — the row correctly stays `active`, because the server *is* running again. A handshake that
   fails propagates its own error through `self.reconnect().await?` and ends that call; the counter is not
   reset, so `ReconnectExhausted` is reached on the **fourth consecutive** `reconnect()` entry with no
   successful handshake or call in between (`attempt > max` with `max = 3`, `:186`). So a dead MCP server whose
   command can no longer start reads `active` until four consecutive calls have failed to re-handshake — not
   "until its first failed call" (rev 3's wording) — stated, not hidden. C2's crash test is written to that
   sequence (§12).
2. **Plugins — the proxies in `openalpaca_plugins`.** The registry's `Plugin` arm sees only a `String`
   (`tool_bridge.rs:47` formats the error) and emits **no `warn!`** (`registry/mod.rs:333` returns it
   verbatim, unlike the two `Mcp` arms), so both detection *and* the log line live where the type is:
   `PluginToolProxy::execute` (`bridge/tool_bridge.rs:30-47`), `PluginSkillBridge`
   (`bridge/skill_bridge.rs:61-65`) and `PluginAgentBridge` are constructed by `PluginManager`
   (`manager.rs:831`, `:419`, `:452`), which holds the registry (`main.rs:337`) and so the ledger; each takes an
   `Arc<ExtensionLedger>` **and the load's `generation`**, and on `PluginError::ChannelClosed |
   PluginError::ProcessCrashed` (`plugins/src/error.rs:18`, `:12`; raised at `stdio_channel.rs:95`, `:120`,
   `:128`, `:154`) it first logs `warn!(plugin, tool, generation, error, "plugin transport failure")` — so a
   plugin transport failure is never log-silent — then calls `mark_failed(ExtensionId::plugin(id),
   generation, Crashed, e.to_string())`. If the generation is not the record's current one the proxy is a
   stale handle from a previous load (§3.0 Fact 3): `mark_failed` is a no-op and the proxy returns the `Stale`
   refusal instead of the raw channel string. The plugin-agent loop's step-boundary `ledger.check()` (§3.2
   T3(b)) then stops the loop on its next step.
3. **The read-side sweep.** `PluginManager::reconcile` and `list()` (i.e. every `GET /v1/extensions`) run
   `process.try_wait()` (`process_pool.rs:177`, non-blocking) on each `Enabled` plugin and `mark_failed` any
   that has exited (with the record's own current generation — this is the live process, by construction).
   `try_wait` takes `&mut self` while `list_plugins` holds only a **read** lock on `plugins`
   (`manager.rs:694-695`, `RwLock<HashMap>`), so the sweep takes the **write** lock for the duration of the
   `try_wait` pass — one non-blocking syscall per plugin, microseconds — and drops it before building the
   rows. **Nothing `.await`s under that write lock**: `try_wait` is synchronous and `mark_failed` is a CAS plus
   an unbounded-channel `send`, so a `GET /v1/extensions` can be delayed by the lock only for as long as
   another holder keeps it — and the one long holder today, `try_load_plugin`'s spawn/`initialize` path, holds
   the `plugins` lock only for its map insert/update (`manager.rs:262-278`, `:288-292`), not across its
   awaits. The sweep cannot stall the list route indefinitely. That is the whole "monitor": the row is correct
   whenever someone looks at it. MCP has no equivalent (an idle stdio child that died is not observable
   without a request), so it relies on item 1's bound.

`mark_failed(ext, generation, reason, detail)` is a CAS `Enabled → Failed{Crashed, detail, since}` **guarded
twice**: it is a no-op from any state other than `Enabled`, so a crash observed during `Disabling` does not
fight the toggle; and it is a no-op unless `generation` is the record's current one, so a handle from a
previous load cannot flip the incarnation that replaced it (§3.0 Fact 3 — without this guard a stale
snapshot's `ChannelClosed` would tear down a healthy plugin). From the instant it succeeds the gate refuses
every call to the extension with the S4 wording for `Failed` (*"… is not running (crashed: <detail>). Retry
it in Settings → Extensions"*), and `extension_tool_defs` drops its tools. The ledger then sends
**`(ExtensionId, generation)`** down a **reaper channel** (`tokio::sync::mpsc::UnboundedSender<(ExtensionId,
u64)>`, one per kind, registered by each supervisor via `ledger.on_crash(kind, tx)` — the sender slot is
interior-mutable, an `OnceLock` per kind, so `ToolRegistry::new()` keeps its arg-free signature and the
supervisors register after the registry exists, §7.1).

**The reaper re-checks under the mutex; it never writes state.** The per-kind reaper is one sequential task,
and its MCP T4 can block for up to `request_timeout + 1 s` (the detach rule above). While it is tearing down
server A, server B's crash sits in the channel with B's row already reading `failed/crashed`; the owner
clicks Retry; `enable` takes B's mutex first (`Failed{*}` + `enable` → `Enabling`, §4.1), bumps the
generation, publishes load N+1 and commits `Enabled`. If the reaper then dequeued B and ran T1→T2→T4
unconditionally it would unpublish load N+1's tools, kill the live process and leave the record `Enabled`
with nothing registered — the exact stale-actor teardown the generations were introduced to prevent. The
mutex prevents *interleaving*, not *reordering*. So the reaper, having taken the per-extension mutex,
**re-reads the record and proceeds only if `state == Failed{Crashed}` and the record's generation equals
the one in the message**; otherwise it logs `debug!(ext, generation, "crash reap superseded")` and drops the
message. When it proceeds it runs **T1 → T2 → T4** — unpublish, dependent scan + cron notice (T1 step 3,
worded *"stopped running (crashed: …)"*), withdraw contributions, close/kill and seal — but **not T5**, and
it writes no state at all: the disposition stays `true`, the state was set to `Failed{Crashed}` by
`mark_failed` and the reaper leaves it there, so the row renders toggle ON + `crashed` + Retry (§9.2) and
`resolve_capabilities` classifies the extension's capabilities as `withheld` with the crash as the reason.

The other ordering — a route `disable` that beats the reaper (`Failed → Disabling` is a legal T0, §4.1) — is
covered by the same re-check: by the time the reaper takes the mutex the record reads `Disabling` or
`Disabled`, not `Failed{Crashed}`, so it drops the message. And T1–T4 are **idempotent** anyway (T1 iterates
the retained `tool_names` and `remove()` of an absent name is a no-op; T2's `withdraw` is a set insert; T4's
`disconnect`/`kill` on an already-closed handle is a no-op), with T1 step 3 firing only on a non-empty
withdrawn set — so even a double pass could not emit a second `ExtensionCapabilityWithdrawn` (§7.3's "one
transition, one announcement"). Retry is `POST …/enable`, which re-runs W, E0–E5 from `Failed` exactly as
§3.4 (and bumps the generation). The reaper is the one path that runs T1–T4 without a route or a reconcile
behind it; it holds the same mutex, so it cannot interleave with either, and the re-check is what makes its
*late* arrival harmless. C2 and C3 each carry the test: `mark_failed` → `enable` before the reaper runs →
reaper runs → load N+1's tools and process are intact and the row reads `enabled` (§12).

What this deliberately does not do: no backoff, no automatic restart, no flap counting. `PluginStatus::Crashed
{ backoff_until }` (`manager.rs:44-47`) implied a restart policy that was never implemented; `FailureReason::
Crashed` carries none, and the GUI copy says "Retry", not "retrying".

---

## 4. States

One enum across both kinds. Plugins and MCP servers answer the same questions; only **reachability** differs,
which a single enum expresses for free. Two enums would duplicate every edge and force the GUI to switch on
kind before it can render a row.

```rust
/// Persisted, per extension. The owner's toggle.
pub struct Disposition(pub bool);     // `enabled` in mcp.toml / .permissions.toml

/// In-memory only. Observed reality. Never persisted.
pub enum ExtensionState {
    Enabled,                                       // loaded, tools published, calls pass
    Disabled,                                      // owner turned it off — S2: unloaded
    Unapproved { reason: UnapprovedReason },       // consent gate not passed (plugins)
    Failed { reason: FailureReason, detail: String, since: DateTime<Utc> },
    Orphaned,                                      // PLUGIN-ONLY: .permissions.toml entry whose directory is gone
    // transient, never persisted; reported LITERALLY as "enabling" / "disabling" (§8, §9.2, §4.3 shim):
    Enabling,
    Disabling,
}

pub enum UnapprovedReason { NeverSeen, Denied, CapabilitiesGrew { added: Vec<String> } }

pub enum FailureReason {
    NeedsAuthorization,                    // actionable
    NeedsConfig { missing: Vec<String> },  // actionable
    ConfigInvalid,                         // actionable (bad declaration)
    Unreachable,                           // not actionable — retry
    Crashed,                               // not actionable — retry
}

impl FailureReason {
    /// Drives the GUI's tag tone and CTA. The owner's own distinction, made first-class.
    pub fn actionable(&self) -> bool {
        matches!(self, Self::NeedsAuthorization | Self::NeedsConfig { .. } | Self::ConfigInvalid)
    }
}
```

**The load-bearing invariant — one-directional, consent pre-empts the switch.** `disposition` and `state`
are **independent** and every API row carries both (`enabled` and `state`). The rules:

- `Disabled` and `Disabling` ⇒ `disposition == false` — the bit is written at W, **before** the T0 CAS, so a
  record is never `Disabling` over a file that still says `true`. The converse does **not** hold.
- `Enabled`, `Enabling` and `Failed{*}` ⇒ `disposition == true` — written at W before E0, so a bring-up that
  fails from `Disabled` lands in `Failed` with the bit already `true`, and a restart reads `(Some(true), true)`
  and tries again (§3.4). A crash mid-`Disabling` restarts into `Disabled` (the bit was already `false`),
  which is what the owner asked for.
- `Unapproved{*}` and `Orphaned` may carry **either** bit. The row's `enabled` field reports it; the state
  word does not encode it.
- **Two rows have no readable bit, and say so.** When `.permissions.toml` is unreadable (§5.1) every plugin
  parks at `Failed{ConfigInvalid, "permissions store unreadable"}`, and a malformed `config/mcp.toml` yields
  the `{id: "config/mcp.toml", state: Failed{ConfigInvalid}}` pseudo-record. Neither has a disposition anyone
  can read, so the `Failed ⇒ bit == true` rule does not apply: the row reports **`enabled: null`**, and
  `enable`/`disable`/`approve`/`deny` on it return **`409 {"error":"store_unreadable"}` without entering a
  transition** — the W write is refused up front, so no CAS is taken and there is nothing to roll back; the
  writer would only have failed against the same unreadable file. The only fix is repairing the file; the row's `detail` carries the parse error.

Why one-directional: consent is evaluated **before** the switch (§6.2 #7's gate, E1's position in the enable
sequence, §9.2's no-switch rendering). A plugin that has never been approved is `Unapproved` whether the owner
has pre-set its toggle on or off, and that pre-set must survive a restart: `disable` on an `Unapproved` plugin
clears the bit and **stays `Unapproved`**, so the row reads `unapproved / enabled: false` before and after a
restart — the boot gate reads the same two facts and lands on the same state. (Rev 2 stated the invariant as an
iff and then contradicted it in three cells; under the iff, disabling an unapproved plugin rendered
`disabled` in-session and `unapproved` after a restart.) `Unapproved` and `Failed` are *"blocked"*, not
*"off"* — which is exactly what lets the GUI render Approve/Deny for an unapproved plugin instead of a switch,
and lets `approve` decide between *start now* and *stay off* by reading the bit.

### 4.1 Transition table

Identical for both kinds **except the last column.** Consent edges (`approve`/`deny`) and `Unapproved` are
plugin-only. `Orphaned` is plugin-only too, and the "declaration gone" column is where the two kinds part ways:
for a plugin the declaration (the directory) and the disposition (`.permissions.toml`) are different files, so
the disposition can outlive the declaration and is preserved as `Orphaned` (§5.1). For MCP the declaration *is*
the disposition — one `[servers.<n>]` block in `mcp.toml` — so when it is deleted the bit goes with it: the
next `reconcile_all()` (edge case 15) diffs desired against actual, runs **T0–T4 with no file write** if the
server was up (§3.2 T5-gone — there is no block to write into, and the writer's re-parse would reject a
synthesized one), and **drops the record**. There is no MCP `Orphaned`; the row simply disappears from
`GET /v1/extensions`.

Every cell that changes the bit says so; a cell that does not mention the bit leaves it untouched.

| from | `enable` (bit := true, written at W **before** E0) | `disable` (bit := false, written at W **before** T0) | `approve` | `deny` | bring-up ok | E1 consent fails | E2–E4 fail | process/conn dies (§3.6) | declaration gone — **plugin** | declaration gone — **mcp** |
|---|---|---|---|---|---|---|---|---|---|---|
| `Disabled` (bit=false) | → `Enabling` | 200 no-op | `Disabled` (consent recorded; bit stays false, so nothing starts) | `Unapproved{Denied}` (bit stays false) | — | — | — | — | → `Orphaned` | record dropped |
| `Unapproved{NeverSeen\|Denied\|CapabilitiesGrew}` (plugin-only; bit=either) | stays `Unapproved`; bit := true; 200 w/ reason | stays `Unapproved`; bit := false; no teardown (nothing is loaded) | → `Enabling` if bit=true, else → `Disabled` | → `Unapproved{Denied}` | — | — | — | — | → `Orphaned` | n/a |
| `Enabled` (bit=true) | 200 no-op (**never reload**) | → `Disabling` | stays `Enabled`; consent **re-recorded** against the current manifest (a disk write, no load) | → `Disabling` then `Unapproved{Denied}` (bit stays true; T5-deny) | — | — | — | → `Failed{Crashed}` via `mark_failed` (current generation only); reaper runs T1–T4 after re-checking state + generation under the mutex | → `Disabling` then `Orphaned` | → T0–T4 (no write) then record dropped |
| `Failed{*}` (bit=true) | → `Enabling` (retry) | → `Disabling` | stays `Failed`; consent re-recorded, **never a load** — Retry is `enable` | → `Unapproved{Denied}` | — | — | — | — | → `Orphaned` | record dropped |
| `Enabling` (bit=true) | CAS fail → 200 current | queued after commit | — | — | → `Enabled` | → `Unapproved{NeverSeen\|Denied\|CapabilitiesGrew}` (bit stays true) | → `Failed{reason}` | `mark_failed` is a no-op here; E2/E3 error → `Failed{reason}` | — | — |
| `Disabling` (bit=false — already written at W) | queued after commit (its own W runs first) | 200 no-op | — | — | — | — | — | `mark_failed` no-op | — | — |
| `Orphaned` (plugin-only; bit=either) | 409 | 409 (bit preserved) | 409 | 409 | — | — | — | — | declaration returns → previous disposition and consent honoured; `DELETE` (§8) removes the entry | n/a |

Reading the plugin boot gate (§6.2 #7) off this table: `approved != Some(true)` → `Unapproved{NeverSeen|
Denied}` **whatever the bit**; `(Some(true), false)` → `Disabled`; `(Some(true), true)` → `Enabling` → E1 drift
check (which can still park at `Unapproved{CapabilitiesGrew}`) → E2–E5.

**`Enabled --deny--> full unload` is a behaviour fix, not a refinement.** Today `deny_plugin`
(`manager.rs:601-618`) writes the denial and sets `status = Disabled` but **never unloads**: the child keeps
running, its tools stay in the registry, its skill stays in the catalog, its agent template stays in the
registry. The plugin reports `"disabled"` while remaining fully usable until the next daemon restart. That is a
live hole in the approval gate; routing `deny` through the same teardown as `disable` closes it.

### 4.2 Recommendation — "needs authorization" is a **reason code**, not a state

**Decision: `FailureReason::NeedsAuthorization` inside `Failed`, with a derived `actionable: bool` on the API
row.** The owner grouped it with the crash family; that grouping is right about the *lifecycle* and needs to be
refined only about the *affordance*.

**Why inside `Failed`:** it occupies the identical lifecycle position as any other bring-up failure — disposition
is on, nothing is running, nothing is registered, and every edge into and out of it is the same. Promoting it
to a peer state duplicates six rows of the table above to encode a distinction that changes no transition.

**Why a first-class reason code and not free text in `detail`:** the user's *next action* differs, and the UI
must be able to say which. `Unreachable` → "check the server is up, Retry". `NeedsAuthorization` → "Authorize",
rendered as an actual affordance. A segfaulted child and an expired token are both `Failed`, but only one is
fixable from the settings row.

**Why `actionable()` rather than the GUI matching on reason strings:** it makes the tone mapping total, so a
future reason code cannot silently render without a CTA. `NeedsConfig` is the proof that the reason axis is the
extensible one — it already exists as `PluginStatus::NeedsConfig` (`manager.rs:40`).

**Detection, honestly bounded.**
- **HTTP MCP:** missing `bearer_env`/`api_key_env` → `resolve_http_auth` (`services/mcp.rs:215-240`) *already*
  returns a distinct `Err("missing env var '<X>' for bearer_env on server '<Y>'")`, today flattened into a
  generic `McpServerStatus::Failed` and thrown away. Also HTTP 401/403 on handshake.
- **Plugins:** the `initialize` / JSON-RPC error may carry `error.data.reason == "needs_authorization"` and an
  optional `hint`. Absent that, it degrades to `Failed{Unreachable}` or `Failed{Crashed}`.
- **Stdio MCP:** a server that exits non-zero on an expired token is **indistinguishable** from one that
  crashed. It classifies as `Unreachable`.

Classification lives in one function, `classify_bringup_failure(&McpError) -> FailureReason`, defaulting to
`Unreachable` when unsure. A misclassification costs a wrong button label, never a wrong lifecycle. The GUI copy
must not promise more than the classifier knows.

### 4.3 Dead code this resurrects or retires

- `McpServerStatus::Disabled` (`tools/mcp/client_set.rs:18`) is currently **never constructed anywhere** — the
  disabled branch at `services/mcp.rs:50-53` is a bare `continue` that builds no summary. `McpServerStatus`
  and `McpServerSummary` are replaced by the ledger record, which is retained rather than dropped
  (`services/mcp.rs:55-61` reads the summary once for a boolean and discards it, which is why the daemon
  cannot enumerate its own MCP servers after boot).
- `PluginStatus` (`manager.rs:34-52`) collapses into `ExtensionState`. `Stopped` (`:51`) and `Crashed`
  (`:44`) are constructed only in the `Display` test (`:1065` and `:1072`, under the `#[cfg(test)]` at
  `:1055`). `Loading` disappears: today a plugin whose spawn or
  `tools/list` fails returns `Err` and leaves the entry pinned at `Loading` **forever** (`manager.rs:339`
  onward never update status on the error path), so `GET /v1/plugins` reports a broken plugin as `"loading"`
  indefinitely. Under the ledger a failure is a state transition, not a return-without-update.
  **Between C3 and C7** `GET /v1/plugins` still serialises `PluginInfo.status: String`
  (`routes/plugins.rs:49`, built by `state.status.to_string()` at `manager.rs:701`) and the GUI's
  `statusWord` (`PluginsSection.tsx:31-34`) parses that vocabulary, so C3 keeps the words alive through a
  `legacy_status_word(&ExtensionState) -> String` shim on `PluginInfo`: `Enabled → "running"`, `Disabled →
  "disabled"`, `Unapproved{*} → "waiting-approval"`, `Failed{NeedsConfig{missing}} → "needs-config (a, b)"`,
  any other `Failed → "crashed: <detail>"`, `Enabling → "loading"`, `Disabling → "stopped"` (the Display
  strings at `manager.rs:54-66`). The shim is deleted with the route in C7. The Plugins panel therefore
  keeps working between C3 and C7 — "tree green" means the GUI too, not only cargo.

---

## 5. Where state lives

| what | where | why | restart |
|---|---|---|---|
| **MCP disposition** | `config/mcp.toml` `[servers.<n>] enabled` (`tools/mcp/config.rs:62`, `:75`) | **The declaration IS the toggle** — one place to look, no precedence rule, no decoy field. It already ships, is already honoured at boot, and is documented in the file. Hand-editable, git-diffable, and it survives `delete state/ = factory reset` (api-fix-plan §1.1) — an owner who turned off the GitHub server does not want a factory reset to turn it back on. | Read by `reconcile_all()` **before** any connect. A disabled server is parked as `Disabled` with a **listable** record — not the bare `continue` of today, which also silently depresses the boot log's `connected/total` ratio (`services/mcp.rs:49` counts it, `:59` does not). |
| **Plugin disposition** | new `enabled` on `PermissionEntry`, `<plugins root>/.permissions.toml` | Same file already carries the per-install owner decision, in the directory the user drops the plugin into, with the same lifetime as the install, outside `state/`. Serde-defaulted, so every existing file reads as enabled. | Read in `try_load_plugin`'s step-2 gate (`manager.rs:284`), which becomes a 2×2 on `(approved, enabled)`. |
| **Plugin consent** (`approved`, `approved_at`, `capabilities`) | same `PermissionEntry`, **shape changed additively** (`permission_gate.rs:11-17`): `approved: Option<bool>` and `approved_at: Option<String>`, both `#[serde(default)]` — `None` = *pending*; `capabilities` keeps its default. Old files parse unchanged (`approved = true` → `Some(true)`). `is_approved()` becomes `table.get(name).and_then(\|e\| e.approved)` — `None` for a missing entry **and** for a decision-less one, so the §6.2 #7 gate reads the same `NeverSeen` either way. `approve`/`deny` become read-modify-write of the existing entry (today both **insert a fresh entry**, `:61-68`/`:77-84`, which would reset `enabled` to its default); `set_enabled()` creates a decision-less entry when none exists. | Consent is a security artifact scoped to the binary it authorises; it must travel with the plugin directory. It stops being *overloaded*, which is the fix — and it stops being *binary*, which is what lets a pre-set toggle on a never-approved plugin be stored at all (§4). | The recorded `capabilities` list (`permission_gate.rs:66`) is now **read back** for the E1 drift check. |
| **Runtime state — bookkeeping**: `ExtensionState`, retained `tool_names`, tombstone index, in-flight counters, warn-dedup set, reaper senders | **in memory**: `ExtensionLedger`, held as `Arc<ExtensionLedger>` inside `ToolRegistry` (`registry/mod.rs:147`) | Observation, not intent. A persisted `Crashed` read back after a restart is a lie — it would render "broken" before anything was tried. Keeping observation in memory and intent on disk is what lets the two disagree *honestly*. It lives inside `ToolRegistry` because that is the one object every execution path already holds, and `Clone` shares it for free. | Rebuilt from zero. Every extension re-enters through the same enable path a GUI toggle takes — boot is just its first invocation. |
| **Runtime state — handles**: the `Arc<McpClient>` per server; the `PluginProcess` per plugin | **in memory**, in the owning supervisor (`McpSupervisor`, `PluginManager.plugins[..].process` — the latter already so, `manager.rs:494-500`) | The ledger stays pure bookkeeping in `openalpaca_core`, which does not depend on `openalpaca_mcp`'s client type for lifecycle and must not spawn or kill anything. Teardown (§3.2 T4), the `try_wait` sweep (§3.6) and the file writers are supervisor work. | Rebuilt by the supervisor's first `reconcile_all()`. |

**No DB migration. No new table. Migration head stays at 034** (`crates/openalpaca_storage/src/migrations/034_drop_context_compaction_log.sql`).

### 5.1 Reconciliation rule when declaration and disposition disagree

**They cannot, for MCP.** The declaration *is* the store: one file, one field, one reader. If a server is
deleted from `mcp.toml`, its bit goes with it — which is correct, and is the main structural advantage of this
storage choice over a shadow table. The supervisor's only job on that path is to stop what was running:
T0–T4 with **no** write (§3.2 T5-gone), then drop the record. No `resync` verb is needed and none is provided.

**For plugins there is exactly one asymmetry**, because the declaration (the plugin directory + `plugin.toml`)
and the disposition (`.permissions.toml`) are different files:

| situation | rule |
|---|---|
| directory present, no `.permissions.toml` entry | **Nothing is written.** The in-memory record is `disposition = true` (the serde default an absent entry would read as), `consent = pending` → `Unapproved{NeverSeen}`. A file entry appears only when something is decided: `approve`/`deny` write a decision, and `disable`/`enable` on the unapproved row write a **decision-less** entry `{enabled = false}` / `{enabled = true}` (`approved` absent → `None`), which is what makes the pre-set bit survive a restart while the row still reads `never_seen`. A freshly installed plugin is enabled-but-unconsented, so **approving is the single action that starts it**. |
| entry present, directory gone | **Never delete the entry.** State `Orphaned`, disposition preserved. The daemon and CLI resolve config dirs differently (`bootstrap/config.rs:10` documents and `:27-32` implements the walk-up for `config/llm.toml`; `apps/openalpaca/src/commands/daemon_config_cli/mod.rs:296-299` documents the CLI order — `OPENALPACA_CONFIG_DIR`, then `app_dir()/config/daemon.toml`, then the CWD walk-up — so the CLI reaches `app_dir()` *before* the walk-up the daemon uses) and the Tauri launcher forces a third (`src-tauri/src/lib.rs:114`), so a vanished declaration is very often a path difference, not an uninstall. Deleting the entry would silently flip the extension back **on** at the next reconcile. Hidden from `GET /v1/extensions` unless `?include_orphaned=true`; GC only on an explicit user action — `DELETE /v1/extensions/plugin/{id}` (§8), which is the only verb an `Orphaned` row accepts and the only thing that ever removes a `.permissions.toml` entry. |
| directory returns | Preserved disposition and consent are honoured **at the next `reconcile_all`** — boot, or the GAP-24 install path once it exists. There is no trigger before that: nothing watches the plugins root (`watch_paths`, `main.rs:259-292`, lists `agents/`, `skills/` and the config files only) and `PluginManager::start()` scans the directory once. |
| manifest capabilities grew since approval | E1 drift check → `Unapproved{CapabilitiesGrew{added}}`. |

**`.permissions.toml` must stop failing open.** `load_permissions_table` (`permission_gate.rs:140-153`)
currently catches a parse error, warns, and returns an **empty** `HashMap` — so one malformed line loses every
approval. With `enabled` in the same file it would additionally **re-enable every integration the owner turned
off**, because every plugin would revert to the serde default. New behaviour: return `Err`; `try_load_plugin`
maps it to `Failed{ConfigInvalid, "permissions store unreadable"}` for **every** plugin; nothing loads; the
file is never overwritten so the user can repair it. Fail-closed. Those rows report `enabled: null` and every
verb on them returns `409 store_unreadable` without a transition (§4) — a write against the unreadable file
is refused at W, before any CAS is taken. Both writes go through the same
lock + temp + re-parse + rename path as `mcp.toml`, replacing the non-atomic `fs::write` at
`permission_gate.rs:165`.

**`config/mcp.toml` must stop being fatal.** Today a non-`NotFound` `LoadError` returns `Err`
(`services/mcp.rs:37-42`), `?`-propagated through `services/tools.rs:108` and `services/mod.rs:137` — the
daemon **will not boot**. Unlike `daemon.toml`, there is no fall-back-to-defaults. That is unacceptable once a
GUI route writes the file. New behaviour: log at `error`, register **one** pseudo-record
`{kind: mcp, id: "config/mcp.toml", state: Failed{ConfigInvalid}, detail: <parse error>}` so the Extensions
list shows exactly what is wrong, and boot normally.

**`seed_default_configs` must seed `mcp.toml`.** It writes only `llm.toml` and `daemon.toml` today
(`bootstrap/config.rs:73-94`), and every `watch_paths` push in `main.rs:259-292` is guarded by
`if path.exists()`. Without a seeded file the watcher never binds and hand edits never apply. Seed a
fully-commented template as a third `include_str!` — which means C2 also **adds the file**
`scripts/release/templates/config/mcp.toml` (a copy of the shipped `config/mcp.toml`): that directory holds
only `daemon.toml` and `llm.toml` today (`bootstrap/config.rs:64-66` `include_str!`s both), so without the
new file the third `include_str!` has nothing to point at.

---

## 6. Enforcement

### 6.1 Assembly-time vs execute-time

- **Execute-time is authoritative for "may this extension backend run".** The gate in `ToolRegistry`'s execute
  paths is the boundary for that question, and it holds regardless of what any surface advertised: the agentic
  loop never verifies that a called tool was on the surface it was shown (`runner/agentic_loop/mod.rs` goes
  straight to `sandbox.execute_tool`), so a stale or over-wide surface cannot reach a disabled backend.
- **Assembly-time has two roles, and only one of them is a courtesy.** As *prompt hygiene* — keeping dead tools
  out of the definitions list so the model does not burn a round on a call that refuses — it is optional and
  nothing depends on it. But on **five of the six** surfaces the assembled list is also what the
  `SandboxPolicy.allowed_capabilities` is *derived from*, and there it is security-relevant:

  | surface | policy source | empty list means |
  |---|---|---|
  | main loop | `simple_query_handler.rs:225-229` — lowercased names of the assembled defs | `policy_opt = None` (`:260`; the `let` is at `:215`) → loop stubs tool calls |
  | lead agent | `lead_agent/mod.rs:314-321` — pushes every assembled name into `allowed_capabilities` (its own comment: *"mirroring how the main loop derives its policy"*) | template allowlist alone |
  | file-based skill | `invocation.rs:281-345` — names of `tool_defs` | `tools_for_loop = vec![]`, `policy_opt = None` (`:344-345`) |
  | nested skill | `invoke_executor.rs:371-384` — names of `tool_defs` | `None` (`:372`) |
  | plugin skill | `invocation.rs:951-961` — names of the resolved defs | **allow-everything** (`security/capabilities/mod.rs:106-113`) — §6.2 #11 |
  | subagent spawn | `SandboxPolicy::from_constraints(&instance_id, &agent.constraints)` (`lead_agent/tools.rs:426`) — the template, not the list | n/a |

  So the honest statement is: assembly-time filtering is *not relied on* to keep a disabled extension's tools
  from running (the gate does that), but assembly-time **correctness** is relied on wherever a resolver's
  output becomes an allowlist — and a resolver must never hand an empty list to a policy constructor that reads
  empty as unrestricted. That is why #11 is a security item and why `resolve_capabilities` (#3) reports
  total loss explicitly instead of returning `vec![]`.

### 6.2 The ordered list

| # | site | change | if missed |
|---|---|---|---|
| **1** | `crates/openalpaca_core/src/tools/registry/mod.rs:300` (`execute`) and `:362` (`execute_with_context`) | **THE GATE — two arms around the DashMap lookup, before `validate_tool_arguments`.** **Hit arm:** derive `entry.extension_id()`; `None` → proceed (builtin / http / command, never gated); `Some(ext)` → `self.extensions.check(&ext, entry.incarnation(), ctx)?`, returning the S4 message on `Blocked` — state not `Enabled`, **or** state `Enabled` but the handle's generation is not the record's (`Blocked(Stale)`, §3.0 Fact 3) — and on success binding the returned `CallGuard` to a local that lives **across the awaited backend call** inside `dispatch(..)` (`let _guard = …; backend.call(..).await`), not one dropped when `check` returns — T3's drain counts exactly the calls whose guard is still alive. `check` takes `Option<&ToolContext>`: `execute()` has none (`:300-303`), so its dedup scope key falls back to `"global"` (§7.4). **Miss arm — mandatory, not defensive:** today a miss short-circuits at `.ok_or_else(..)?` (`:305-311` `Unknown tool`, `:369-375` `Tool '..' not found in registry`) before anything else runs, and after T1 that is exactly what a run on the **live** registry sees (§3.0 Fact 2). So on a miss, first `self.extensions.owner_of(tool_name)` — the retained `tool_names` map, §3.2 T1; if it returns `Some(ext)` whose state is not `Enabled`, run the **same** `check(&ext, None, ctx)` (identical S4 string, identical `warn!` + `ExtensionCapabilityWithheld` observability, §7.1) and return its error; only when `owner_of` is `None`, or the owner is `Enabled` (a genuinely unknown name — or a name the extension no longer offers after a re-enable; the E4→E5 window is *not* this case: there the tool is present and the hit arm blocks it on `Enabling`), fall through to the existing not-found error. **An `ExtensionId` with no ledger entry resolves to `Allow`** (§6.2a — fail-open, audited). **Refactor both into one private `dispatch(definition, backend, args, ctx)`** so the gate runs exactly **once** per call — today `execute_with_context`'s `Http \| Command \| Plugin(_)` arm delegates to `execute` (`registry/mod.rs:408-415`, `self.execute` at `:414`), which would double-take the guard. The `Mcp` arms' `Err(e)` branches additionally map `McpError::ReconnectExhausted` to `mark_failed(ext, generation, ..)` (§3.6). | The feature does not exist (hit arm); **S4 fails on the ordinary skill** — chat gets an unattributed "not found", the log gets nothing (miss arm); a run that straddles a re-enable calls a sealed client for the rest of the run (no generation). |
| **2** | `registry/mod.rs:627` `extension_tool_defs` | **Hygiene only.** Drop tools whose extension is not `Enabled` (an absent ledger entry counts as enabled, §6.2a). Signature loses `deny: &[String]` in C8 (§11). Keep the `sort_by(name)` at `:639` — it feeds prompt-cache fingerprints. Both callers build the surface **once per request** from the live registry — `lead_agent/mod.rs:154` before the snapshot at `:231`, `main_loop.rs:185` inside `main_loop_tool_set`, called once from `simple_query_handler` — and never rebuild it during the run, so this filter's only window is T0→T1 and E4→E5 (tools registered but state still `Enabling`). | Not a hole (#1 refuses). A request assembled inside one of those two windows advertises a tool that refuses on first use — one wasted round. |
| **3** | `registry/mod.rs:558` / `:584` | Both become thin wrappers over new `resolve_capabilities(caps, denied) -> CapabilityResolution { defs, withheld, partially_withheld, unknown }`, which consults the tombstone index (`capability → Set<ExtensionId>`) for **every** requested capability, not only empty lookups: an empty lookup with recorded providers is `withheld`; a non-empty lookup whose surviving tools no longer cover every recorded provider is `partially_withheld` (§7.2); an empty lookup with no record is `unknown`. No caller is forced to change. | **S4 is violated on the whole ALLOW axis.** This is the single point where a disabled extension's disappearance is invisible today: `:567` contributes nothing for an unindexed capability, with no error, no warn, no diagnostic — and with two providers of one capability, disabling one shrinks the tool set with no signal at all. |
| **4** | `registry/mod.rs:229` / `:267` | Add `replace()` = remove-then-register (§3.3 E4). In `remove`, after `names.retain(...)` (`:273`, `:280`), **drop the `capability_index` key when the vec is empty** — today a disabled extension's capabilities survive as keys mapping to `[]`, so anything enumerating index keys reports phantom capabilities. | Index corruption that self-heals only on the next `remove`; phantom capabilities in any catalog built from index keys. |
| **5** | `crates/openalpaca_mcp/src/client.rs:180` (`reconnect`), `:107` (`do_handshake`) + `:165` (`disconnect`) | `closed: AtomicBool` on `ClientInner`, set by `disconnect` before it takes the lock, checked **twice**: at `reconnect`'s entry and at `do_handshake`'s install point under the service lock (`:137`), where a just-spawned child is closed if the seal is set (§3.2 T4b). | **A disabled MCP server resurrects its own child process.** `TransportClosed` is retriable (`error.rs:58-66`) and both `list_tools` (`:251`) and `call_tool` (`:306`) call `reconnect()` → `do_handshake()` → fresh spawn; with only the entry check, a reconnect already sleeping or handshaking at T0 installs a live child into the sealed client after T4. |
| **6** | `apps/openalpacad/src/services/mcp.rs:50-53` | This function becomes `McpSupervisor::reconcile_all`. Replace the bare `continue` (`:52`) with a ledger record `{disposition: false, state: Disabled}` and **do not connect**. On the connect path, `ledger.record_tools(ext, names)`; the `Arc<McpClient>` stays in the supervisor's own map (§5 — the ledger never holds a client). Downgrade the `:37-42` parse error from fatal (§5.1). Replace the serial `for` loop (`:48`) with `join_all` over enabled servers — N unreachable servers currently cost N × 30 s of boot. The boot log's `connected/total` ratio (`total += 1` at `:49`, `connected += 1` at `:59`) is then honest, since disabled servers are listed rather than skipped. | Disabled servers stay invisible; the ledger has no `tool_names` to reverse a later disable or attribute a miss; a bad hand-edit still bricks startup. |
| **7** | `crates/openalpaca_plugins/src/manager.rs:284` | The step-2 `match self.permission_gate.is_approved(&name)` becomes the gate the §4.1 table reads off, with `is_approved()` now returning `entry.approved` (an `Option<bool>` field, §5) — so `None` covers **both** a missing entry and a decision-less entry written by `set_enabled`: `None` → `Unapproved{NeverSeen}`, `Some(false)` → `Unapproved{Denied}` — **whatever the `enabled` bit says** (consent pre-empts the switch; the bit is read from the entry when there is one, defaults `true` when there is not, and is reported on the row); `Some(true)` with `enabled == false` → `Disabled`, no spawn; `Some(true)` with `enabled == true` → `Enabling` → E1 drift check (may park at `Unapproved{CapabilitiesGrew}`, bit stays true) → E2–E5. | A plugin the owner disabled spawns its child and registers its tools, skills and templates at every boot; or (under rev 2's iff reading) an unapproved plugin's row flips between `disabled` and `unapproved` across a restart; or (under rev 3's binary `approved`) a disabled-but-unapproved plugin reads `denied` after a restart because the only entry that could hold its bit had to carry a decision. |
| **8** | `manager.rs:601` (`deny_plugin`) | Run the full teardown **before** writing the denial. | **Live security hole today** — deny leaves the child running with tools registered while reporting `"disabled"`. |
| **9** | `manager.rs:621` (`enable_plugin`) / `:645` (`disable_plugin`) | `enable` no longer calls `approve()` (`:638`); `disable` no longer calls `deny()` (`:682`). Both write only the new `enabled` bit (via `set_enabled`, creating a decision-less entry if none exists, §5) **first** — step W — and then reconcile; a failed write is `500` and no CAS (§3.2 W). | Silent consent-widening on re-enable; trust revoked by an integration toggle. |
| **10** | `skill/invocation.rs:152` and `skill/invoke_executor.rs:157` | Consume `resolve_capabilities`. Non-empty `requires_capabilities` resolving to **zero** defs → **refuse**, naming the extension. Partial → run, prefix the chat-visible warning. | Silent degradation: the SKILL.md body still tells the model to use the missing tool, so the reliable outcome is a confidently fabricated result. |
| **11** | `skill/invocation.rs:951-961` (`invoke_plugin_skill`) | **Fail closed** on total loss. | **PRIVILEGE ESCALATION.** `allowed_capabilities` is set to the resolved tool *names* (`:951-961`), and `CapabilityManager::check_agent_capability` treats an **empty allow list as ALLOW EVERYTHING** (`security/capabilities/mod.rs:104-113`). A plugin-backed skill whose every declared capability came from a now-disabled extension gets an empty allowlist and can call **any** tool in the registry through `SandboxToolCallback` (`invocation.rs:1067-1092`). Disabling an extension currently **widens** that skill's reach. The file-based path dodges this only by accident, via `if !tool_defs.is_empty()` (`:281`) sending `policy_opt = None`. |
| **12** | `skill/router/mod.rs:101` | Beside the existing `invoke.mode == "disabled"` skip, drop candidates whose `requires_capabilities` are wholly withheld. `SkillRouter::route` takes only `(&str, &SkillCatalog)` and has no registry handle, so the catalog gains `set_availability_oracle(Arc<dyn CapabilityOracle>)` — a one-method trait **implemented by `ToolRegistry`**, not by the ledger: `withheld` (§7.2) is "the `capability_index` lookup is empty *and* the tombstone set is non-empty", and the ledger holds only the second half — a third, never-disabled provider could still serve the capability, which only the index knows. `resolve_capabilities` is already a registry method, so the oracle is `is_satisfiable(caps) = resolve_capabilities(caps, &[]).withheld.is_empty()` on the `Arc<ToolRegistry>` the services bundle already owns. Same filter in `catalog_summary` (feeding `<available_skills>` via `build_skills_catalog_block`, `query_handler/mod.rs:212-238`) and the `invoke_skill` listing (`invoke_skill.rs:103-115`). | A `mode: auto` skill whose only capability came from a disabled server still auto-selects at score ≥ 0.65 and runs toolless, while `<available_skills>` keeps coaching the model toward it. |
| **13** | `apps/openalpacad/src/scheduled_skills.rs:147` | After the catalog lookup in `spawn_timer_turn`, check the same predicate; skip with a `warn!` + event. **Do not deregister the cron job** — re-enable would then need a re-registration trigger `resync_skill` (`:86-97`) cannot provide, since it keys only on the catalog entry. Skip-and-log is idempotent and self-heals. | Unattended fabrication on a schedule: the turn goes through the gateway as a real user message on `{user}:scheduled` and the fabricated result is pushed cross-channel by the NotificationDispatcher. |
| **14** | `skill/catalog/mod.rs:529` (`register_plugin_skill`) | **Lowercase the id at insert** — a hygiene fix with a precisely bounded blast radius, not a load-bearing one. | Insert is verbatim (`:529`) while every reader lowercases. Two distinct consequences for a plugin whose `skill/info` returns a mixed-case `id` (e.g. `MySkill`): **(a) unreachable by `/slash` and `invoke_skill`** — `get_by_command` (`:355-379`) resolves the command index to the verbatim id and then calls `get_by_id`, which does `entries.get(&id.to_lowercase())` with **no** name fallback (`:382-385`), so the lookup misses; `get` (`:335-345`) is the only reader with a frontmatter-name fallback, and it is used by `scheduled_skills.rs:147` and the `depends_on` paths, not by the slash tier. **(b) survives `unload_plugin`'s `catalog.remove(skill_id)`** (`manager.rs:546`) only when `frontmatter.name.to_lowercase() != skill_id.to_lowercase()` — `remove` (`:465-478`) falls back to a name scan — which is the ordinary case whenever a plugin supplies both an `id` slug and a display `name` (`build_skill_frontmatter_from_info`, `manager.rs:881-886`, defaults `name` to the *plugin* name, not the id). In that case an entry holding an `Arc<dyn PluginSkillExecutor>` to a killed process leaks, and a later `/slash` for it still misses because of (a). |
| **15** | `apps/openalpacad/src/state.rs:17-40` | Add `tool_registry: Arc<ToolRegistry>` (GAP-18's read path) and **`extensions: Arc<Extensions>`** — the aggregator of §3: `{ ledger, mcp: Arc<McpSupervisor>, plugins: Arc<PluginManager> }`. The ledger alone cannot serve a route: it cannot run T2–T4, write `mcp.toml` or `.permissions.toml`, or `disconnect` — those are supervisor methods. `AppState.plugin_manager` (`state.rs:39`, `Option<Arc<PluginManager>>`; the struct closes at `:40`) already exists and is folded into `extensions.plugins` — non-optional, since `main.rs:334` constructs it unconditionally; `McpSupervisor` moves here from the services bundle it sat on between C2 and C6 (§3). Clone the registry `Arc` **before** its move into `Orchestrator::new` (`main.rs:373`) — the plugin manager already does exactly this at `main.rs:337` (`svcs.tool_registry.clone()`; `:337` is the skill-catalog clone). | No route can list or toggle anything. `Gateway` holds the orchestrator behind `Arc<dyn MessageHandler>` and `SharedContext` has no registry; there is no other path in. Also unblocks GAP-18. |

### 6.2a The unrecorded-extension default — fail-open, audited

`ExtensionLedger::check()` and the `extension_tool_defs` state filter both need an answer for an `ExtensionId`
that has **no ledger entry**. This is not a corner case: MCP tools are registered today by
`apps/openalpacad/src/services/mcp.rs` and plugin tools by `crates/openalpaca_plugins/src/manager.rs:836-847`
(the `RegisteredTool` literal at `:836-845`, `tool_registry.register(..)` at `:847`), and neither touches a
ledger until C2/C3 land. (There is no request-serving boot window to worry about: the
listener is bound at `main.rs:169` but `axum::serve` starts at `:630`, after `plugin_manager.start()` at
`:345` — connections queue in the backlog and nothing is served while the supervisors populate the ledger.
The rationale below stands on the C1-before-C2/C3 argument alone.)

**Decision: absent ⇒ `Allow`.** `check()` returns `Allow` with a no-op guard, and `extension_tool_defs` keeps the
tool. Absence means *"no supervisor owns this yet"*, not *"disabled"*: the owner's toggle only ever reaches the
ledger through a supervisor, so an unrecorded extension is by definition one the owner has not turned off.
Fail-closed would make C1 — an additive commit with no supervisor — silently remove every MCP and plugin tool
from the lead and main-loop surfaces and refuse every call at the gate, which is not "byte-identical" but a
regression that breaks bisectability. The same rule governs the miss arm of #1: `owner_of(name)` returning
`None` means "not an extension tool the ledger knows" and falls through to the ordinary not-found error.

**Why fail-open is safe here and not a bypass.** The only way it could become one is a supervisor that registers
a tool and forgets to record it. Three things pin that:
1. `ToolRegistry::register` / `replace` emit `warn!(tool, extension, "extension tool registered with no ledger
   record")` when `extension_id()` is `Some` and the ledger has no entry — visible in the log from the first
   boot after C1, and expected to appear exactly until C2/C3 land;
2. `ExtensionLedger::audit(&ToolRegistry) -> Vec<String>` lists every registered extension tool without a
   record; the supervisors call it at the end of boot and log at `error` if it is non-empty;
3. tests, named in the commits that introduce them: **C1 `unrecorded_extension_tool_executes`** — an
   MCP-backed tool registered directly through `ToolRegistry::register` with no ledger record still executes
   and still appears in `extension_tool_defs`; **C2 `mcp_supervisor_records_every_registered_tool`** and
   **C3 `plugin_supervisor_records_every_registered_tool`** — after `reconcile_all` / `start`, `audit()` is
   empty, so the fail-open path is unreachable for anything a supervisor loaded.

### 6.3 Why the list is COMPLETE

The claim: **every path by which an extension's backend can be invoked passes through site #1.** This is not an
assertion; it is an audit.

1. A backend is only reachable through `RegisteredTool.backend`, whose variants hold the `Arc<McpClient>` and
   `Arc<dyn PluginToolExecutor>`.
2. `grep -rn "\.backend" crates apps --include='*.rs'` outside `registry/mod.rs` returns **zero production
   reads of `RegisteredTool.backend`** (re-run for rev 4). Every production hit is an unrelated type:
   `ToolConfig.backend` (`tools/config/mod.rs:88`), `ConfigKeyDef.backend` (`apps/openalpaca/src/commands/
   config_handlers.rs`, `config_tui.rs`), `MemoryPreferences.backend` (`memory/preferences.rs`). The test hits
   are `tools/builtins/tests.rs:154` and `registry/tests.rs:1830` (the two on `RegisteredTool`), plus
   `tools/config/tests.rs` (`ToolConfig`, including `[tools.backend]` TOML fixtures) and
   `crates/openalpaca_storage/src/config_schema/tests.rs` (`ConfigKeyDef`) — rev 3's list omitted those last
   two files; both are unrelated types and the conclusion is unchanged. The public iterator `iter_registered_tools`
   has exactly one production consumer, `effective_confirmation_set` (`security/sandbox/mod.rs:401-420`),
   which reads `reg.annotations` only and never touches `backend` — worth naming, because the guard rail below
   relies on that iterator never being used to reach a backend.
3. `grep -rn "call_tool(" crates apps` outside `openalpaca_mcp/src` returns exactly **two** production sites:
   `registry/mod.rs:340` and `registry/mod.rs:392` — the two execute arms. The only other hits are
   `openalpaca_mcp/tests/filesystem_e2e.rs`.
4. `ToolBackend::Plugin(executor).execute(..)` appears exactly once, `registry/mod.rs:333`.
5. `SandboxManager::execute_tool`'s **only** execution step is `registry.execute_with_context`
   (`sandbox/mod.rs:310` and `:314`). So the gate is strictly *below* the sandbox and covers strictly more.
6. The plugin-skill out-of-process path proxies every **tool request** through `SandboxToolCallback`
   (`invocation.rs:1067-1092` → `sandbox.execute_tool`), and the plugin-agent loop does the same
   (`runner/plugin_agent.rs:151-174`). Both land on #1 for tool calls.
7. Snapshot registries are deep clones of the same struct with the same private `tools` field and the same two
   public execute methods, and the ledger is `Arc`-shared, so a snapshot's gate reads live state — and reads
   the live **generation**, so a snapshot entry built for a previous load is refused as `Stale` even when the
   extension is `Enabled` again (§3.0 Fact 3).
8. The **runs** behind item 6 — `skill/invoke` and `spawn`/`step` over the plugin's stdio channel — do not pass
   through the registry and are not claimed to. They are covered by the run-guards of §3.2 T3(b), taken at the
   only two in-process entry points (`invoke_plugin_skill`, `invocation.rs:934`, reached only from `:72`;
   the `run_plugin_agent_loop` call at `lead_agent/tools.rs:513`), which is a grep-verified count:
   `PluginSkillBridge::new` has one constructor site (`manager.rs:419`), `PluginSkillExecutor::invoke` one
   caller (`invocation.rs:1042`); `run_plugin_agent_loop` has one production caller.
9. A tool that is **no longer in the registry** cannot be dispatched at all, so it is not a bypass — but its
   refusal must still be attributed (S4), which is the miss arm of #1. Its inputs are the ledger's retained
   `tool_names`, which every supervisor writes at E5 and never clears before restart.

**Guard rail:** a regression test asserts that *every* public `async fn` on `ToolRegistry` that dispatches to a
backend refuses a disabled extension, and a doc-comment on `RegisteredTool.backend` states that reading it
outside the registry bypasses the gate. `backend` cannot be made private (plugin and MCP registration build the
struct by literal), so this is enforced by test and comment, not by the type system. That is the one soft edge
in the argument, and it is named rather than papered over.

**Why not enforce at `SandboxManager` instead:** it covers strictly less (it sits above the registry) *and*
costs more — `SandboxManager` is constructed at **seven** production sites, six of them per-request, all via
`SandboxManager::new`: `services/mod.rs:145` (the global one), `lead_agent/mod.rs:289`,
`lead_agent/tools.rs:261`, `invoke_executor.rs:371`, `invocation.rs:681`, `invocation.rs:997`,
`simple_query_handler.rs:633`. (Rev 2 said eleven and counted the four `with_defaults` sites —
`main_loop.rs:266`, `start_workflow.rs:222`, `security/gate.rs:54`, `plugin_agent.rs:330` — which all sit
inside `#[cfg(test)]` modules opened at `main_loop.rs:244`, `start_workflow.rs:191`, `gate.rs:46`,
`plugin_agent.rs:208`; `grep -rn 'SandboxManager::new\|SandboxManager::with_defaults' apps crates` re-run for
rev 3, every other hit is under `tests.rs` or `tests/`.) Threading a gate through seven constructors versus one
field on the registry is the cost half; item 5 — the registry sits strictly below the sandbox, so a gate
there covers every sandbox instance including any future one — is the coverage half. (Rev 3 added "direct
`registry.execute*` callers" to the coverage half; there are none in production, §3.2 T3(a). The argument
does not need them.) Wrong on both.

**Why not registry removal alone** (what plugin disable does today): §3.0. It is a no-op for the duration of
any run already in flight, which on a single-user daemon is the ordinary case — the owner opens Settings and
flips a toggle while something is running.

---

## 7. The warning path (S4)

S4 has two distinct moments and they need different mechanisms. Conflating them is how silent degradation
survives.

### 7.1 Moment 1 — ATTEMPTED USE. Cannot be silent, by construction.

The gate at site #1 returns, on `Blocked`:

```
tool 'github__create_issue' is unavailable: the MCP server 'github' is disabled.
Enable it in Settings → Extensions, or ask the user to turn it on.
```

The wording is total over the states, so no state ever falls back to a raw transport string:

| ledger reads | second sentence after `tool '<name>' is unavailable:` |
|---|---|
| `Disabled` | the MCP server 'github' is disabled. Enable it in Settings → Extensions, or ask the user to turn it on. |
| `Disabling` | the MCP server 'github' is being turned off right now. Do not retry it. |
| `Enabling` | plugin 'notion' is still starting. Retry. |
| `Unapproved{*}` | plugin 'notion' has not been approved (never approved / denied / asks for new capabilities). Approve it in Settings → Extensions. |
| `Failed{NeedsAuthorization}` | the MCP server 'github' needs authorization: <detail>. Authorize it in Settings → Extensions. |
| `Failed{NeedsConfig\|ConfigInvalid}` | plugin 'notion' needs configuration (<missing keys> / <parse error>). Configure it in Settings → Extensions. |
| `Failed{Unreachable\|Crashed}` | the MCP server 'github' is not running (crashed: <detail>). Retry it in Settings → Extensions. |
| `Orphaned` | plugin 'notion' is recorded but its directory was not found at <path>. |
| `Enabled`, stale generation | belongs to a previous load of MCP server 'github' (this run started before it was re-enabled); the copy in this run is stale — it is available again on the next request. |

(`Enabling` says "retry" and not "on the next request" because the E4→E5 window is milliseconds and the same
run's next call will ordinarily succeed; the stale row is tied to the request boundary because a snapshot is
only replaced when the next request takes a fresh one, §3.0.)

**This error string *is* the warning.** It
travels back through `SandboxManager::execute_tool` (which only forwards the registry's `Err`), into the
agentic loop, through `format_tool_error_with_hint`, and is pushed as a `ChatMessage::tool_result`. The model
reads it in-context and can tell the user in the same turn.

There is no separate "surface it in chat" plumbing anyone can forget to call, **because omitting the warning
would mean returning `Ok`** — on the hit arm. On the miss arm (§6.2 #1) omitting it would mean returning the
generic not-found string, which is *also* an error the model relays, so the property has to be pinned by the
miss arm running the identical `check()` rather than by construction alone; C1's
`live_registry_miss_on_withdrawn_tool_refuses_with_attribution` is the pin. Between the two arms, every call to
a tool that belongs to a non-`Enabled` extension — whether the caller holds a snapshot or the live registry —
gets the attributed string and the observability below.

Alongside the return, `ExtensionLedger::check(&ext, incarnation: Option<u64>, ctx: Option<&ToolContext>)` —
not the caller — does the observability (`ctx` is `None` from `execute()`, which has no `ToolContext`; the
scope key then falls back to `"global"`, §7.4):
- `tracing::warn!(extension, state, tool, agent_id, task_id, stale, "extension capability withheld")` on
  first occurrence in scope, `debug!` after.
- The one transport-failure path that does *not* pass through `check` — a plugin proxy hitting
  `ChannelClosed`/`ProcessCrashed` on a live or stale handle — logs its own `warn!` in the proxy (§3.6 item 2),
  because the registry's `Plugin` arm (`registry/mod.rs:333`) returns the executor's string with no log line.
- `bus.publish(SystemEvent::ExtensionCapabilityWithheld { .. })`. The ledger holds an `Option<EventBus>`
  (`bus.rs` — `Clone`), installed **once** by `ToolRegistry::with_event_bus(bus)`. The constructor lands in
  **C1**; the one production call, at `services/tools.rs:25`, lands in **C4** together with the event variant
  it exists to publish — so C1 genuinely changes no production caller, and a C1 ledger warns via `tracing`
  only. `ToolRegistry::new()` keeps its arg-free signature and `impl Default` (`registry/mod.rs:185-189`,
  which calls `new().expect(..)`) survives untouched: the two constructors have **101** call sites across
  `apps/` and `crates/` (`grep -rn 'ToolRegistry::new()\|ToolRegistry::default()'`), of which exactly **one**
  is production (`services/tools.rs:25`) and the other 100 are tests (60 in `registry/tests.rs` alone; the
  `Default` sites at `security/gate.rs:53`, `runner/plugin_agent.rs:310`, `main_loop.rs:265`,
  `start_workflow.rs:221` and `mcp/bridge.rs:177` are all inside `#[cfg(test)]`). A ledger with no bus logs
  and returns; every existing test constructs a registry exactly as before.

The **error** is never suppressed; only the announcement is deduped. A blocked call fails every time.

### 7.2 Moment 2 — WITHDRAWAL AT DECLARATION. Where today's silence lives.

The tombstone index is `capability → Set<ExtensionId>`, **a set, not a single id**. Two facts force that
shape: every tool a plugin registers carries the manifest's entire `capabilities.provides` list
(`manager.rs:839`), so one capability string maps to N tools of one extension; and nothing stops two plugins —
or a plugin and an MCP server — declaring the same capability, so one key can legitimately have several
providers. `withdraw(ext, caps)` inserts `ext` under each capability — called at T1 with each tool's
`provides_capabilities` and at T2 step 1 with the plugin's **virtual** capabilities
(`manifest.capabilities.virtual_.provides`, `manager.rs:470-474`), which no tool carries and which would
otherwise never be tombstoned; `restore(ext)` removes `ext` from every set; the set is never cleared
wholesale.

`resolve_capabilities` (site #3) consults it for **every** requested capability and classifies:
- **withheld** — the lookup is empty *and* the set is non-empty: every provider is gone, at least one of them
  by disable → warn + event, attributed to each recorded extension that is currently blocked;
- **partially withheld** — the lookup is non-empty, but the set records a provider that is now blocked, i.e.
  extension A is disabled while B still serves the capability → the **same** attributed warn + event
  (`Moment::SurfaceAssembly`), because a skill that expected A's tools just lost them and nothing else would
  say so; the resolution proceeds with B's tools;
- **unknown** — the lookup is empty and the set is empty: nothing ever provided it → `debug!` only.

**Partial withdrawal does not affect router candidacy, `/slash` refusal or the cron skip** — only total loss
does. That matches §10 case 3's total/partial split: a skill that still has *some* of its declared tools runs
and carries the chat-visible warning; a skill with *none* is refused. `CapabilityOracle::is_satisfiable` is
therefore "no requested capability is `withheld`", and `partially_withheld` is reported, never gating.

The attributed/unattributed split is upgrade-safety, not fastidiousness: `validate_annotation_capability`
(`tools/registry/capabilities.rs:57-64`) short-circuits to `Ok` for any string without the `annotation:`
prefix, so a typo and a withdrawal are indistinguishable today. Promoting unattributed misses to warnings would
fire on every existing install the moment this ships.

Call sites: `resolve_agent_tools` (`tools/mod.rs:19`), `invocation.rs:152`, `invoke_executor.rs:157`,
`invocation.rs:954`.

### 7.3 Moment 3 — THE TRANSITION. The one the owner is looking at.

As **T1 step 3** (§3.2) — inside the unpublish sequence, so the route, the watcher's `reconcile_all` and the
crash reaper all run it — the supervisor scans the agent registry (`capabilities`) and the skill catalog
(`requires_capabilities`) and emits **one** un-deduplicated `tracing::warn!` naming the extension and
**every** template and skill that just stopped resolving, plus the single event, defined once:

```rust
SystemEvent::ExtensionCapabilityWithdrawn {
    extension: ExtensionId,
    state: ExtensionState,               // Disabling on the route/watcher paths, Failed{Crashed,..} from the reaper
    capabilities: Vec<String>,           // the withdrawn set (T1 + T2 step 1 tombstones)
    affected_templates: Vec<String>,
    affected_skills: Vec<String>,        // total loss only
    affected_cron_skills: Vec<String>,   // subset of affected_skills that carry invoke.cron
    notice_lane: String,                 // the daemon's default lane, `{local_user_id}:gui`
}
```

(Rev 3 gave this variant two field lists; this is the one.) Both supervisors need the agent registry and the
skill catalog to run the scan: `PluginManager` already holds both as `Option`s (`main.rs:338-339`), and C4
hands `McpSupervisor` the same two handles alongside `default_lane_key`. The wording of the `warn!` and of the
notice below is keyed on `state` — *"disabled"* versus *"stopped running (crashed: <detail>)"*.

**How `PluginManager` publishes at all.** Today it has no `EventBus`: `PluginManager::new` takes the plugin
dir, the registry and the two `Option` handles (`manager.rs:167-172`, called at `main.rs:335-339`), and its only
outlet is the legacy `PluginEventSink` callback (`manager.rs:153`, `with_event_sink` at `:193`, `emit` at
`:199`), wired at `main.rs:343` to the WS broadcaster as `ServerEvent::Plugin*`. The ledger's bus is installed
in C4. So **C3 gives `PluginManager` the bus directly** — a `with_event_bus(bus: EventBus)` builder beside
`with_event_sink` (`openalpaca_plugins` already depends on `openalpaca_core`, which owns `bus.rs`) — and T5/E5
publish `SystemEvent::ExtensionStateChanged` on it; the `ServerEvent` peer is produced by the `event_bridge`
arm C2 adds, never by the sink. The six legacy producers keep firing beside it until C7 — the
`self.emit(ServerEvent::Plugin*)` calls at `manager.rs:294`, `:331`, `:504`, `:571`, `:611`, `:684` — and C7
deletes them together with `PluginEventSink`, `with_event_sink`, `emit`, the `main.rs:343` call and the test
wiring at `manager.rs:1154`, so the tree is green when the variants go (§8, §9.5, C7).

*What the scan intersects with, precisely.* Not `capability_index` alone — by this point T1 has removed the
withdrawn keys and #4 has dropped the empty ones, so the index has nothing left to attribute. The scan takes
the **withdrawn set** — the capabilities T1 and T2 step 1 just tombstoned under this extension's id — and
intersects it with each template's and skill's declared list; the current index is consulted only to classify
each hit as **total** (no surviving provider) or **partial** (another provider still serves it, §7.2). The
cron notice below fires on total loss only.

On the route path this is the only warning that fires while the user is still looking at the switch they
flipped; on the reaper path it is the only warning that fires at all for a crash nobody was watching. It is the
cheapest — two in-memory reads — and it is deliberately never deduped: one transition, one announcement.

**Additionally**, and only here: if the scan finds a **cron-scheduled** skill that just became unsatisfiable, a
**single** notice is delivered to the owner — **once per transition, never per fire**. A cron skill runs
unattended; the event log alone is not enough for the one failure mode with no human in the loop.

*How the notice actually reaches the owner* (each step is existing code):

1. The supervisor publishes the `SystemEvent::ExtensionCapabilityWithdrawn` defined above; `notice_lane` is
   the daemon's default lane, `format!("{local_user_id}:gui")` (`main.rs:199`), which the supervisor receives
   at construction (in C4, when this scan lands).
2. The **`NotificationDispatcher`** (`apps/openalpacad/src/notification/mod.rs:66`, a `SystemEvent` bus
   subscriber) gains a `handle_extension_notice` arm. When `affected_cron_skills` is non-empty it does the two
   halves `post_update` does today (`crates/openalpaca_core/src/runner/lead_agent/tools.rs:1049-1067`):
   - **write** — `persist_conversation(&db, &notice_lane, "gui", text, None, 0, 0, 0)`
     (`orchestrator/dispatcher/outcome.rs:323`). Two things about that call, stated so nobody re-derives
     them: **(i)** the third argument is the `source` **column**, not the role — `role` is hardcoded
     `"assistant"` at `outcome.rs:345`, which is exactly why the row renders (the GUI transcript skips only
     `role === "system"`, `views/chat/transcript-model.ts:208`). It is passed as `"gui"`, the lane's own
     source, because if the default lane has no conversation row yet `get_or_create_conversation`
     (`repository/conversation/mod.rs:146-165`) creates one with whatever `source` it is handed, and a
     `"system"`-sourced default lane would be wrong forever after. **(ii)** widening the fn to `pub` is not
     enough: its module is `pub(crate) mod outcome;` (`orchestrator/dispatcher/mod.rs:6`) while `dispatcher`
     itself is `pub mod` (`orchestrator/mod.rs:6`), so C4 adds `pub use outcome::persist_conversation;` to
     `dispatcher/mod.rs` and the daemon calls
     `openalpaca_core::orchestrator::dispatcher::persist_conversation`. The row is what `GET /v1/chat/history`
     serves (`routes/chat.rs:196`) and what the GUI chat renders (`hooks/useChat.ts:54`, `useChatHistory`).
     This is the half that reaches the default lane; the dispatcher holds `db` already (`mod.rs:32-37`).
   - **push** — the same cross-channel fan-out `handle_failure` uses for non-connector-origin tasks
     (`mod.rs:207-215`): `try_cross_channel_telegram` / `_imessage` / `_discord(user_id, &text)`
     (`notification/telegram.rs:11`, `imessage.rs:66`, `discord.rs:29`; they take a user id, which
     `handle_failure` supplies as `&task.created_by`). `NotificationDispatcher` has **no** `local_user_id`
     field (`mod.rs:32-37`), so the arm derives it as `notice_lane.strip_suffix(":gui")` — the same trick
     `resolve_telegram_chat_id` uses with `":telegram"` at `mod.rs:264`. If the owner also talks to the
     daemon from a connector, the notice lands there too.
3. `event_bridge.rs` maps the `SystemEvent` to `ServerEvent::ExtensionCapabilityWithdrawn` (same pattern as
   `WorkflowProgress` at `event_bridge.rs:491-496` → `events/handlers.rs:380`), so it is broadcast on the WS and
   written to the event log; the GUI's `invalidationKeysFor` (`lib/query-client.ts:41`) maps it to
   `qk.chat.all()`, so an open chat refetches and shows the row without a reload.

*Why not `SystemEvent::WorkflowProgress`, which rev 1 named.* Two reasons, both fatal. `handle_progress`
(`notification/mod.rs:223-236`) dispatches **only** to lane keys ending `:telegram`, `:imessage` or `:discord`
and does nothing else — the default `:gui` lane falls through all three branches, so the one notice the design
calls "the only failure mode with no human in the loop" would have been a silent no-op for the default user.
`post_update` reaches GUI chat only because it *also* calls `persist_conversation` before publishing; rev 1
copied the publish half alone. And `WorkflowProgress` requires a `task_id` (`events.rs:385`), which a disable
transition does not have. C4's test pins the replacement: disabling an extension with one cron dependent inserts
**exactly one** `assistant` row on the default lane and broadcasts **exactly one** `ServerEvent`.

Every transition also emits `ServerEvent::ExtensionStateChanged`, bridged to WS and written to the event log,
**carrying `ts` and `instance_id`** — which the six existing `plugin_*` variants omit (GAP-22,
`apps/openalpaca-gui/src/lib/events.ts:61-67`), making plugin rows unorderable. This family does not repeat
that.

### 7.4 De-duplication

`ExtensionLedger.warned: DashSet<(ScopeKey, ExtensionId, Moment)>`, 10-minute suppression window, LRU-capped at
512, swept lazily on insert. `ScopeKey` = `ctx.task_id` → `ctx.request_id` → `ctx.agent_id` → `"global"`
(`ToolContext` carries all three, `registry/mod.rs:19-37`).

What the dedup is *for*, stated against the code rather than an imagined mechanism: there is **no per-round
surface rebuild** — the lead and main-loop surfaces are assembled once per request (§6.2 #2) — so per-round
spam from `extension_tool_defs` was never the threat. The two genuinely repeating sources are:
- `Moment::AttemptedUse` — per call. A model that keeps retrying the same blocked tool inside one run (the
  ordinary failure loop) would otherwise emit a `warn!` and an event on every attempt; with dedup it announces
  once per scope per ten minutes while **every** attempt still fails with the full S4 error (§7.1).
- `Moment::SurfaceAssembly` — per spawn / per invocation. A lead that spawns eight subagents from one template
  runs `resolve_agent_tools` eight times; a skill invoked in a loop resolves on every invocation. Keyed on the
  task, that collapses to one announcement per workflow.

`withdraw()` and `restore()` both clear the affected extension's entries, so a disable/enable/disable cycle
re-announces rather than being swallowed. The transition warning of §7.3 is never deduped.

### 7.5 The log-versus-chat rule

> **Surface in chat when the withdrawal changes what the USER ASKED FOR. Log when it changes only what the
> MODEL WAS OFFERED.**
>
> Restated for docs and GUI copy: *the log records every withdrawal; chat mentions one only when it is about to
> change the reply.*

| case | chat? | why |
|---|---|---|
| `Moment::AttemptedUse` | **yes** — already chat-visible as the tool result | the model tried and could not |
| explicit `/slash` skill with a withheld requirement | **yes** — refuse, naming skill, capability, extension, remedy | the user named it; the deterministic tier (`handlers.rs:178-198`) returns directly with no fallback, so this message *is* the answer. `invoke_skill_with_telemetry` returns `Result<String, String>` (`handler_helpers.rs:31-42`), and the refusal is returned as **`Ok(reply)`** — the reply text — not as `Err`, so it does not depend on whatever `handlers.rs` does with an `Err` today (C5) |
| `invoke_skill` called by the model with a withheld requirement | **yes** — same refusal as the tool result | the model named it |
| auto-routed skill | no — it is dropped from candidacy before it can fire | nothing was attempted |
| cron skill | no per fire; **one notice on the owner's default lane per transition** (§7.3), written to the conversation store and fanned out cross-channel — not pushed through the connector-only progress path | nobody is watching |
| subagent template's declared capability (`resolve_agent_tools`) | no — log + event, plus an `unsatisfied_capabilities` chip on the template's GUI row | the user did not name this subagent; interrupting a running workflow to report a template's declaration is noise |
| main loop / lead agent losing default-surface extension tools | **no** — log + WS only | the model was never promised them; a paragraph about a disabled integration in every system prompt is prompt pollution that also invalidates prompt caching. If the model reaches for one anyway, §7.1 hands it a relayable error. |
| an extension simply being off, with nothing trying to use it | **never** | S4 is about *withdrawn capabilities*, not announcing inventory |

---

## 8. API surface

Error envelope: `{"error": "string"}` — matching the existing plugins/tasks/agents routes
(`routes/plugins.rs:38`). Deliberately **not** a third envelope. Bare arrays for unbounded lists
(api-fix-plan §7 rule, line 791).

### `GET /v1/extensions`

Bare array, both kinds. `?include_orphaned=true` (default `false`).

```jsonc
{
  "kind": "mcp" | "plugin",
  "id": "github",
  "version": "1.4.0" | null,
  "transport": "stdio" | "streamable-http" | null,   // mcp only
  "enabled": true,                                    // PERSISTED DISPOSITION — the toggle binds here
  "consent": "approved" | "pending" | "denied" | null,// plugins; null for mcp
  "state": "enabled"|"disabled"|"unapproved"|"failed"|"orphaned"|"enabling"|"disabling",
  "reason": "never_seen"|"denied"|"capabilities_grew"
          | "needs_authorization"|"needs_config"|"config_invalid"|"unreachable"|"crashed"|null,
  "actionable": false,          // derived from FailureReason::actionable(); drives the GUI CTA + tone
  "detail": "HTTP 401 from https://…" | null,
  "hint": "https://…/authorize" | null,
  "missing_config_keys": [],
  "added_capabilities": [],     // Unapproved{CapabilitiesGrew} — the DELTA, not the whole manifest list
  "tools":  ["github__create_issue"],
  "skipped_tools": [],          // E4 name collisions with a tool another ENABLED extension currently serves (§10 case 13)
  "skills": ["daily-digest"],   // plugins
  "agents": ["notion-writer"],  // plugins
  "connector": "slack" | null,  // plugins — contributed connector platform (manager.rs:494); null unless enabled
  "provider":  "acme-llm" | null,// plugins — contributed LLM provider (manager.rs:495); null unless enabled
  "since":  "2026-09-01T10:04:00Z"  // when the record entered its CURRENT state (every state, not only failed)
}
```

`since` is the instant of the last transition into the current `state`, for every state: for `enabled` the E5
commit, for `disabled` the T5 commit, for `failed` the `mark_failed`/E2 classification (this is the `since`
inside `Failed{..}`), for `unapproved`/`orphaned` the reconcile that parked it. Boot counts as a transition, so
after a restart every row's `since` is ≥ the daemon's start time — there is no pretence of remembering when
an extension was turned off in a previous session.

`tools` is live when `Enabled`. When not running it is **empty**, not invented — nothing caches a disabled
server's tool names across boots (§10). `skills`/`agents` restore data `PluginInfo` already computes
(`manager.rs:141-142`, populated at `:706-707`) and the hand-rolled JSON at `routes/plugins.rs:46-54` silently
drops. `connector`/`provider` exist so that a disabled plugin's residue is *visible*: a row that reads
`state: "disabled"` with a non-null `connector` is a T2 bug (§3.2), and C3's guard test asserts it never
happens.

`enabled` is **`null`** on the two rows whose disposition cannot be read — every plugin while
`.permissions.toml` is unreadable, and the `config/mcp.toml` pseudo-record (§4) — and the four verbs return
`409 {"error":"store_unreadable"}` on them without entering a transition. `Enabling`/`Disabling` are reported
literally as `"enabling"`/`"disabling"` (never as their target state), which is what §9.2's *connecting… /
shutting down…* row renders.

**`200`** always on success. **`404`** unknown id. There is **no `503`**: `AppState.extensions` is
non-optional and `plugin_manager` is constructed unconditionally (`main.rs:334`), so no "subsystem absent"
path exists to report.

### `POST /v1/extensions/{kind}/{id}/enable`

Runs W, then E0–E5 synchronously; returns the resulting row. **`200` even when bring-up fails** — the write
at W succeeded and the intent is durable; the connection outcome is a separate fact in the body (§3.4).
**`500`** only when the W write itself fails, in which case no transition was taken and the row is unchanged.
Idempotent: enable on `Enabled` is a CAS no-op returning the current row, **never a reload** (and W is
skipped — the bit is already `true`). `404` unknown, `409` orphaned.

### `POST /v1/extensions/{kind}/{id}/disable`

Runs W, then T0–T5 synchronously. Bounded by the **per-extension mutex wait** (zero unless an `Enabling` is
in flight, at most that server's `connect_timeout_secs`, default 30 s, `tools/mcp/config.rs:40`) **plus**
`drain_timeout_secs` **plus, for MCP, at most one in-flight call** — `min(max_tool_runtime_secs,
request_timeout) + 1s` — because `call_tool` holds the transport mutex across its await and `disconnect` needs
it (§3.2 T4). A `200` means: the bit is on disk (W), no new call can reach the extension (T0), its tools
and contributions are unpublished (T1/T2), and its teardown is done or unstoppably in progress (a detached
`disconnect` that completes when the straggler's future drops; the `closed` seal is already set). **`500`**
only when the W write fails — nothing was torn down and the row still reads `enabled: true`. Returns the
row with `enabled: false, state: "disabled"`; carries `"warnings": ["torn down with N call(s) in flight"]` if
the drain expired and/or `"teardown pending: 1 call still holding the transport"` if T4 had to detach.
Idempotent. On an `Unapproved` plugin it clears the bit — writing a **decision-less** entry
`{enabled = false}` if the plugin had none (§5, §5.1) — and returns `state: "unapproved", enabled: false`
(§4.1); nothing was loaded, so nothing is torn down, and a restart reads `never_seen` + `enabled: false` from
that entry.

### `POST /v1/extensions/plugin/{id}/approve` · `…/deny`

Plugins only; `409 {"error":"unsupported_for_kind"}` for `kind=mcp` — writing a server into your own
`config/mcp.toml` *is* the consent, and there is no untrusted binary to gate.

- **approve** records `consent = approved` against the manifest's **current** `capabilities.provides` (a
  read-modify-write that preserves `enabled`, §5). It **loads only from `Unapproved` with the bit set**
  (returning the `Enabling`→E5 result); from `Disabled` it returns `Disabled`; from `Enabled` or `Failed{*}`
  it re-records consent and returns the current row unchanged — **never a load** (Retry on a `Failed` row is
  `enable`, §4.1). Approving does not set `enabled`.
- **deny** records `consent = denied` **and performs the full unload** if anything is loaded (§4.1). Leaves
  `enabled` untouched — an `Enabled` plugin becomes `Unapproved{Denied}` with `enabled: true`, a `Disabled`
  one becomes `Unapproved{Denied}` with `enabled: false` — so a later approve restores the owner's last toggle
  position rather than guessing it. Consistent with the one-directional invariant of §4.

### `DELETE /v1/extensions/plugin/{id}`

`Orphaned` rows only — `409 {"error":"not_orphaned"}` otherwise, `404` if unknown. Removes the plugin's
`.permissions.toml` entry through the same atomic writer and drops the ledger record. It is the "Remove"
affordance of §9.2 and the only path that ever deletes a permissions entry (§5.1 — never automatically). It
does **not** touch a plugin directory; uninstalling an installed plugin, and installing one, is GAP-24.

### `GET /v1/tools` — GAP-18, respecified

Bare array.

```jsonc
{
  "name": "github__create_issue",
  "description": "…",
  "source": "builtin" | "mcp" | "plugin" | "config",
  "origin": { "kind": "mcp", "id": "github", "enabled": true, "state": "enabled" } | null,
  "provides_capabilities": ["github__create_issue"],
  "requires_confirmation": false,
  "invocations_today": 12,
  "version": "1.4.0",
  "author": "mcp:github"
}
```

`invocations_today` is a `COUNT(*)` over `tool_execution_log` (created by
`crates/openalpaca_storage/src/migrations/030_skill_tool_execution_log.sql:29`) for the name since local
midnight; no new table. Two facts about that table so the query is written right: **(i)** the sandbox does
not write it — `SandboxManager` publishes `SystemEvent::ToolExecuted` (`security/sandbox/mod.rs:380`) and the
daemon's event persistence writes the row (`apps/openalpacad/src/events/persistence.rs:423`,
`persist_tool_execution`), so the count lags a call by one bus hop; **(ii)** `timestamp` defaults to
`datetime('now')` — **UTC** text (migration 030 line 37) — so "since local midnight" is
`WHERE tool_name = ?1 AND timestamp >= ?2` with `?2` = today's local midnight **converted to UTC** by the
daemon (`chrono::Local::now().date_naive().and_hms(0,0,0)` → `.with_timezone(&Utc)` → the same
`%Y-%m-%d %H:%M:%S` text), not a bare `date('now')`, which would be off by the UTC offset. The index
`idx_tel_tool_ts (tool_name, timestamp DESC)` (migration 030 line 39) serves that predicate.

`origin` is **`null` for builtins and for `config/tools/*.toml` tools** — a builtin row carries **no enable
field at all**. This **supersedes** `ToolCatalogEntry.denied: boolean`, frozen into the frontend's proposed
contract at `apps/openalpaca-gui/src/lib/api/unbacked.ts:288-296` (the ADR-029 shape). There is no per-tool
enable state anywhere in the system; availability is *derived* — (the agent's capabilities) ∩ (its extension
being enabled) — never asserted per tool. Read-only; there is no `PUT` and no per-tool toggle (S1).

### `GET /v1/plugins` and `POST /v1/plugins/{name}/{approve|deny|enable|disable}`

**Deleted in C7 — the same commit that ships the GUI Extensions section**, together with the six `plugin_*`
WS mappings (§9.5) and the C3 `legacy_status_word` shim (§4.3). C6 only *adds* `/v1/extensions`; for the one
commit between C6 and C7 both route families exist and the Plugins panel keeps calling the old one, so no
commit ships a GUI that calls a deleted route. The app is not distributed, so no compatibility window beyond
that is warranted, and keeping aliases whose semantics change underneath (enable stops touching approval,
deny starts unloading) is a transitional lie in the CLI. `POST /v1/plugins/{name}/config`
(`routes/plugins.rs:158`) moves to `POST /v1/extensions/plugin/{id}/config` unchanged in shape (added in C6);
on success it writes the config and then, if the row is `Failed{NeedsConfig}` with `enabled` and `approved`,
**invokes the `enable` verb** (W is skipped — the bit is already `true` — then E0–E5, under the per-extension
mutex, with the generation bump and CAS rules of §3.3), so setting the last missing key starts the plugin
without a second call. It is not a separate transition: §4.1 has no column for it because it is the
`Failed{*}` + `enable` cell. The CLI's `openalpaca plugin …`
(`apps/openalpaca/src/commands/plugin.rs:18-57`) is re-pointed at `/v1/extensions` in C6 and gains
`openalpaca ext …` covering MCP — which has **zero** CLI surface today (grep for "mcp" in
`apps/openalpaca/src/` returns nothing).

---

## 9. GUI contract

### 9.1 Nav

`SETTINGS_SECTION_IDS` (`apps/openalpaca-gui/src/views/settings/sections.ts:12`): `plugins` → `extensions`,
and `skills` → `tools` (the design's "Skills" rows are in fact **tools** — `API_MAP.md:875-881` — which
`SkillsSection.tsx:1-13` already documents by honestly substituting skill-health rows). Still eight sections.
Counts come from the real list queries and are `undefined` while loading, **never `0`**
(`src/views/settings/SettingsView.tsx:7`, `:57`, `:68-69` — *"a zero is a claim"*; `src/views/SettingsView.tsx`
is a three-line re-export shim).

New **GAP-24** ("extension install / uninstall") added to the `GapId` union
(`apps/openalpaca-gui/src/lib/unavailable.ts:18-41`). Today there is **no MCP gap id at all** — a grep for
"mcp" in that file returns nothing — so MCP is absent from the entire hand-off accounting generated from that
table (`tasks/gui-api-requirements.md:3`). GAP-19 (plugin install) is **renamed** GAP-24 — same mechanism, widened
to MCP-server add/remove — and stays scheduled as `api-fix-plan.md` Phase 8 item 9, which C8 relabels (§12.1); C7
replaces the id in the `GapId` union. GAP-18's tool-catalog
half is closed by `/v1/tools`; its claim that the missing route blocks "the Settings → Skills rows (name,
description, asks, **enabled**)" must have the `enabled` half struck — that field is derived from the
extension row and no longer exists per tool.

### 9.2 Extensions rows

One list, MCP servers and plugins together, each row carrying a `kind` chip ("MCP" / "Plugin"). Uses the
existing `ListRow {name, tags, description, chips, meta, control}` (`views/settings/primitives.tsx:216`) with
no new primitive. The section queries **`GET /v1/extensions?include_orphaned=true`** — the API default hides
orphaned rows (§5.1, §8) so that scripts and the CLI's `ext list` see only real extensions, but the settings
page is exactly where an owner needs to see and Remove an orphan, so it opts in.

**The toggle binds to `record.enabled`, never to the status word.** This fixes a live correctness bug:
`PluginsSection.tsx:125` computes `checked={word === "running"}` from
`statusWord = status.split(/[:(]/)[0]` (`:31-34`), so a plugin that is approved and enabled but crashed,
loading, or needs-config renders **OFF** — and clicking it fires `enable` on something already enabled.
Serving `enabled` as its own field is what makes the switch honest.

`Tag` accepts an explicit `tone` prop (`components/ui/Badge.tsx`), so the row keeps the **state word** as its
text *and* gets the right colour. `toTagTone`'s literal table (`Badge.tsx:51-64`) is not the constraint.

| state / reason | control | tag text | tone | affordance | description |
|---|---|---|---|---|---|
| `enabled` | Toggle **ON**, live | `active` | `live` | — | "N tools" (+ transport for MCP) |
| `disabled` | Toggle **OFF**, live | `disabled` | neutral | — | "Turned off" |
| `enabling` / `disabling` | Toggle `disabled` with `disabledReason` "connecting…" / "shutting down…" | `loading` | neutral | — | — |
| `unapproved` / `never_seen` | **no switch** — Approve / Deny buttons | `waiting-approval` | `asks` | Approve, Deny | the manifest's declared capabilities, listed; suffixed "— starts on approval" when `enabled: true`, "— stays off after approval" when `false` (the bit is real, §4, just not rendered as a switch) |
| `unapproved` / `capabilities_grew` | **no switch** — Approve / Deny | `waiting-approval` | `asks` | Approve, Deny | **"Now also asks for: fs_write, net_connect"** — the delta, not the whole list; same `enabled` suffix |
| `unapproved` / `denied` | **no switch** — Approve button | `denied` | neutral | Approve | "You denied this plugin"; same `enabled` suffix |
| `failed` / `needs_authorization` | Toggle **ON**, live | `needs-auth` | **`asks`** | **Authorize** (opens `hint`) | `detail` |
| `failed` / `needs_config` | Toggle **ON**, live | `needs-config` | **`asks`** | **Configure** | the missing keys |
| `failed` / `config_invalid` | Toggle **ON**, live | `config-invalid` | **`asks`** | Open config | the parse error |
| `failed` / `unreachable` \| `crashed` | Toggle **ON**, live | `crashed` | **`warn`** | Retry (= `enable`) | the transport error + `since` |
| `orphaned` | Toggle `disabled` | `orphaned` | `warn` | Remove (= `DELETE /v1/extensions/plugin/{id}`, §8 — backed, not a GapNote) | "declaration not found at <path>" |

**Tone carries the actionable/not-actionable split** — `asks` means *you* can fix it, `warn` means *it* is
broken — driven by the API's `actionable` boolean, not by the GUI matching reason strings. That is the fastest-
read channel and it is where the owner's own distinction belongs.

**`enabled` + `warn` answers the "enabled but not working" question directly: the switch stays ON.** Anything
else would lie about what the owner asked for and make the Retry button nonsensical.

**Consent pre-empts the switch**, preserving the existing correct instinct at `PluginsSection.tsx:11-14`
(*"a switch would misrepresent it"*). MCP rows never render Approve/Deny — the daemon returns `409` for that
verb on `kind=mcp`, and the UI simply does not offer it.

### 9.3 Tools rows

- **A builtin row renders NO control at all.** Not a disabled toggle, not a checked-and-disabled one — no
  control element. A greyed-out switch implies a switch exists and is merely unavailable; the truth is that
  builtins are governed by agent config, which is not a per-tool switch. This is the directive rendered
  literally.
- **An extension tool row** renders a read-only provenance chip ("via MCP `github` — enabled") that navigates
  to the Extensions row. No per-tool toggle anywhere (S1).
- Skill health metrics move to their own subsection fed by the existing `GET /v1/skills/health`.

This is a **deliberate, documented departure from `DESIGN_SPEC`**, which draws a per-row Toggle on those six
tool rows (`API_MAP.md:877-879`). The design draws a control the settled model says must not exist; drawing it
disabled-forever would be worse than not drawing it. The section copy says so in one line — matching the
instinct already codified in `SkillsSection.tsx`.

### 9.4 No-mock-data contract

Every field above is served by `/v1/extensions` or `/v1/tools` on day one, so the Extensions section ships
**fully backed** with no decorative disabled toggle — precisely the failure the api-fix-plan warns about for
GAP-20 (*"the toggles are decorative"*, line 857) and which `deregister_provider`
(`routing/router/mod.rs:220`, zero callers) already demonstrates for LLM providers. The only `GapNote` is
**GAP-24** for install/uninstall. If the daemon is old or absent, the section renders its chrome plus that note.

### 9.5 WS invalidation

`src/lib/query-client.ts:35-110` gains:
- `extension_state_changed` → invalidate `["extensions","plugins","tools","skills","agents","connectors"]`
  (skills and agents because a plugin's contributions come and go with it; connectors because a plugin may
  declare one);
- `extension_capability_withheld` → invalidate `["extensions","tools"]`, and surface in the event log;
- `extension_capability_withdrawn` (the §7.3 transition event) → invalidate `["extensions","tools","skills",
  "agents"]` **plus `qk.chat.all()`**, so the default-lane notice written by the dispatcher appears in an open
  chat without a reload (the same reason `chat_stream_ended` invalidates chat, `query-client.ts:62`).

The six `plugin_*` mappings (`:67-73`) and their `ServerEvent` union members (`lib/events.ts:61-67`) are
deleted in C7 with the `/v1/plugins*` routes (§8).

---

## 10. Edge cases

| # | case | resolution |
|---|---|---|
| 1 | **Disable mid-run, mid-tool-call** | T0 flips the gate before any teardown. A run holding a deep snapshot (the lead agent always; skills and the main loop under the §3.0 predicates) sees it on its very next call because the ledger is `Arc`-shared through `Clone for ToolRegistry` (`registry/mod.rs:156`); a run holding the live registry — the ordinary skill — sees the entry vanish at T1 and is refused on the gate's miss arm (§6.2 #1) with the same attributed message. The in-flight call holds a `CallGuard` (a plugin skill or agent *run* holds one too, §3.2 T3(b)); T3 waits for the counter to hit zero, bounded by `[extensions] drain_timeout_secs` (default 10 s), then tears down regardless with a `warn!` naming the straggler — and a run cut off at the deadline fails with the S4 refusal, not a channel error. The loop's next call gets the S4 refusal as a tool result and the model can tell the user in the same turn. **The workflow is not aborted** — a hard mid-run stop is `/cancel`, an existing control with the right blast radius. |
| 2 | **Agent template names a capability from a disabled extension** | Template loads normally — a toggle must never be able to prevent the daemon from booting, and boot-time hard failure is already reserved for unknown `annotation:` names (`services/agents.rs:40,48`). At spawn, `resolve_agent_tools` (`tools/mod.rs:19`) reports it as `withheld`; the spawn path logs `warn!` + emits with `Moment::SurfaceAssembly`, deduped per (agent instance, extension) for 10 min. **Log only.** `GET /v1/agent-templates/{id}` gains a derived `unsatisfied_capabilities: [{capability, extension, reason}]` for the GUI chip. The subagent still spawns with a smaller set — refusing would turn one toggle into a cascade of workflow failures. |
| 3 | **Skill `requires_capabilities` includes a withheld one** | **Partial loss:** run with what survives; because it was explicitly invoked, prefix the result with the chat-visible warning naming the extension. **Total loss:** **refuse**. The total-loss refusal is mandatory, not stylistic (§6.2 #11 — it closes a privilege escalation). Availability rule for the router, `<available_skills>`, `/slash` refusal and the cron skip: a skill is unsatisfiable iff **at least one** required capability is **wholly withheld** (empty resolution with a recorded provider, §7.2). A capability that is *partially* withheld — one of two providers disabled — leaves the skill available and only warns. `requires_capabilities` has no optionality in the schema (`middleware/skill/types.rs:329`) and inventing one is scope creep. **Unattributed** misses (typos) keep today's silent-degrade behaviour, so no existing skill changes behaviour on upgrade. |
| 4 | **Cron skill fires while its dependency is disabled** | `spawn_timer_turn` (`scheduled_skills.rs:147`) checks the wholly-withheld predicate after the catalog lookup and **skips**, with a `warn!` + event per fire. The cron job stays registered — re-enable then needs no re-registration, which `resync_skill` (`:86-97`) could not provide anyway since it keys only on the catalog entry. A **single** notice is written to the owner's default lane at the disable transition (§7.3 — conversation row + cross-channel fan-out, not the connector-only `handle_progress` path), never per fire. |
| 5 | **Disabled plugin contributing skills and agent templates** | Both are withdrawn at T2 — already today's `unload_plugin` behaviour (`manager.rs:546`, `:554`) and correct under S2: a `SkillSource::Plugin` entry holds an `Arc<dyn PluginSkillExecutor>` pointing at a killed process. Two additions: (a) the catalog and agent registry record a **tombstone** so `/slash` answers *"skill 'x' is provided by plugin 'notion', which is disabled"* instead of the current "unknown skill" plus a dump of every catalog name (`invoke_skill.rs:103-115`); same for `spawn_subagent` naming a withdrawn template. (b) lowercase the plugin skill id at insert (`catalog/mod.rs:529`) — a bounded hygiene fix: without it a mixed-case id is unreachable by `/slash` (`get_by_id` has no name fallback, `:382-385`) and its catalog entry survives `remove` whenever the display name differs from the id (§6.2 #14). An in-flight subagent from a plugin template holds a cloned executor (`lead_agent/tools.rs:447`) and a run-guard (§3.2 T3(b)); its next tool call hits the gate and gets a clean refusal, the loop's next step-boundary `ledger.check()` stops it deliberately (`executor.stop()`), and if neither happens before the drain deadline the teardown surfaces through `run_scoped` as the S4 refusal rather than a dead channel. **Plugin-contributed skills never have cron jobs today, and the reason is the frontmatter, not the registration path.** Rev 4 said `register_plugin_skill` "does not call `resync_skill`"; true but not the reason — `scheduled_skills::sync_all` (`apps/openalpacad/src/scheduled_skills.rs:55-84`) iterates `catalog.entries_snapshot()`, plugin entries included, at boot (`main.rs:411`, after `plugin_manager.start()` at `:345`) and on every `daemon.toml` reload (`hot_reload.rs:131`), so it *would* register a plugin skill's job if one carried a cron expression. None can: `build_skill_frontmatter_from_info` (`crates/openalpaca_plugins/src/manager.rs:940-948`) builds `InvokeConfig { mode, slash, aliases, ..Default::default() }`, so `invoke.cron` is always `None` for a plugin skill and `register_skill` (`scheduled_skills.rs:103-111`) returns `false` before scheduling. **Pinned, because the day someone maps `cron` from `skill/info` this silently breaks:** C3 carries `plugin_skill_frontmatter_never_carries_cron` — feed `build_skill_frontmatter_from_info` a `skill/info` payload that *does* include `invoke.cron`, assert the result's `invoke.cron.is_none()`. Without the pin, a withdrawn plugin skill's job would survive T2 and fire into the unattributed *"no longer in the catalog — ignoring"* warn at `scheduled_skills.rs:147-152` with no notice — the unattended failure §7.3 says the log alone cannot cover. If cron is ever mapped, T2 step 2 must gain `wake.remove_job(skill_job_id(id))` for each withdrawn plugin skill, which means handing `PluginManager` a `WakeManager` handle; that is the cost the test makes visible. *Tombstone hygiene for (a):* `SkillCatalog::remove` (`catalog/mod.rs:465-500`) scrubs the command and alias indices via `remove_index_entries`, so after T2 `get_by_command` (`:355-379`) resolves to nothing and falls through to its `get_by_id` fallback, which also misses. The tombstone is therefore a **separate map** on the catalog, keyed by the lowercased skill id **and** by the slash command and aliases captured from the entry *before* `remove` runs, consulted by the `/slash` tier and `invoke_skill` only on a miss; the live indices are left scrubbed exactly as today. |
| 6 | **Re-enable cannot connect** | E2 precedes E4, so nothing was published and there is nothing to unwind. `enabled` stays **true**; `state = Failed{reason, detail, since}`. Route returns **200** with that row. GUI: toggle ON + `asks`/`warn` + CTA. Retry is `enable` again, idempotent. No exhausted-forever state — a genuine improvement over `ConnectionState::Failed{ReconnectExhausted}` (`client.rs:186-195`), which has no path back short of a restart. |
| 7 | **Disabled MCP server must not reconnect** | Three independent guarantees. (a) `reconcile_all` never calls `connect` for a disabled server, so no client exists. (b) `reconnect` (`client.rs:180`) is reachable **only** from inside `list_tools` (`:251`) and `call_tool` (`:306`) — there is no background poller, and §3.6's detection adds none — and the gate refuses before either. (c) `closed: AtomicBool` (§3.2 T4b), checked at `reconnect`'s entry **and** at `do_handshake`'s install point under the service lock, makes reconnection terminal even if (a) and (b) were both bypassed — including a reconnect that was already sleeping or handshaking when T0 flipped, which (b) does not cover because that reconnect entered legitimately. **This third guarantee is not optional:** `TransportClosed` is retriable (`error.rs:58-66`), so without it a snapshot call after teardown would `do_handshake()` and respawn the child; and without the install-point check an in-flight reconnect would install a live child into the sealed client after T4. (d) The **crashed** case is the mirror: an `Enabled` server's client *may* reconnect — for a stdio server that means **respawning the child** on the next call, which is its in-session recovery and is correct while the command still starts; only after four consecutive `reconnect()` entries with no successful handshake does it report `ReconnectExhausted` (§3.6 item 1), whereupon `mark_failed` + the reaper's T4 seal it exactly like a disable, so a *failed* server never respawns on its own again. (e) A snapshot from before a re-enable holds the **previous** load's sealed client; the gate refuses it as `Stale` by generation before it can touch the seal (§3.0 Fact 3). |
| 8 | **Restart with a disabled extension** | MCP: `enabled = false` → `services/mcp.rs:50` builds a ledger record `{disposition: false, state: Disabled}` and does not connect. Plugin: `.permissions.toml` `enabled = false` → the 2×2 gate at `manager.rs:284` stops before spawn. Both are **enumerable** — the row renders with its toggle off rather than vanishing. Observed state starts empty: a `Crashed` from the previous boot would be a lie. **Consequence, stated honestly:** after a restart, a capability from a never-connected disabled extension classifies as `unknown`, not `withheld`, so the warning is less precise than during a session. The extension row still reads `disabled`, so the user is not stranded. Caching tool names across boots would fix it and is deliberately **not** built — it buys a diagnostic nicety for a stale-cache class of bug. |
| 9 | **Malformed `config/mcp.toml`** | **Write side:** `toml_edit` surgical edit → temp file → **re-parse with `McpConfig::load`** → rename. A failed re-parse aborts with the file untouched; on the route path that is a `500` **before any CAS** (the write is step W), and off the route path it follows the §3.2 persistence-failure rule (log at `error`, keep the in-memory state, retry at the next reconcile). The declaration-gone path never writes at all (§3.2 T5-gone), so the one case where the re-parse is *guaranteed* to fail — assigning `enabled` into a `[servers.<n>]` table that no longer exists, which `toml_edit` would synthesize without a `transport` tag — is never attempted. **Read side:** boot downgrades a parse error from fatal (`services/mcp.rs:37-42` → `services/tools.rs:108` → `services/mod.rs:137`) to zero servers plus one pseudo-record `{id: "config/mcp.toml", state: Failed{ConfigInvalid}, detail: <parse error>}`. A bad hand-edit can no longer brick the daemon. |
| 10 | **Corrupt `.permissions.toml`** | Stop failing open. `load_permissions_table` (`permission_gate.rs:140-153`) returns `Err`; every plugin parks at `Failed{ConfigInvalid, "permissions store unreadable"}`; nothing loads; **the file is never overwritten**, so the user can repair it. Writes go through the same lock + temp + re-parse + rename path, so a crash mid-write can no longer truncate the approval store. |
| 11 | **Enable on an already-Enabled extension** | CAS at E0 fails; 200 with the current row; nothing happens. Fixes the permanent capability-provider leak at `manager.rs:262-278` (§3.3 E0). |
| 12 | **Plugin capabilities grew between disable and re-enable** | E1 drift check reads back the list recorded at `permission_gate.rs:66`. Growth → `Unapproved{CapabilitiesGrew{added}}`, requiring a fresh Approve that shows **only the new capabilities**. Falls straight out of splitting `approved` from `enabled`. |
| 13 | **Two extensions register the same tool name** | Pre-existing and not solved here, but not made worse. `register` overwrites on duplicate and returns `Ok` (`registry/mod.rs:262`). The rule, stated once so §8's `skipped_tools` and this cell agree: **a name is blocked only by a live incumbent.** At E4 the newcomer asks `owner_of(name)`; if it returns a *different* extension whose state is `Enabled` — the tool is live in the registry — the newcomer skips the name with `warn!(ext, tool, incumbent, "tool name collision — skipping")` and records it in its row's `skipped_tools`. If the incumbent is **not** `Enabled` (a retained attribution from a disabled, failed or unapproved extension), the newcomer **takes the name**: `record_tools` writes `tool_names[name] = newcomer` and removes the name from the incumbent's retained set, so a later miss attributes to the extension that actually served it last, and the incumbent's own T1 (on its eventual disable) will not touch a name it no longer owns. `tool_names` stays a single `name → ExtensionId` map — two claimants are never recorded at once; the loser is either skipped (live incumbent) or displaced (dead incumbent). At T1 an extension removes only names the ledger currently attributes to it. Without this, disabling A could delete a tool B had overwritten it with, or a re-enabled A could be refused its own tool because a long-disabled B once had the name. A real namespace fix is separate work. |
| 14 | **`config/mcp.toml` does not exist** | After the seeding change (§5.1) it always does, fully commented. Toggling a server not present in the file is **404**: this API never *creates* servers, only toggles declared ones. Server creation is GAP-24. The shipped file has every server commented out, so the empty case is the common one and the section gets an explicit empty state naming where to declare a server and where to drop a plugin. |
| 15 | **Hand edit to `mcp.toml` the daemon did not write** | The file **is** the store, so a hand edit is authoritative and there is no precedence rule to surprise anyone. `mcp.toml` joins `watch_paths` (`main.rs:259-292`) with a `mcp_hashes` dedup ring on `FileWatcherContext` (`hot_reload.rs:23-53`); the reload arm calls `reconcile_all()`, which diffs desired against actual and loads/unloads only what changed. Losing the event is tolerable — filesystem events are `try_send` with drop-on-full (`wake/watcher/filesystem/mod.rs:114-120`) — because the **route** path never depends on the watcher: `set_enabled` writes the file (step W) **and then** reconciles in-process. The watcher **does** observe the daemon's own write — a route-driven toggle produces the same filesystem event a hand edit does — and the `mcp_hashes` ring is what swallows it: unlike the existing `llm_hashes` ring, which nothing ever pushes into (making the "skipping reload" branch unreachable), the writer here pushes the post-write content hash **before** the rename, so the reload arm finds the hash already present and skips; the in-process `reconcile(name)` is the only reconcile a route-driven toggle runs. |
| 16 | **Plugin directory vanishes / returns** | `Orphaned`, entry preserved (§5.1). |
| 17 | **Disable → re-enable while a run holds a snapshot** (§3.0 Fact 3) | The lead agent's snapshot keeps the **previous** load's `RegisteredTool`: an `Arc<McpClient>` that T4 disconnected and T4b sealed, or a `PluginToolProxy` over a channel whose process is dead. After E5 the ledger reads `Enabled`, so a state-only gate would pass the call to the dead handle — a raw `transport closed` string for the rest of the run (MCP), or, for plugins, a `ChannelClosed` that rev 3's proxy would have turned into `mark_failed` **against the healthy new process**. Resolution: every load has a `generation` (bumped at E0, stamped into the `ToolBackend::Mcp` literal at `bridge.rs:46` and into the proxies at `manager.rs:831`/`:419`/`:452`); the hit arm compares `entry.incarnation()` to the record and refuses a mismatch as `Stale` with the §7.1 wording, `warn!` + `ExtensionCapabilityWithheld`; `mark_failed` is a no-op for a non-current generation. The run loses the tool until its next request — which is exactly when a fresh snapshot is taken. Pinned by C1's `stale_snapshot_after_reenable_refuses_and_live_stays_enabled` and C3's `stale_proxy_channel_closed_after_reenable_does_not_flip_row`. |

---

## 11. Migration — `global_tool_deny`

**PURGED.** Field, key, parameter, and every application site deleted in one commit. No shim, no deprecation
window, no re-homing, no automatic conversion. The app is not distributed and legacy compat is not required.

### 11.1 Deletion list

Re-run for rev 3: `grep -rn global_tool_deny apps crates config scripts docs --include='*.rs'
--include='*.toml' --include='*.md'` → **41 hits in 16 files**, every one of which is enumerated below (rev 2
had the totals right and omitted three of the files — the two user manuals and the agent-loop doc). Outside
those five roots the identifier also appears in **`CLAUDE.md:136` and `:154`**, listed with the docs. A second
grep, `grep -rni 'global tool deny'`, finds **six** prose-only lines without the identifier; they are listed
at the end.

- `crates/openalpaca_core/src/daemon_config/execution.rs:83` (the field) and `:95` (its `Vec::new()` default)
- `crates/openalpaca_core/src/daemon_config/orchestrator.rs:47` (doc comment)
- `crates/openalpaca_core/src/tools/registry/mod.rs:627` — the `deny` parameter; signature becomes
  `extension_tool_defs(&self)`. Doc comment at `:620` rewritten. **`sort_by(name)` at `:639` kept** — it feeds
  prompt-cache fingerprints.
- `crates/openalpaca_core/src/tools/builtins/main_loop.rs:179-185` (+ module doc at `:9`)
- `crates/openalpaca_core/src/runner/lead_agent/mod.rs:148-154`
- `crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs:176-190` (the
  `tool_selection = "full"` base-pick branch)
- `crates/openalpaca_core/src/orchestrator/skill/invocation.rs:194-201` (the `retain`), `:290-296` (fold into
  `denied_capabilities`), `:630` (constructor arg), `:963-973` (plugin-skill fold)
- `crates/openalpaca_core/src/orchestrator/skill/invoke_executor.rs:30-32` (the struct field), `:54`, `:70`,
  `:175`, `:305`, `:384`
- `crates/openalpaca_core/src/tools/builtins/invoke_skill.rs:172-176`, `:199`
- `apps/openalpaca-gui/API_MAP.md:893` and `:906` (the `denied` field's provenance note — superseded by
  `origin`, §8)
- **Config files — prose only, no key.** `config/daemon.toml:79-80` is not the key: it is two lines inside the
  `tool_selection` comment block (*"minus execution.skill_defaults.global_tool_deny, the opt-out"* and
  *"entire registry minus the global tool deny list"*), and `scripts/release/templates/config/daemon.toml:34`
  carries only the second sentence (`grep -n global_tool_deny` on the template returns **nothing**; rev 1's
  "corresponding line" claim was wrong). **Neither shipped file sets `global_tool_deny = [...]`.** Both comment
  blocks are rewritten to describe the surface without the opt-out. Consequence for the stale-key probe below:
  it fires only on a key a user added by hand, never on a shipped file.
- tests: `main_loop.rs:536`, `lead_agent/tests.rs:1321`, `invoke_executor.rs:654`
  (`test_nested_invocation_respects_global_tool_deny`), `daemon_config/tests.rs:112`, `:122-129`, `:147`
- **User-facing docs (the three files rev 2 omitted):** `docs/agent-loop.md:46` and `:104` (the lead-agent and
  main-loop surface descriptions, "minus `execution.skill_defaults.global_tool_deny`"),
  `docs/Skill_Template_Reference.md:208` (the `tools.deny` row's "combined with the global deny list"),
  `:525` (resolution step 5) and **`:540` — a config-key table row documenting `global_tool_deny` as a live
  key**, which must be deleted, not reworded; `docs/Daemon_Manual.md:97` ("list a namespaced name in
  `execution.skill_defaults.global_tool_deny` … to opt a tool out of both surfaces" — replaced by a sentence
  pointing at the Extensions toggle). Plus `CLAUDE.md:136` ("minus `global_tool_deny`") and `:154` ("is the
  opt-out"). All in C8.
- **Prose without the identifier (six lines, second grep):** `invoke_skill.rs:6` (module doc — **not**
  adjacent to the `:172-176`/`:199` edits, so listed on its own), `main_loop.rs:176` (comment above the
  `:179-185` block), `daemon_config/execution.rs:82` (the field's doc comment, goes with `:83`),
  `invoke_executor.rs:651` (test doc, goes with `:654`), `config/daemon.toml:80` and
  `scripts/release/templates/config/daemon.toml:34` (both covered by the comment-block rewrite above).

**Stale configs:** `DaemonConfig` and its sub-structs use `#[serde(default)]` with **no**
`deny_unknown_fields` (`daemon_config/mod.rs:33-45`), so an existing `daemon.toml` carrying the key parses
clean and the value is ignored. Silence is the one outcome worth avoiding — a user could believe a tool is
still suppressed. `load_daemon_config` (`daemon_config/mod.rs:48`) deserialises **directly** —
`toml::from_str::<DaemonConfig>(&content)` at `:50`, with no intermediate `toml::Value` (rev 3 said it
"already holds" one; it does not) — so the probe adds one: parse to `toml::Value`, inspect
`execution.skill_defaults.global_tool_deny`, then `Value::try_into::<DaemonConfig>()`. Three lines, same
function, logging once at `WARN`:

> `execution.skill_defaults.global_tool_deny` was removed — per-extension toggles replace it; see
> `openalpaca ext list`. The key is being ignored.

**No automatic conversion into extension disables.** A deny list naming two tools from a five-tool server does
not mean the user wanted the server off; guessing would silently disable working integrations on upgrade.

### 11.2 Is any per-TOOL reach still needed under S1? **No.**

S1 puts the toggle on the install unit; `global_tool_deny` is per-tool, i.e. strictly finer. So the honest
question is whether anything real is lost. Three findings settle it, and the last one is decisive.

**(1) It was never a gate, so it cannot be the fine-grained half of an enable mechanism.** A denied tool stays
registered, stays a live entry in `capability_index`, and stays fully callable by any subagent template naming
its capability and by `ToolRegistry::execute` directly. It filters what the model is **shown** on three
surfaces, not what can **run**. Keeping a display filter beside a real kill switch, under names that imply the
same thing, is a security-shaped trap — someone will read a deny list as "off" and be wrong.

**(2) Its reach is incoherent in ways nobody can reason about.**
- Matched by tool **NAME** wherever it applies (`registry/mod.rs:635`, `main_loop.rs:185`,
  `lead_agent/mod.rs:154`, the skill-path `retain`s below) — while the one per-agent deny that *does* exist,
  `constraints.denied_capabilities`, is matched by **capability** (`registry/mod.rs:596-610`, fed by
  `resolve_agent_tools`, `tools/mod.rs:19-27`). Two deny mechanisms with two match keys, and only one of
  them is `global_tool_deny`.
- Never reaches `resolve_agent_tools` (`tools/mod.rs:19`) at all — **it does not reach subagents**.
- Does not filter the default `core_union` main-loop base picks (`simple_query_handler.rs:101-115`); only the
  `"full"` escape hatch applies it.
- Script tools and synthetic `invoke_skill:<dep>` defs are appended **after** the `retain` that applies it
  (`invocation.rs:201` then `:203-217`; `invoke_executor.rs:175` then `:177-198`), so they are structurally
  un-denyable.
- On the skill paths it is a flat `retain(|t| !global_deny.contains(&t.name))` over an already
  capability-resolved set, so **it will happily deny a BUILTIN** — which alone disqualifies it under
  "builtins are not toggled".

**(3) It is the rejected shape.** A flat per-tool name list on `SkillDefaults` is exactly what ADR-029
generalised and the owner rejected. Leaving a vestigial instance in `daemon.toml` would keep that framing alive
in the config file the owner reads.

**What is actually lost, and where it goes.** One real use dies: trimming a chatty 40-tool MCP server down to
three tools on the main-loop prompt. That is **prompt-budget management, not access control**, and it has two
existing homes: the per-agent ALLOW axis (route heavy work to a template naming the three tools — subagents are
genuinely template-scoped), and `[orchestrator.routing] tool_selection`, which already governs how much of the
registry reaches the main-loop prompt. If it bites in practice, the correct future shape is a per-server
**contribution allowlist** in that server's own block — `[servers.gh] expose = ["create_issue", …]`, applied in
`bridge::rmcp_tool_to_registered` (`tools/mcp/bridge.rs:20`) so the tool is never registered at all. That is a
declaration feature about what an extension *offers*, scoped to the install unit; it is explicitly **not built
now**, and it is recorded here so it is not re-derived as "bring back `global_tool_deny`".

**Known consequence, named:** the lead agent and main loop still receive every installed extension tool
regardless of their template's `capabilities` (`lead_agent/mod.rs:154`, `main_loop.rs:185`), and after this
purge the whole-extension toggle is the **only** lever there. Making those two surfaces respect their template
is the right fix and is out of scope.

### 11.3 What does not migrate

`providers.<name>.enabled` in `llm.toml` and `<name>.enabled` in `system_config` (connectors) stay where they
are. Connectors already have a working DB-backed toggle (`managers/connector.rs:151-176`); LLM providers are a
separate axis with their own known defect (`deregister_provider`, `routing/router/mod.rs:220`, zero callers —
GAP-15's problem). `.permissions.toml` needs no version field: `enabled` is serde-defaulted and additive, and
the `approved`/`approved_at` widening to `Option` (§5) reads every existing entry unchanged.
`config/mcp.toml` gains no new keys; its comment *"Explicitly disabled without removing"* is now simply true at
runtime rather than only at boot.

---

## 12. Implementation plan

Eight commits, each independently reviewable, each leaving the tree green. ~1,900 lines of Rust, ~400 of
TypeScript, plus tests. No DB migration.

**Dependency changes (all in C1):** add `toml_edit` to `[workspace.dependencies]` and to
`crates/openalpaca_core` (the shared `config_io::atomic_write_toml` helper lives there, §2.1); promote
`tempfile.workspace = true` from `[dev-dependencies]` to `[dependencies]` in `crates/openalpaca_core/Cargo.toml`
(it is already vendored at `Cargo.toml:111`); `file-lock` already exists at `Cargo.toml:107`.

| # | commit | contents | verification |
|---|---|---|---|
| **C1** | **Ledger + gate + shared plumbing** (~540 + 260 test) | New `crates/openalpaca_core/src/tools/extensions/`: `ExtensionId`, `ExtensionKind`, `Disposition`, `ExtensionState`, `UnapprovedReason`, `FailureReason` (+`actionable()`), **the `ExtensionSupervisor` trait** (§3 — declared here because both implementors are downstream of `openalpaca_core` and nothing else is upstream of both), `ExtensionLedger` (CAS transitions incl. `mark_failed(ext, generation, ..)`, **per-record `generation` bumped and returned by `begin(ext, Enabling)`** (§3.0 Fact 3), retained `tool_names` + `owner_of(name)`, tombstone index `capability → Set<ExtensionId>` with `withdraw(ext, caps)`/`restore(ext)`, in-flight counters + `CallGuard` (counter incremented before the state read, §3.2 T0), `begin_run(ext, generation)`/`run_scoped` for out-of-process runs, `on_crash(kind, tx)` reaper senders carrying `(ExtensionId, u64)` — the per-kind sender slot is a `OnceLock` so `new()` stays arg-free and the supervisors (which do not exist in C1) register later — warn-dedup, `Option<EventBus>`, `audit()`). `RegisteredTool::extension_id()` + **`incarnation()`**; **`generation: u64` on `ToolBackend::Mcp`** (one production literal at `bridge.rs:46` via a new `rmcp_tool_to_registered` parameter — its one production caller `services/mcp.rs:135` passes `0` until C2, and its **three test callers** change with it: `tools/mcp/bridge.rs:160` and `:175` (under the `#[cfg(test)]` at `:111`) and `crates/openalpaca_core/tests/mcp_integration.rs:61` — two destructuring arms at `registry/mod.rs:334`/`:384`, three `ToolBackend::Mcp` test literals, §3.1) and **`PluginToolExecutor::generation() -> u64 { 0 }` default method** in `openalpaca_api`. `ToolRegistry.extensions` + the `Arc::clone` in `Clone` (`registry/mod.rs:156`) + `extensions()` accessor; **`ToolRegistry::with_event_bus(bus)` constructor only — `new()` and `Default` unchanged, no production caller yet** (§7.1). Private `dispatch` refactor + the **two-arm** gate at `:300`/`:362` — hit arm on the entry **with the generation compare**, **miss arm via `owner_of`** (§6.2 #1), `check(&ext, Option<u64>, Option<&ToolContext>)` — **absent entry ⇒ `Allow`** (§6.2a). `resolve_capabilities` with `withheld`/`partially_withheld`/`unknown`; `replace()`; empty-key cleanup in `remove`; `exempt_from_timeout` forced `false` for extension tools; unrecorded-registration `warn!`. `extension_tool_defs` state filter (keeps `deny` param until C8). **Shared plumbing both supervisors need:** `[extensions] drain_timeout_secs` (default 10) in `DaemonConfig`, and `config_io::atomic_write_toml` (lock + `toml_edit` + re-parse callback + temp + rename) in `openalpaca_core`. | **THE three tests that matter:** (i) take a `(*registry).clone()` snapshot, disable through the ledger, assert the **snapshot's** `execute_with_context` refuses with the S4 string; (ii) **`live_registry_miss_on_withdrawn_tool_refuses_with_attribution`** — record the tool under an extension, disable through the ledger, `remove()` it from the **live** registry, call `execute_with_context` on that same registry, assert the S4 string (not "not found") and exactly one `warn!` (observed through a `tracing` subscriber or the ledger's dedup set — the `ExtensionCapabilityWithheld` variant lands in C4 and C1's ledger has no bus, so C1 cannot assert the event); (iii) **`stale_snapshot_after_reenable_refuses_and_live_stays_enabled`** — snapshot → `begin(Disabling)`…`Disabled` → `begin(Enabling)` (generation bumps) → `replace()` a fresh entry with the new generation on the live registry → `Enabled`; the **snapshot's** call refuses with the `Stale` wording and one `warn!`, the **live** registry's call succeeds, and `mark_failed(ext, old_generation, ..)` leaves the record `Enabled`. Plus: gate taken exactly once for a `Plugin` backend via `execute_with_context`; builtin unaffected; an unknown name with no ledger owner still gets the plain not-found error; `capability_index` has no empty keys after remove; enable/disable/enable leaves no duplicate index edges; **`unrecorded_extension_tool_executes`** (an MCP-backed tool registered straight through `register` with no ledger record still executes and is still listed); **`extension_tools_never_timeout_exempt`**; partial-withdrawal classification with two providers of one capability; `mark_failed` is a no-op from `Disabling`/`Disabled` **and for a stale generation**; a call that took its guard just before `begin(Disabling)` is counted by the drain. Writer test: comments preserved, malformed edit aborted. **Lands with no production behaviour changed** (the one production caller edit is `services/mcp.rs` passing `generation = 0`, inert with no ledger record). Behaviour for the tools `services/mcp.rs` and `manager.rs:836-847` already register is unchanged *because* an unrecorded extension is fail-open — a property the named test proves, not an assumption that "nothing registers an extension yet" (rev 1's "byte-identical" claim was false as stated). |
| **C2** | **MCP supervisor** (~470 + 160 test) | `closed: AtomicBool` in `openalpaca_mcp` (`ClientInner`, `client.rs:53`; set in `disconnect` at `:165` before the lock; checked at `reconnect`'s entry `:180` **and at `do_handshake`'s install point `:137` under the service lock**, closing the just-spawned child if sealed — §3.2 T4b). `McpSupervisor` (`apps/openalpacad/src/managers/mcp.rs`, implements `ExtensionSupervisor`): reconcile / enable / disable / load / unload, its own `Arc<McpClient>` map, `ledger.record_tools` at E5, **step W write-first on both verbs** (`500` + no CAS on write failure), teardown via `(*arc).clone().disconnect()` under the T4 timeout + fresh-future detach rule, `classify_bringup_failure`, partial-load unwind, the crash reaper task (**re-check `Failed{Crashed}` + generation under the mutex, then** T1→T2→T4, never writing state — §3.6), the `Mcp` execute arms' `ReconnectExhausted → mark_failed` (§3.6). `services/mcp.rs:50-53` builds records instead of `continue`; `join_all` boot; non-fatal parse (`:37-42`). `mcp.toml` writer on top of C1's `atomic_write_toml`. `seed_default_configs` seeds a commented `mcp.toml` — **adds `scripts/release/templates/config/mcp.toml`** (a copy of the shipped `config/mcp.toml`; the directory holds only `daemon.toml`/`llm.toml` today) as the third `include_str!`. `mcp.toml` on `watch_paths` + `FileWatcherContext` + reload arm + hash ring (pushed before the rename so the daemon's own write is swallowed). **`SystemEvent::ExtensionStateChanged` + `ServerEvent::ExtensionStateChanged` (with `ts`/`instance_id`) + their `event_bridge`/persistence arms land here** — T5 has to emit something and this is the first commit with a transition. Deleted MCP declaration → T0–T4 with **no file write**, then record dropped + `ExtensionStateChanged` (§3.2 T5-gone; no MCP `Orphaned`); the off-route persistence-failure rule (log at `error`, keep state, retry at next reconcile). The supervisor is parked on the services bundle until C6 (§3); `shutdown_all()` is called directly from the daemon shutdown path here. The E0 generation is threaded into `rmcp_tool_to_registered` (C1's parameter). | Integration test: a stdio server (a temp-dir script that **writes a pidfile** on start — the daemon holds no MCP child handle, rmcp kills it from a detached task on close, §3.2 T4, so liveness is observed externally), enable → tools registered; disable → poll `kill -0 <pid>` until it fails within the T4 bound **and** a stale snapshot call refuses **and** a live-registry call refuses with attribution **and** no new pidfile appears (no respawn); re-enable → tools back with no duplicate index entries, **and a snapshot taken before the disable now refuses as `Stale` while the live registry serves the new load**. **Seal-in-flight test (T4b, window 2):** make the server hang so a call times out and enters `reconnect()`; while it sleeps the backoff, start `disable` with `drain_timeout_secs` short enough to expire first; let the handshake complete; assert no live pid, `ledger` reads `disabled`, and the sealed client's next `call_tool` returns `TransportClosed` without spawning. **Write-first:** make `mcp.toml` read-only → `disable` returns `500`, the row still reads `enabled: true, state: enabled`, the server is still up; `enable` from `Disabled` on an unreachable command → `200`, row `enabled: true, state: failed`, and a supervisor restart reads the bit as `true` and re-tries. **Reaper superseded:** `mark_failed` → `enable` (load N+1) before the reaper task is released → release it → load N+1's tools remain registered, its pid alive, row `enabled`. **Declaration gone:** delete the block → `reconcile_all` → pid gone, tools withdrawn, record absent from `list()`, file byte-identical to the edit (no write attempted). **Watcher path:** hand-edit `enabled = false` → `reconcile_all` → `Disabling` → the same three refusals, since edge case 15 is the one disable path with no route behind it. **Crash test, written to what `reconnect` does (§3.6 item 1):** first prove the recovery — kill the child out-of-band with the command still runnable, next call succeeds (transparent respawn) and the row stays `active`; then make the command un-spawnable (the test's server is a temp-dir script; delete it) and drive **four** consecutive calls — each fails with its own handshake error, the fourth with `ReconnectExhausted` → `mark_failed` → row reads `failed/crashed`, tools unpublished; **only now** assert no respawn (the reaper's T4 seal), restore the script, `enable` recovers with a new generation. **`mcp_supervisor_records_every_registered_tool`** — `ledger.audit()` is empty after `reconcile_all`. **After C2 the MCP toggle is fully functional through the supervisor API, with no HTTP route yet.** |
| **C3** | **Plugin supervisor** (~340 + 150 test) | `PermissionEntry.enabled` (serde default true) **+ `approved: Option<bool>` / `approved_at: Option<String>` (tri-state consent, §5)**, `is_approved()` → `entry.approved`, `approve`/`deny` as entry-preserving read-modify-write, `set_enabled` creating a decision-less entry; fail-closed `load_permissions_table` (`:140`) with `enabled: null` + `409 store_unreadable` rows (§4); atomic `save_permissions_table` (`:156`) on C1's helper; the consent-first gate at `manager.rs:284` (§6.2 #7); E1 drift check; `deny_plugin` full unload (`:601`, T5-deny); `enable_plugin` stops approving (`:638`); `disable_plugin` stops denying (`:682`); CAS no-op on redundant enable; **`PluginToolProxy::new(.., generation)` at `:831` plus the same number into `PluginSkillBridge` (`:419`) and `PluginAgentBridge` (`:452`)**, `generation()` implemented on the proxy; T4 awaits `child.wait()` under 2 s; `PluginStatus` → `ExtensionState` **with the `legacy_status_word` shim on `PluginInfo.status`** so `/v1/plugins` and `PluginsSection` keep working until C7 (§4.3); `register_plugin_skill` id lowercase (`catalog/mod.rs:529`); `PluginManager` implements `ExtensionSupervisor`; `shutdown_all` on the daemon shutdown path; **T2 step 1 tombstones virtual caps**; **T2 step 4** — `PluginManager` clears `registered_connector`/`registered_provider`/`registered_models` on disable and holds the deregistration seam for when the bridges are wired (§3.2); **run-guards** at `invoke_plugin_skill` (`invocation.rs:934`) and the `run_plugin_agent_loop` call site (`lead_agent/tools.rs:513`) — `begin_run(ext, bridge.generation())`, refusing `Stale` at pre-flight — + the step-boundary `ledger.check()` in `plugin_agent.rs`; **crash detection** — proxies take the ledger + generation, `warn!` and `mark_failed(ext, generation, ..)` on `ChannelClosed`/`ProcessCrashed`, `try_wait` sweep under the `plugins` **write** lock in `reconcile`/`list` (no `.await` under it), reaper task with the §3.6 re-check; **step W write-first** for `enable`/`disable`/`approve`/`deny` with `500` + no CAS on failure; **`PluginManager::with_event_bus(bus)`** wired at `main.rs` beside the existing `with_event_sink` (`:343`) so T5/E5 publish `SystemEvent::ExtensionStateChanged` before C4 installs the ledger's bus — the six legacy `emit(ServerEvent::Plugin*)` producers keep firing until C7 (§7.3). | Deny on a running plugin kills the child (`child.wait()` returns) and unregisters tools/skills/templates. Disable on an unapproved plugin leaves `unapproved`, `enabled: false`, writes a decision-less entry, and a restart reads the same (`never_seen`, not `denied`). **`stale_proxy_channel_closed_after_reenable_does_not_flip_row`** — hold a proxy from load N, disable, re-enable (load N+1), call the old proxy: it returns the `Stale` refusal, logs one `warn!`, and the row stays `enabled` with load N+1's process alive. Redundant enable registers no second capability provider. Manifest capability growth re-prompts. Corrupt `.permissions.toml` loads nothing and overwrites nothing. Plugin skill with a mixed-case id is reachable by `/slash` and removed on unload. A template naming only a **virtual** capability classifies `withheld` (not `unknown`) after disable. Kill the child out-of-band → next `list()` reads `failed/crashed`, next call refuses with attribution. **Reaper superseded:** `mark_failed` → `enable` (load N+1) before the reaper runs → reaper runs → load N+1's process alive, tools registered, row `enabled`. **`plugin_skill_frontmatter_never_carries_cron`** — a `skill/info` payload carrying `invoke.cron` still yields `invoke.cron == None` (§10 case 5). **`plugin_supervisor_records_every_registered_tool`** — `audit()` empty after `start()`. **Guard test for S2 residue:** after `disable`, the row reads `connector: null, provider: null` and `LlmRouter::list_models_for_provider(..)` returns nothing for the plugin's provider. **Drain sees runs:** disable during a stubbed multi-second `skill/invoke` waits for it (or hits the deadline) and the caller receives the S4 refusal, never a channel-error string; a plugin agent mid-loop stops at its next step with the S4 refusal. `/v1/plugins` still serialises `running`/`disabled`/`waiting-approval` words. |
| **C4** | **Warning path** (~300 + 120 test) | `SystemEvent::ExtensionCapabilityWithheld` + `ExtensionCapabilityWithdrawn`; `ServerEvent` peers **with `ts` and `instance_id`**; `event_bridge` arms; event-log persistence arm; **the one production `ToolRegistry::with_event_bus(bus)` call at `services/tools.rs:25`** (§7.1); the 10-min dedup; the T1-step-3 dependent scan against the withdrawn set (§7.3) — the supervisors take `default_lane_key` (`main.rs:199`) here, and `McpSupervisor` additionally takes the agent-registry and skill-catalog handles `PluginManager` already holds (`main.rs:338-339`); the scan's wording keyed on state so the reaper path reads *crashed*; **the cron notice path** — `pub use outcome::persist_conversation` re-exported from `orchestrator::dispatcher` (`dispatcher/mod.rs:6` is `pub(crate) mod outcome`), `NotificationDispatcher::handle_extension_notice` (write to `notice_lane` with `source = "gui"` + cross-channel fan-out via the existing `try_cross_channel_*` helpers with the user id derived from `notice_lane`), `extension_capability_withdrawn` → `qk.chat.all()` in the GUI map; wire `withheld`/`partially_withheld` into `resolve_agent_tools`, `invocation.rs:152`, `invoke_executor.rs:157`, `invocation.rs:954`. | Dedup: 100 blocked attempts in one task → 1 warn; 8 spawns from one template → 1 warn. Attributed vs partial vs unattributed classification. Disable emits exactly one dependent-scan warn naming the affected templates and skills. **Notice reaches the default lane:** disabling an extension with one cron dependent inserts exactly one `assistant` row on `{local_user_id}:gui` (read back through `ConversationRepository::list_by_lane`; the conversation's `source` is `"gui"`) and broadcasts exactly one `ServerEvent::ExtensionCapabilityWithdrawn`; a second disable of an unrelated extension inserts nothing. |
| **C5** | **Fail-closed + availability** (~250 + 120 test) | **The security commit.** Plugin-skill total-loss refusal (`invocation.rs:951-973`) — the empty-allowlist escalation. File-based total-loss refusal at **both** file-skill sites: `invocation.rs:152` and the nested-skill path `invoke_executor.rs:157` (§6.2 #10). `CapabilityOracle` implemented by `ToolRegistry` over `resolve_capabilities` and installed on `SkillCatalog` via `set_availability_oracle` (§6.2 #12); router candidate filter (`router/mod.rs:101`); `catalog_summary` / `<available_skills>` / `invoke_skill` listing filters; explicit-slash refusal **returned as `Ok(reply)` from `invoke_skill_with_telemetry`** (§7.5); cron skip (`scheduled_skills.rs:147`); catalog + agent-registry tombstones for withdrawn plugin contributions. | **Assert the escalation is closed:** a plugin skill whose every capability is withheld cannot call an unrelated builtin. Auto-route drops the skill; `/slash` returns the named error; cron fire is skipped. |
| **C6** | **Routes + CLI** (~420) | `AppState.tool_registry` + `AppState.extensions: Arc<Extensions>` (§6.2 #15 — folds the existing `plugin_manager` field in; clone the registry `Arc` before `main.rs:373`); `GET /v1/extensions`, enable/disable/approve/deny/config, **`DELETE /v1/extensions/plugin/{id}`** (orphaned-only); `GET /v1/tools` (**GAP-18**); router mount; `openalpaca ext list\|info\|enable\|disable\|approve\|deny\|remove`; `openalpaca plugin …` re-pointed. **`/v1/plugins*` is not deleted here** — it survives one commit so the GUI never calls a missing route (§8). | Route tests for every status code in §8, including 200-on-failed-bringup, 409-on-approve-for-mcp, 409-on-delete-not-orphaned, and disable-on-unapproved returning `unapproved`/`enabled:false`. MCP gains a CLI surface for the first time. |
| **C7** | **GUI + old-route removal** (~400 TS, ~−120 Rust) | Extensions section replacing Plugins; Tools section replacing the misnamed Skills rows; the `checked={record.enabled}` fix; state→row mapper with explicit `tone` and the `unapproved` `enabled` suffix; GAP-24 in `unavailable.ts`; GAP-18 note text corrected; WS invalidation entries; type updates; `ToolCatalogEntry.denied` deleted. **In the same commit:** delete `/v1/plugins*` (`routes/plugins.rs`), the six `plugin_*` `ServerEvent` variants **and their producers** — `self.emit(ServerEvent::Plugin*)` at `manager.rs:294`, `:331`, `:504`, `:571`, `:611`, `:684`, the `PluginEventSink` type (`:153`), `with_event_sink` (`:193`), `emit` (`:199`), the `main.rs:343` wiring and the test sink at `manager.rs:1154` — plus their WS/persistence arms, the GUI's `plugin_*` mappings and union members, and C3's `legacy_status_word` shim (§4.3, §7.3, §8, §9.5). | `bun run check`, `bun run test`; `cargo build --workspace` (the route file is gone). Manual: every row in §9.2 renders distinctly; a crashed-but-enabled plugin shows the switch ON; an orphaned row's Remove works. |
| **C8** | **`global_tool_deny` purge + docs** (~150 deleted, net negative) | §11.1 in full — all 16 files plus `CLAUDE.md`, including `docs/agent-loop.md:46,:104`, `docs/Skill_Template_Reference.md:208,:525` and the `:540` config-key row (deleted), and `docs/Daemon_Manual.md:97`; the stale-key `WARN` probe; CLAUDE.md (`:136`, `:154`, the config table row, an Extensions line in the extensibility section); `docs/GUI_Manual.md` (new Extensions section, Plugins section removed); the `config/mcp.toml` comment; the two `tool_selection` comment blocks (§11.1); **ADR-030** in the Obsidian vault superseding ADR-029 — citing the existing correction at `10-decision-log.md:317` (which already records that "extension-only by construction" was wrong) rather than restating it; `tasks/api-fix-plan.md`: N5 marked resolved, the Phase 8 GAP-18 bullet (item 2) re-pointed here, and **GAP-19 relabelled GAP-24** in Phase 8 item 9 and the summary row (line 823) so the plan and the GUI `GapId` union never diverge. | `cargo build --workspace && cargo test --workspace && cargo clippy`; `grep -rn global_tool_deny . --include='*.rs' --include='*.toml' --include='*.md'` returns **only** the probe's own string and this design document. |

**Sequencing:** C1 alone first (additive, zero behaviour change, and its three gate tests are the property the
whole design turns on). C2 and C3 both depend only on C1 — that is why C1 carries the `ExtensionSupervisor`
trait, the generation plumbing, `drain_timeout_secs` and `atomic_write_toml` — and can then proceed in
parallel, but they are **not** disjoint: C2 edits
`apps/openalpacad` + `crates/openalpaca_mcp` + the `Mcp` arms in `registry/mod.rs`, C3 edits
`crates/openalpaca_plugins` + `openalpaca_core` (`invocation.rs`, `plugin_agent.rs`, `lead_agent/tools.rs`,
`catalog/mod.rs`). Whichever lands second rebases across `registry/mod.rs`. Then C4→C5, then C6→C7→C8. **C5
deserves its own review pass**: it is small in lines and security-relevant.

**Smallest shippable slice with real value:** C1+C2 — MCP servers gain a working, S2-honouring toggle for the
first time. **Smallest slice the owner can see:** C1+C2+C6+C7. **C4 and C5 cannot be deferred** without
violating S4 and leaving the escalation open.

### 12.1 Relative to `tasks/api-fix-plan.md` Phase 8

Phase 8 (line 764) holds three relevant items:

- **GAP-18 (`GET /v1/tools`, `/v1/skills`)** — line 769 already scopes the `AppState` plumbing this design
  needs (*"Clone the tool-registry `Arc` before its move into `Orchestrator::new` (`main.rs:373`); two
  `AppState` fields"*) and states that *"Per-tool enable writes stay out of scope pending the N5 design"* and
  that when N5 lands *"it supersedes this bullet and likely reaches further than `/v1/tools`."* **It does.**
  C6 ships GAP-18's `/v1/tools` in its final shape: `origin` replaces `denied`, and `/v1/skills` is unchanged
  from the plan. **This design's C6 subsumes and replaces Phase 8 item 2.**
- **GAP-19 (plugin install)** — line 776, `source:"path"` only, lands a directory in `WaitingApproval`. Still
  valid and **unchanged in mechanism**, but under this design a freshly installed plugin lands
  `enabled = true` (serde default) + `consent = NeverSeen`, so **approving is the single action that starts
  it**. **Disposition, stated once:** GAP-19 is **renamed GAP-24** — not subsumed, not deleted — and widened to
  cover MCP-server add/remove. It remains scheduled as Phase 8 item 9 with its mechanism unchanged, and it is
  **not** in this plan. C7 replaces `"GAP-19"` with `"GAP-24"` in `apps/openalpaca-gui/src/lib/unavailable.ts`
  (union at `:37`, entry at `:233`); C8 relabels item 9 and the summary row at line 823 in the fix plan in the
  same change, so the 23-gap registry and the plan agree on landing.
- **GAP-20 part 2 (agent-template `enabled`)** — line 772 / 857. Untouched. It is the ALLOW axis's own
  question and needs enforcement in the spawn path; nothing here changes its status.

**Ordering:** this work is **independent of Phases 1–7** and can land before or after them, with one caveat —
C6 and Phase 8's GAP-18 both add the same two `AppState` fields, so whichever lands first owns that edit and
the other rebases. Recommend landing this design **first**, since GAP-18's shape is defined by it.
The `~/.openalpaca` re-home (Phase 1) moves `config/` and the plugins root; this design touches only paths
already resolved through `resolve_config_base_dir()` and `paths::app_dir()`, so it rebases cleanly either way.

---

## 13. Open questions for the owner

Four. Each has a recommended default that takes effect if you say nothing. Nothing here is settled by S1–S4.

**Q1. Does a disabled extension's row show the tools it *used to* provide?**
Today nothing caches a disabled server's tool names, so after a restart a disabled row lists zero tools and its
capabilities warn as *"unknown"* rather than *"withheld by github"*. Caching them across boots would make the
row and the warning more informative, at the cost of a stale-cache class of bug (the cache lies after you edit
the server's command).
→ **Default: do not cache.** Empty list, less precise warning after a restart, no stale cache. The row still
reads `disabled`, so nobody is stranded.

**Q2. Should the cron notice go to your default lane, or nowhere?**
When a disable makes a scheduled skill unsatisfiable, §7.3 writes **one** assistant message into your default
lane's conversation (so it shows in the GUI chat) and fans it out to any connector you use, so an unattended job
does not just quietly stop. The alternative is log + event only, visible in the GUI event log.
→ **Default: post it, once per transition.** It is the only failure mode with no human in the loop, and it
cannot repeat. (Rev 1 routed this through the workflow-progress path, which never reaches the GUI lane; rev 2
routes it through the conversation store, which does.)

**Q3. What should the drain deadline be?**
How long a disable waits for in-flight tool calls and plugin runs before tearing down under them. Shorter feels
snappier and risks cutting a long call; longer makes the toggle feel unresponsive.
→ **Default: 10 s**, as `[extensions] drain_timeout_secs` in `daemon.toml` — the single source; there is no
per-request policy at the supervisor level to blend it with.

**Q4. Should re-approving after capability growth offer "always trust this plugin's future capabilities"?**
The E1 drift check re-prompts whenever a plugin's manifest grows between a disable and a re-enable. For a
plugin you update often, that is a recurring prompt with no escape hatch.
→ **Default: no escape hatch in v1.** The prompt shows only the delta ("Now also asks for: X, Y"), which is
short and cheap to accept. A blanket-trust flag can be added later if the prompting proves annoying; it cannot
be removed once people rely on it.

---

## Revision log

- **rev 1** (2026-09-01) — first full design of record; superseded ADR-029.
- **rev 2** (2026-09-01) — critique round 0 (2 directive violations, 5 gaps, 5 unverified claims, 5
  contradictions) repaired. S4: cron notice re-routed from the connector-only `handle_progress` path to
  `persist_conversation` + cross-channel fan-out + WS invalidation (§7.3). S2: plugin connector/provider
  contributions withdrawn at T2 with the deregistration seam named (§3.2). Drain redesigned to cover
  out-of-process plugin runs via run-guards (§3.2 T3). Unrecorded-extension default stated as fail-open with an
  audit and three named tests (§6.2a). Tombstone made `Set<ExtensionId>` with partial withdrawal defined (§7.2).
  `ToolRegistry::new()` left arg-free (`with_event_bus`, §7.1). `Orphaned` made plugin-only in the transition
  table (§4.1). Drain deadline unified on `drain_timeout_secs` (§3.2, §10, §13). §6.1 re-derived: five of six
  surfaces derive policy from the assembled list. §7.4 dedup re-grounded on attempted-use and per-spawn
  frequency. §6.2 #14 bounded precisely (`get_by_id` has no fallback; leak iff name ≠ id). §11.1 re-run: 41
  hits / 16 files; no `global_tool_deny` key in either shipped config. GAP-19 → GAP-24 fixed as a rename that
  stays scheduled. Decision-log correction at `:317` found already present; ADR-030 cites it.
- **rev 3** (2026-09-01) — critique round 1 (3 blocking, 1 carried over, 28 non-blocking) repaired. **S4:** the
  gate gained a miss arm — three of the four "snapshot" sites clone conditionally (`invocation.rs:593`,
  `invoke_executor.rs:267`, `simple_query_handler.rs:625`), so the ordinary skill runs on the live registry
  and hit `Tool 'x' not found` after T1; the ledger now retains `tool_names` and `owner_of()` attributes the
  miss (§3.0, §6.2 #1, §7.1, C1 test). **State model:** invariant made one-directional (`Disabled ⇒
  bit=false`), consent pre-empts the switch; §4.1 rewritten with the bit's effect per cell, `Enabling → E1 →
  Unapproved` added; #7 and the §8 deny/disable contracts aligned. **S3 Crashed:** §3.6 added — lazy
  `mark_failed` on `ReconnectExhausted` (MCP) / `ChannelClosed|ProcessCrashed` (plugin proxies), `try_wait`
  sweep on list, reaper runs T1–T4 without T5; no poller. **Consistency:** `Arc<McpClient>` in the supervisor
  only (§5, #6); `ExtensionSupervisor`/`McpSupervisor`/`PluginManager`/`Extensions` named once (§3) and used
  in #15/C6; `/v1/plugins*` + `plugin_*` WS + status shim deleted together in C7; `ExtensionStateChanged`
  moved to C2, `with_event_bus` call and `default_lane_key` to C4; C2/C3 dependency on C1 stated and the
  writer homed in `openalpaca_core`; plugin virtual caps tombstoned at T2; `skipped_tools` and `DELETE
  /v1/extensions/plugin/{id}` added to §8; §7.3 scan intersects the withdrawn set, derives the user id from
  `notice_lane`, passes `source="gui"`, and re-exports `persist_conversation`; T4's MCP mutex bound stated;
  `SandboxManager` production count corrected to 7; `LlmRouter::list_models_for_provider` named; boot-window
  clause dropped from §6.2a; ~20 line anchors corrected; §11.1 now lists all 16 files + CLAUDE.md and all six
  prose-only lines.
- **rev 4** (2026-09-01) — critique round 2 (3 blocking, 1 carried over, ~40 non-blocking) repaired. **S4 /
  correctness — incarnations:** §3.0 gained Fact 3 (a snapshot can outlive an incarnation); every load has a
  ledger `generation` bumped at E0, stamped into `ToolBackend::Mcp { generation }` (`bridge.rs:46`) and into the
  three plugin bridges (`manager.rs:831`/`:419`/`:452`) via a `PluginToolExecutor::generation()` default method;
  the hit arm refuses a stale handle with attribution (`Stale` wording, §7.1) and `mark_failed(ext, generation,
  ..)` ignores a non-current one — closing the false `Failed{Crashed}` teardown a stale proxy would have caused
  under rev 3's §3.6 item 2. §10 case 17, C1 test (iii), C3 stale-proxy test. **Consent tri-state:**
  `approved: Option<bool>` / `approved_at: Option<String>` so a decision-less entry can carry a pre-set bit
  (§2.2, §5, §5.1, §6.2 #7, C3); `approve`/`deny` made entry-preserving. **Wire contract:**
  `Enabling`/`Disabling` reported literally (§4 comment fixed); `approve` on `Enabled`/`Failed` re-records
  consent and never loads (§4.1, §8); `enabled: null` + `409 store_unreadable` for rows with no readable bit
  (§4, §5.1, §8); `503` dropped; `ExtensionCapabilityWithdrawn` given one field list (§7.3);
  `invocations_today` sourced from `tool_execution_log`. **Mechanism:** dependent scan + cron notice homed in
  T1 step 3 so the reaper and watcher fire it (§3.2, §3.6, §7.3); `check` counts before it reads (T0); T4
  awaits `child.wait()`; T5-deny stated; `try_wait` sweep under the write lock; plugin proxies `warn!`
  themselves; `check(.., Option<&ToolContext>)` for `execute()`; §3.6 and §10 case 7 restated around what
  `reconnect` really does (transparent stdio respawn; `ReconnectExhausted` after four consecutive failed
  entries) and C2's crash test rewritten to that sequence, plus the watcher path. **Plan:**
  `ExtensionSupervisor` trait homed in `openalpaca_core` (C1); `McpSupervisor` parked on the services bundle
  C2–C6; C4 hands it the agent-registry/skill-catalog handles; C5 covers `invoke_executor.rs:157` and returns
  the `/slash` refusal as `Ok`; C1 test (ii) asserts the `warn!`, not the C4 event. **Claims re-derived:**
  no production direct `registry.execute*` callers (§3.2 T3(a), §6.3); `.backend` grep list completed; 79
  `RegisteredTool` literals; `load_daemon_config` deserialises directly (§11.1 probe adds the `toml::Value`
  parse). ~20 anchors corrected (`state.rs:39`, `routes/plugins.rs:49`, `PluginsSection.tsx:125`,
  `manager.rs:601`/`:638`/`:1065`, `invocation.rs:679`/`:201`, `simple_query_handler.rs:631`,
  `registry/mod.rs:635`/`:334-355`, `query_handler/mod.rs:212-238`, `settings/SettingsView.tsx`,
  `daemon_config_cli` path and order, plugin skills never cron-registered).
- **rev 5** (2026-09-01) — critique round 3 (4 blocking, 1 carried over, 26 non-blocking) repaired. **S2 —
  seal:** T4b now checks `closed` at `do_handshake`'s install point under the service lock as well as at
  `reconnect`'s entry, closing the in-flight-reconnect window (`reconnect` releases the lock before its sleep,
  `client.rs:210-218`; `do_handshake` installs at `:137` unchecked); §6.2 #5, §10 case 7(c), C2 test named.
  **Write-first:** step W added to §3.2/§3.3 — the bit is written before the CAS on both verbs, `500` + no CAS
  on failure; T5/E5 are memory-only; invariant restated (`Disabled|Disabling ⇒ false`,
  `Enabled|Enabling|Failed ⇒ true`); §4.1 header/`Disabling` row, §8 enable/disable, §1 example B, case 15
  (watcher sees the daemon's own write; hash ring swallows it) aligned. **Reaper:** message carries the
  generation; the reaper re-checks `Failed{Crashed}` + generation under the mutex, never writes state; T1–T4
  idempotent, step 3 fires only on a non-empty withdrawn set; C2/C3 superseded-reap tests. **Declaration
  gone (MCP):** T5-gone runs T0–T4 with no write (the writer's re-parse would reject a synthesized tagless
  table, `config.rs:46-47`) and a general off-route persistence-failure rule. **Carried over:** §10 case 5's
  cron claim re-grounded on `build_skill_frontmatter_from_info` (`manager.rs:940-948`) — `sync_all` does
  iterate plugin entries — and pinned by `plugin_skill_frontmatter_never_carries_cron`; slash tombstone made a
  separate map since `remove` scrubs the indices. **Non-blocking:** C2 observes MCP child exit via a pidfile
  (rmcp `ChildWithCleanup::drop` kills detached; the daemon has no handle); `PluginManager::with_event_bus`
  in C3 and the six `emit` producers listed for C7; `invocations_today` sourced from the event-persistence
  writer with the local-midnight-in-UTC predicate; `rmcp_tool_to_registered`'s three test callers listed;
  §11.2 deny-key bullet reworded; `CapabilityOracle` homed on `ToolRegistry`; "directory returns" bound to
  the next `reconcile_all`; T4 spawns a fresh `disconnect` future; config route invokes `enable`;
  `begin_run` takes the bridge generation; disable bound gains the mutex wait; `CallGuard` held across the
  await; `list()` sweep awaits nothing under the lock; `since` defined for every state; §9.2 opts into
  `include_orphaned`; `on_crash` slot is a `OnceLock`; `Enabling` wording says "Retry"; case 13 collision rule
  fixed to "blocked only by a live incumbent"; E0 provider-leak wording bounded to plugins with virtual
  capabilities; C2 adds `scripts/release/templates/config/mcp.toml`; anchors corrected (`main.rs:337`,
  `:337-338`, `client.rs:53`, `unavailable.ts:18-41`).
