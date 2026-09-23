# Tool System — Design Document

> **Last verified against the code on 2026-09-20** — `crates/openalpaca_core/src/tools`,
> `crates/openalpaca_core/src/security`, `crates/openalpaca_core/src/runner`,
> `apps/openalpacad/src/services`, `apps/openalpacad/src/managers` and the shipped
> `config/`. Workspace version 0.1.0.
>
> Covers the architecture, design decisions, and conceptual model of the
> OpenAlpaca tool system.  For implementation details, API surfaces, and
> code-level reference see the companion [TECHNICAL.md](./TECHNICAL.md).
> For how a turn is routed and how the agentic loop runs, see
> [agent-loop.md](../agent-loop.md).

---

## 1. Purpose & Scope

The tool system gives OpenAlpaca agents the ability to *act* on the world
beyond pure language generation.  A **tool** is any callable unit of work
— reading a file, searching the web, running a shell command, calling an
MCP server — that an agent may invoke during an agentic loop iteration.

### Design Goals

| # | Goal | Rationale |
|---|------|-----------|
| 1 | **Least-privilege by default** | An agent receives *zero* tools unless its declared capabilities intersect a tool's provided capabilities, and the sandbox refuses any call outside the surface the agent was handed. An empty allow list admits nothing. |
| 2 | **Defense-in-depth security** | Independent security layers (capability check → input sanitization → confirmation gate → circuit breaker → timeout → extension gate) protect every tool call. |
| 3 | **Extensibility without recompilation** | Users add tools via TOML config files, external MCP servers (`config/mcp.toml`), or out-of-process plugins; no Rust code required. |
| 4 | **Two governance axes** | **ALLOW** is per agent (which capabilities a template or skill declares). **ENABLE** is per extension (one switch per MCP server and per plugin). Built-in tools are never switched off. |
| 5 | **No silent degradation** | A tool that is withheld is refused with a message naming the extension that owns it, and the refusal is logged. A skill that lost a required tool is refused rather than run without it. |
| 6 | **Parallel execution** | All tool calls within a single LLM round execute concurrently via `join_all`. |
| 7 | **Runtime registration** | The registry is lock-free and mutable: MCP and plugin tools register and unregister while the daemon runs. Per-request tools live in a per-request copy of the registry. |
| 8 | **Owner-scoped data isolation** | Tools accessing user-specific data (memory, persona, artifacts) take identity only from a trusted per-invocation `ToolContext`, never from LLM-supplied arguments. |

### Non-Goals

- **In-process plugin binaries.** There is no dynamic `.so`/`.dylib`
  loading.  Extension code runs out of process: MCP servers are external
  programs, and plugins are child processes speaking JSON-RPC over stdio.
- **MCP server mode.** OpenAlpaca connects *out* to MCP servers and imports
  their tools; it does not expose its own tools over MCP.
- **A per-tool on/off switch.** The ENABLE axis stops at the extension.
  To take one tool away from one agent, leave its capability out of that
  agent's template.
- **Cross-agent tool sharing at runtime.** Each agent's tool set is resolved
  at dispatch time and fixed for the lifetime of that agent run.
- **Fine-grained per-field authorization.** The security model operates at
  tool-name granularity, not per-parameter.

---

## 2. Conceptual Model

### 2.1 Tool Lifecycle

```text
              ┌──────────────┐
              │  DEFINITION  │   TOML config, Rust code, MCP server,
              │              │   or plugin manifest defines name,
              └──────┬───────┘   schema, backend
                     │
              ┌──────▼───────┐
              │ REGISTRATION │   ToolRegistry (DashMap) stores tools;
              │              │   built-ins + TOML at startup, MCP and
              └──────┬───────┘   plugins when their extension loads
                     │
              ┌──────▼───────┐
              │  RESOLUTION  │   Each caller assembles its surface:
              │  (dispatch)  │   capabilities → tools, minus denials,
              └──────┬───────┘   minus extensions that are not enabled
                     │
              ┌──────▼───────┐
              │  INVOCATION  │   LLM returns ToolCall in response;
              │  (runtime)   │   agentic loop dispatches execution
              └──────┬───────┘
                     │
              ┌──────▼───────┐
              │  EXECUTION   │   SandboxManager security checks
              │  (runtime)   │   → ToolRegistry extension gate
              └──────┬───────┘   → backend (with ToolContext identity)
                     │
              ┌──────▼───────┐
              │   FEEDBACK   │   Result appended as a tool_result
              │  (runtime)   │   message; an oversized result is
              └──────────────┘   spilled to the session log or cut
```

### 2.2 Tool Categories (by backend)

Every registered tool has a `ToolBackend` that executes its logic:

| Backend | Examples | Source | Notes |
|---------|----------|--------|-------|
| **BuiltIn** | `shell_execute`, `file_read`, `file_write`, `artifact_write`, `read_result`, `web_search`, `web_fetch`, `memory_search`, `workspace_read`/`workspace_write`, `update_persona`, `send` | Rust code, registered at startup | Tools needing identity override `execute_with_context()` |
| **BuiltIn (per request)** | Chat: `start_workflow`, `task_status`, `memory_store`, `memory_forget`, `invoke_skill`, `steer_workflow`, `queue_followup`. Lead agent: `spawn_subagent`, `spawn_subagents_batch`, `check_subagent_status`, `wait_for_subagents`, `post_update`, `queue_followup`, `invoke_skill` | Built fresh for one chat turn or one lead-agent run | Registered into a per-request copy of the registry, never the shared one (§4.2) |
| **BuiltIn (skill scripts)** | `skill_script:analyze`, `invoke_skill:<id>` | Skill `SKILL.md` frontmatter | Available only during that skill's invocation |
| **Http** | User-defined REST API wrappers | `config/tools/*.toml` | SSRF-validated, response capped |
| **Command** | User-defined CLI wrappers | `config/tools/*.toml` | Treated as shell-like for injection sanitization |
| **Mcp** | `<server>__<tool>` (e.g. `fs__read_file`) | `config/mcp.toml` | Author recorded as `mcp:<server>`; on the ENABLE axis |
| **Plugin** | `<plugin>::<tool>` | Plugin directory, approval-gated | Executes via out-of-process JSON-RPC; author `plugin:<name>`; on the ENABLE axis |

Every `RegisteredTool` also carries provenance and policy metadata:
`provides_capabilities`, `exempt_from_timeout`, optional MCP-style
`annotations`, `version`, `author`, and `created_at`.  Which extension owns
a tool is derived from its backend (the MCP server name, the plugin
directory name), never from the free-form `author` string.

---

## 3. Architecture

### 3.1 Layer Stack

```text
┌──────────────────────────────────────────────────────────────────┐
│                        AGENTIC LOOP                              │
│  run_agentic_loop_routed()                                       │
│  • Receives LLM response with tool_calls                         │
│  • Enforces max_tools_per_round budget (overflow calls errored)  │
│  • Executes tools in parallel via SandboxManager                 │
│  • Spills or cuts oversized results, feeds them back to the LLM  │
├──────────────────────────────────────────────────────────────────┤
│                     SECURITY SANDBOX                             │
│  SandboxManager::execute_tool(tool_call, policy, ctx)            │
│  1. Capability check   (allow list + deny list, by tool name)    │
│  2. Input sanitization (InputSanitizer)                          │
│  3. Confirmation gate  (approval cache, auto-approve, unattended │
│                         refusal, ConfirmationBroker)             │
│  4. Circuit breaker    (ToolCircuitBreaker)                      │
│  5. Timeout-wrapped execution (skipped for exempt tools)         │
│  6. Outcome recording + event emission                           │
├──────────────────────────────────────────────────────────────────┤
│                      TOOL REGISTRY                               │
│  ToolRegistry::execute_with_context(name, args, &ToolContext)    │
│  • EXTENSION GATE: refuses a tool whose MCP server or plugin is  │
│    not enabled, or whose handle belongs to a previous load       │
│  • JSON Schema argument pre-validation (required fields,         │
│    per-property types, enum values)                              │
│  • Dispatches to BuiltIn / Http / Command / Plugin / Mcp backend │
│  • BuiltIn backends receive the ToolContext; other backends      │
│    don't need identity and ignore it                             │
│  • Shared reqwest::Client with SSRF-checked redirect policy      │
└──────────────────────────────────────────────────────────────────┘
```

Identity flows through **`ToolContext`**, a lightweight per-invocation
struct built by the sandbox caller (never by the LLM).  It carries:

| Group | Fields |
|-------|--------|
| Who | `agent_id`, `agent_instance_id`, `owner_id`, `principal` |
| Which run | `task_id`, `request_id`, `session_id`, `lane_key`, `source` |
| Which workspace | `workspace_id` (memory scoping), `request_workspace_root` (the only field that may place files on disk), `workspace_path` (as sent by the client) |
| Skill nesting | `skill_stack`, `effective_constraints` |
| Side channels | `session_log` (for `file_write`'s pre-edit snapshot), `event_bus` (filled in by the sandbox so a tool can announce what it produced) |

### 3.2 The Extension Ledger

One **`ExtensionLedger`** (`tools/extensions/`) holds the state of every MCP
server and plugin: `enabled`, `disabled`, `unapproved`, `failed`,
`orphaned`, plus the transient `enabling` and `disabling`.  It is pure
bookkeeping — it never holds a client, a process or a file path.

- Two **supervisors** own the lifecycles: the MCP supervisor in the daemon
  (`apps/openalpacad/src/managers/mcp.rs`) and the `PluginManager`
  (`crates/openalpaca_plugins`).  Both record what they load in the ledger.
- The registry shares the ledger by reference, so even a per-request *copy*
  of the registry reads live extension state at the gate.
- Each load of an extension gets a **generation** number, stamped on its
  tools.  A run holding a tool handle from before a disable → re-enable is
  refused as stale instead of talking to a dead transport.
- The ledger remembers which extension *used to* provide a tool name or a
  capability.  That is what lets a refusal say "provided by MCP server
  `fs`, which is disabled" instead of "tool not found".

### 3.3 Skill Invocation Executor

Synthetic `invoke_skill:*` tool calls (from a skill's `depends_on`) and the
general `invoke_skill` tool (offered to the chat loop and the lead agent)
are handled by `SkillInvocationToolExecutor`
(`orchestrator/skill/invoke_executor.rs`).  It runs the invoked skill as a
nested agentic loop, carrying a call stack (bounded depth, cycle check),
sharing the parent's cost accumulator and cancellation token, and
intersecting the parent's tool constraints with the child skill's
(`compose_constraints` / `filter_tools_by_constraints`).  See
[Skill_Template_Reference.md](../Skill_Template_Reference.md) §8.

---

## 4. Design Decisions

### 4.1 Capabilities-to-Tools Mapping (the ALLOW axis)

**Decision:** Agents declare *capabilities* (and optionally
`denied_capabilities`) in their template frontmatter; tools declare
`provides_capabilities`.  `resolve_agent_tools()` includes a tool when any
of its capabilities matches any agent capability and none is denied.

**Rationale:**
- Decouples agent configuration from tool implementation details.
- One capability can map to multiple tools (e.g. `web_access` →
  `web_search` + `web_fetch`), and one tool can serve multiple capabilities.
- Deny lists let an agent exclude whole capability groups (e.g.
  `denied_capabilities: [web_access]` for an offline coding agent).

**Built-in capability names:** `file_read`, `file_write`, `artifact_write`,
`shell_execute`, `memory_read` (memory_search), `read_result`,
`web_access` (web_search, web_fetch), `workspace_read`, `workspace_write`,
`persona_write` (update_persona), `messaging` (send), `orchestration`
(lead-agent coordination tools).

**Extension capabilities:** an MCP tool provides one capability equal to its
own namespaced name (`fs__read_file`), so a template selects it by listing
that name.  A plugin's tools all provide the capabilities its manifest
declares under `capabilities.provides`.

**Ambient grants:** every subagent gets `workspace_read` and
`workspace_write` whether or not its template lists them.  Nothing else is
ambient.  `read_result` and `artifact_write` are per-template grants; all
nine shipped templates list `read_result`.

**Virtual capabilities:** a `CapabilityProvider` extension point derives
additional capabilities from tool metadata.  The built-in
`AnnotationCapabilityProvider` derives eight `annotation:*` capabilities
from MCP-style annotation hints (`annotation:readonly`,
`annotation:destructive`, `annotation:idempotent`, `annotation:open_world`,
plus their `annotation:non_*` inverses), letting an agent deny e.g.
"all destructive tools" without naming them.  A plugin can register its own
provider from its manifest.

### 4.2 Lock-Free Mutable Registry, Per-Request Copies

**Decision:** `ToolRegistry` is backed by `DashMap` and shared as
`Arc<ToolRegistry>`.  `register()`, `replace()` and `remove()` take `&self`
and are safe at any time.  Tools that exist for one request are registered
into a **value copy** of the registry made for that request.

**Rationale:**
- MCP servers and plugins register tools when they load and remove them
  when they are disabled.  A build-once immutable map cannot support this.
- DashMap gives lock-free concurrent reads on the hot path without an
  `RwLock` around the whole registry.
- The chat loop's `start_workflow`, the lead agent's `spawn_subagent` and a
  skill's `skill_script:*` tools are bound to one turn, one run or one
  invocation.  Putting them in the shared registry would leak them into
  every other tool listing, so each of those callers copies the registry
  and adds its own.
- Registration validates tool names (non-empty, ≤ 256 **bytes** of UTF-8 — not characters, so a non-ASCII name reaches the limit sooner — no null bytes)
  and updates an inverted capability index (string capabilities plus
  provider-derived virtual capabilities) used by `resolve_capabilities()`.
- TOML custom tools are still loaded only at daemon startup; changing
  `config/tools/*.toml` requires a restart.

### 4.3 Enable/Disable per Extension (the ENABLE axis)

**Decision:** One toggle per MCP server and per plugin.  Disabled means
**unloaded**: the MCP connection is closed or the plugin's child process is
stopped, its tools leave the registry, and nothing reconnects until it is
enabled again.  Built-ins are never on this axis.

**Where the bit lives:** `enabled` in the server's `[servers.<name>]` block
of `config/mcp.toml`; `enabled` in the plugins root's `.permissions.toml`.
The toggle writes the bit and then loads or unloads.  It is reachable from
`openalpaca ext enable|disable <kind> <id>`, the GUI (Settings →
Extensions), and `POST /v1/extensions/{kind}/{id}/{verb}`.

**What a disable does, in order:**
1. The toggle is written, and new calls to the extension's tools are
   refused from that moment.
2. The tools are removed from the shared registry.  Skills and agent
   templates that depended on them are scanned, and an
   `extension_capability_withdrawn` event names the ones that lost
   something.
3. In-flight calls are given `[extensions] drain_timeout_secs` (default 10)
   to finish.
4. The connection is closed or the child process is stopped.

**What callers see afterwards:**

| Surface | Behaviour |
|---------|-----------|
| Chat loop and lead agent tool lists | The extension's tools are left out. |
| A call that still reaches the registry (a run that copied it earlier) | Refused at the gate with a message naming the extension and saying what the model should do instead. |
| A skill whose requirement is wholly gone | Refused; dropped from auto-routing and from the skill list shown to the model; its cron fire is skipped. |
| A skill that lost one of two providers | Runs; the answer starts with a note saying what it ran without. |
| A subagent whose template names a lost capability | Runs without it; a warning is logged and an `extension_capability_withheld` event is published. |

An extension the ledger has never heard of is treated as enabled, so a tool
registered outside the supervisors keeps working.

### 4.4 Trusted-Context Identity Injection (Anti-Spoofing)

**Decision:** Owner-scoped tools take identity exclusively from the
`ToolContext` constructed by the sandbox caller.  `memory_search`
*overwrites* any LLM-supplied `owner_id`/`workspace_id` arguments with the
context values inside its `execute_with_context()` implementation, and
errors if the context carries no owner.  `update_persona` writes only to
the paths in its `PersonaToolContext`, configured at startup.
`artifact_write` chooses its store from `request_workspace_root` alone, so
a chat turn that carried no project lands its deliverable in the home store
rather than in whatever directory the daemon was started from.
`read_result` resolves only inside the calling session's own `results/`
directory.

**Rationale:**
- The LLM controls tool arguments — a model could fabricate an `owner_id`
  to access another user's memories.
- Injection happens inside the tool itself, at the last possible moment, so
  no dispatch path can bypass it.
- Non-BuiltIn backends (HTTP, Command, Plugin, MCP) never receive identity;
  the registry discards the context for them.

### 4.5 Parallel Tool Execution

**Decision:** All tool calls in a single LLM response execute concurrently
via `futures::join_all()`, capped at `max_tools_per_round`; calls beyond
the cap receive error results instead of executing.

**Rationale:**
- LLMs often emit multiple independent tool calls (e.g., `web_search` +
  `file_read`).  Sequential execution would be unnecessarily slow.
- The security sandbox is stateless per-call (circuit breaker state is
  shared but accessed atomically via `Mutex`), so parallel execution is safe.
- The task's cancellation token is raced against `join_all` for responsive
  cancellation.

### 4.6 Circuit Breaker Pattern

**Decision:** A per-(agent, tool) circuit breaker tracks consecutive
transient failures and temporarily disables tools that repeatedly fail.

**Rationale:**
- Prevents runaway API costs when an external tool endpoint is down.
- Three-state model: Closed → Open (blocking) → HalfOpen (probe) → Closed.
- Only *transient* errors (timeouts, 5xx, connection refused) trip the
  breaker.  Permanent errors (bad arguments, 404) do not.  A refusal because
  an extension is withheld never counts: it is a decision, not a failure.
- Trips emit `SystemEvent::CircuitBreakerTripped`.

**Scope:** breaker state lives in the `SandboxManager`.  Production code
builds one sandbox per chat turn, per skill invocation, per lead-agent run
and per subagent, so a breaker protects that unit of work; it does not
carry over to the next turn.

### 4.7 Confirmation Gate

**Decision:** Security-critical tools require interactive human
confirmation before execution, routed via `ConfirmationBroker`.

**How the confirmation set is chosen:** an explicit non-empty
`require_confirmation_for` list on the sandbox policy wins (an agent
template's `require_confirmation_for`, or a skill's
`permissions.confirm.tools`); otherwise the set is *derived from
annotations* — every registered tool with `destructive_hint = true`
(built-ins: `file_write`, `artifact_write`, `workspace_write`,
`update_persona`, `shell_execute`, `send`) requires confirmation.

**Behavior, in order:**
1. An **`ApprovalCache`** remembers approvals, scoped either to the exact
   argument hash (default) or to the whole tool; a cached approval skips the
   prompt.  The cache lives in the sandbox, so it lasts one turn, one skill
   invocation, one lead run or one subagent.
2. An **auto-approve bypass** (`security.auto_approve_confirmations`
   globally, or per-agent `auto_approve`) skips prompts entirely; each
   bypass is audit-logged to the `event_log` table.
3. An **unattended** run — the client said it cannot answer a prompt (a
   piped or redirected `openalpaca chat`, a scheduled skill) — is
   **refused at once**, in words the model can act on, instead of waiting
   out the timeout.  A workflow, and a follow-up the runner starts later,
   inherit the declaration of the turn that created them.  Nothing is
   approved.  The refusal is written to `event_log` as
   `tool_approval_unavailable`, and a workflow's completion report names
   what nobody could approve.
4. Otherwise a `ToolConfirmationRequested` event is published (carrying
   `stream_id`/`lane_key`/`task_id` so the right SSE stream, connector lane
   or run surfaces the prompt), and execution blocks until the user
   responds or the timeout expires (default **300 s**,
   `execution.agent_defaults.confirmation_timeout_secs`).
5. **Fail-closed:** if no broker is available the tool call is rejected.

**Every exit is a resolution.**  The broker owns the clock, and each way a
wait can end publishes `ToolConfirmationResolved` with an outcome of
`approved`, `denied`, `timed_out` or `cancelled`.  The tool runs only on
`approved`.  Clients settle their approval cards on that event.

**Answering a prompt:** the GUI's approval card, the CLI's inline `[y/N]`
at an interactive `openalpaca chat`, or
`openalpaca tasks confirmations list|watch|approve|deny`.  All of them call
`POST /v1/chat/confirmations/{request_id}`; `GET /v1/chat/confirmations`
lists the prompts still waiting.

### 4.8 Workspace Tools (Inter-Agent Collaboration)

**Decision:** `workspace_read` and `workspace_write` are registered
built-in tools providing key-value storage scoped to a task.

**Rationale:**
- Agents in a multi-agent task need a shared data plane.
- Optimistic locking prevents lost updates; on version conflict the write
  retries up to 5 times with jittered exponential backoff.
- Content is capped at 32 KB per entry to prevent context window overflow.
- Entries carry a type (`text`/`artifact`/`summary`/`context`) and an
  optional `file_asset_id` enabling file delivery to external channels
  (e.g. Telegram).
- An entry written with `entry_type = "artifact"` is also saved to the
  artifact store; the workspace entry keeps a 512-character preview and the
  artifact's id.

### 4.9 Artifacts and Large Results

**Decision:** Two built-ins keep large content out of the context window
without losing it.

- **`artifact_write`** saves a deliverable (document, plan, table, code,
  report) to the artifact store.  It is versioned — writing the same name
  again supersedes the previous version, which stays retrievable — listed
  in the GUI Library, and attached to the task that produced it.  Bodies
  are capped by `[execution.artifacts] max_artifact_bytes` (default 10 MB).
  It is the right tool for anything the user is meant to keep;
  `file_write` only drops bytes in the daemon's workspace.
- **Spill, don't truncate.**  When a tool result exceeds
  `[orchestrator.sessions] tool_result_inline_bytes` (default 32 KB) and
  the loop has a session log, the whole result is written to the session's
  `results/` directory.  The model is handed the first 2 KB plus a
  `result_ref=file:results/…` reference, and pages the rest back in with
  **`read_result`**.  A loop with no session log (a skill invocation, for
  one) cuts the result inline at the same threshold instead.  An error
  result is always shown as head **and** tail, so a compiler or test
  failure keeps the assertion at its end.

When a call runs inside a session, `file_write` takes a pre-edit snapshot
of any file it is about to overwrite (into the session's `snapshots/`), and
refuses the write when the snapshot cannot be kept — for instance when the
file is larger than `[orchestrator.sessions] snapshot_max_bytes`.  Outside a
session there is nowhere to keep one, and the write proceeds without it.

### 4.10 Protected Tool Names

**Decision:** Custom TOML tools cannot shadow built-in or runtime tool
names.  At startup the daemon collects the names of all registered
built-ins *dynamically*, adds the four lead-agent coordination tools
(`spawn_subagent`, `spawn_subagents_batch`, `check_subagent_status`,
`wait_for_subagents`), and skips (with a warning) any TOML tool whose name
collides.

**Rationale:**
- Prevents users from accidentally (or maliciously) replacing
  security-critical tools with arbitrary HTTP/command backends.
- Collecting names dynamically means new built-ins are protected
  automatically — there is no hardcoded list to keep in sync.
- Note the protection lives in the daemon's TOML-load path only; the
  registry's `register()` API itself permits overwriting (needed for
  runtime re-registration).  The other per-request names (`start_workflow`,
  `invoke_skill`, …) are not on the protected list; a per-request
  registration shadows a TOML tool of the same name for that request.

---

## 5. Security Model

### 5.1 Threat Model

| Threat | Mitigation |
|--------|------------|
| LLM invokes unauthorized tool | Sandbox allow/deny lists per policy, checked by tool name; an empty allow list denies everything |
| LLM crafts malicious arguments | InputSanitizer: path traversal, command injection, null bytes |
| LLM spoofs owner_id | Identity comes only from trusted `ToolContext`; owner-scoped tools overwrite LLM-supplied identity args |
| Tool endpoint returns hostile data | Result spill/cut (32 KB to the LLM by default; backend-level caps), no code execution on results |
| External tool endpoint is down | Circuit breaker prevents runaway retries/costs |
| Tool performs destructive action | Confirmation gate on `destructive_hint` tools (or explicit list), fail-closed, refused at once when nobody can answer |
| Owner switched an extension off | Extension gate refuses its tools even for a run that captured them earlier |
| Tool accesses internal network | SSRF validation blocks private IPs, cloud metadata, localhost |
| Tool runs indefinitely | Per-tool timeout via `tokio::time::timeout` (unless explicitly exempt; extension tools are never exempt) |
| LLM calls excessive tools | `max_tools_per_round` and `max_tool_calls` budget enforcement |

### 5.2 SSRF Protection

All HTTP tool backends (built-in `web_fetch` and custom HTTP tools from
TOML) pass through `url_validation::validate_url()`:

**Blocked targets:**
- Non-HTTP(S) schemes (`file://`, `ftp://`, etc.)
- Cloud metadata endpoints (`169.254.169.254`, `metadata.google.internal`)
- Localhost variants (`localhost`, `127.0.0.1`, `[::1]`, `0.0.0.0`)
- Private IP ranges: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`
- Carrier-grade NAT: `100.64.0.0/10`
- IPv6 private: `fc00::/7` (ULA), `fe80::/10` (link-local)
- IPv4-mapped IPv6 with private embedded addresses (`::ffff:10.0.0.1`)
- HTTP redirect chains are also SSRF-checked hop by hop (max 10 redirects)

### 5.3 Input Sanitization

The `InputSanitizer` provides three validation surfaces:

**User input sanitization:**
- Maximum length: 32 KB default (configurable via `security.max_input_length`)
- Null byte detection and rejection

**Tool argument sanitization** (recursive over all string values):
- Tool name must be in the registered-tool list
- Path traversal detection: `../` and `..\` are rejected in any string value
- Null byte detection in all string values
- Command injection detection — **only** for shell-like tools
  (`shell_execute` plus any command-backend tool from TOML):
  - Backtick execution (`` `cmd` ``)
  - Subshell execution (`$(cmd)`)
  - Newline and carriage-return injection (`\n`, `\r`)
  - Normal shell operators (pipes, redirections, `&&`) are intentionally
    allowed — the agent constructs full commands on purpose.  Non-shell
    tools (e.g. `file_write`) may legitimately contain multi-line content
    and are not injection-checked.

**File upload validation** (separate path, used by upload endpoints):
filename traversal and absolute-path rejection, size limit, polyglot
detection (declared MIME vs magic bytes), ZIP-bomb heuristic
(compression ratio > 100:1), and image dimension bounds.

### 5.4 Execution Flow (Security Path)

```text
Tool Call arrives
       │
       ▼
┌──────────────────┐    ┌───────────────┐
│ 1. Capability    │───►│ BLOCK if tool  │
│    Check         │    │ denied or not  │
│                  │    │ in allow-list  │
└──────┬───────────┘    └───────────────┘
       │ PASS
       ▼
┌──────────────────┐    ┌───────────────┐
│ 2. Input         │───►│ BLOCK if path  │
│    Sanitization  │    │ traversal or   │
│                  │    │ injection      │
└──────┬───────────┘    └───────────────┘
       │ PASS
       ▼
┌──────────────────┐    ┌───────────────┐
│ 3. Confirmation  │───►│ BLOCK if user  │
│  (destructive or │    │ denies, times  │
│   listed tools;  │    │ out (300s), is │
│   approval cache │    │ unattended, or │
│   / auto-approve │    │ no broker      │
│   may bypass)    │    │ (fail-closed)  │
└──────┬───────────┘    └───────────────┘
       │ PASS / SKIPPED
       ▼
┌──────────────────┐    ┌───────────────┐
│ 4. Circuit       │───►│ BLOCK if too   │
│    Breaker       │    │ many failures  │
└──────┬───────────┘    └───────────────┘
       │ PASS
       ▼
┌──────────────────┐    ┌───────────────┐
│ 5. Timeout-      │───►│ ABORT if       │
│    wrapped exec  │    │ exceeds limit  │
│ (skipped for     │    └───────────────┘
│  exempt tools)   │
└──────┬───────────┘
       │  inside the registry:
       ▼
┌──────────────────┐    ┌───────────────┐
│ 5a. Extension    │───►│ BLOCK if the   │
│     gate         │    │ MCP server or  │
│ (MCP + plugin    │    │ plugin is not  │
│  tools only)     │    │ enabled, or    │
└──────┬───────────┘    │ handle stale   │
       │ PASS           └───────────────┘
       ▼
┌──────────────────┐
│ 6. Record        │
│    Outcome       │
│ (circuit breaker,│
│  event emission) │
└──────────────────┘
```

---

## 6. Tool Annotations & Permission Tiers

Tools carry optional MCP-style annotations (`read_only_hint`,
`destructive_hint`, `idempotent_hint`, `open_world_hint`).  They come from
three places: `annotations_for_builtin()` for built-ins, the optional
`annotations` table in TOML tool configs, and the MCP server itself for
imported tools.  Plugin tools and the per-request tools carry none.

**Built-in annotation profiles:**

| Tools | Profile |
|-------|---------|
| `file_read`, `workspace_read`, `memory_search`, `read_result` | read-only, idempotent, closed-world |
| `web_fetch`, `web_search` | read-only, idempotent, open-world |
| `file_write`, `workspace_write`, `artifact_write`, `update_persona` | destructive, closed-world |
| `shell_execute`, `send` | destructive, open-world |

Annotations feed three mechanisms:

1. **Confirmation gating** — `destructive_hint = true` tools require user
   confirmation by default (§4.7).
2. **Virtual capabilities** — the eight `annotation:*` capabilities (§4.1).
3. **Permission tiers** — `permission_tier()` derives a coarse tier for
   introspection and policy: destructive → `Admin`, read-only →
   `ReadOnly`, otherwise `ReadWrite`.

---

## 7. Extensibility

### 7.1 Adding a Custom Tool (TOML)

Create a file in `config/tools/` (e.g., `config/tools/my_tools.toml`):

```toml
[[tools]]
name = "weather_lookup"
description = "Get current weather for a city or coordinates"
provides_capabilities = ["weather"]   # required for agents to resolve it

[tools.parameters]
type = "object"
required = ["location"]

[tools.parameters.properties.location]
type = "string"
description = "City name or lat,lon coordinates"

[tools.backend]
type = "http"
url = "https://api.weatherapi.com/v1/current.json?q={location}"
method = "GET"
timeout_secs = 10
```

Optional fields: `version` (default `"0.0.0"`), `author` (default
`"user"`), and an `annotations` table (feeds confirmation gating and
virtual capabilities).  Validation rules enforced at load time:

- HTTP URLs must start with `http://` or `https://`
- `timeout_secs` must be in `[1, 300]` (default 30)
- Any `annotation:*` entries in `provides_capabilities` must be one of the
  eight known annotation capability names
- Names colliding with protected tools are skipped (§4.10)

A custom tool with an empty `provides_capabilities` can never be resolved
to any agent — resolution matches only on capabilities.  Then declare the
capability on an agent template:

```yaml
# config/agents/research_agent.md (frontmatter)
---
id: research_agent
capabilities:
  - web_access
  - weather        # ← matches the tool's provides_capabilities
---
```

Custom tools are not on the ENABLE axis.  To remove one, delete its TOML
block and restart the daemon.

### 7.2 Adding a Built-in Tool (Rust)

1. Create a new file in `crates/openalpaca_core/src/tools/builtins/`.
2. Implement the `BuiltInTool` trait:
   ```rust
   #[async_trait]
   impl BuiltInTool for MyTool {
       async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
           // implementation
       }
       // Override execute_with_context() instead if the tool needs
       // identity (owner_id, task_id, workspace, session) from ToolContext.
   }
   ```
3. Create a factory function returning `RegisteredTool` — populate
   `definition`, `backend`, `provides_capabilities`, `exempt_from_timeout`
   (normally `false`), `annotations`, `version`, `author`, `created_at`.
4. Add an annotation profile for the new name in `annotations_for_builtin()`
   if the tool should be classified (read-only vs destructive drives
   confirmation gating and virtual capabilities).
5. Register it in `builtin_tools()` (or
   `builtin_tools_with_persona_context()` if it needs persona/connector
   wiring).  Name-collision protection for TOML tools picks it up
   automatically.
6. Grant its capability in the agent templates that should have it
   (`config/agents/*.md`).  A tool that belongs to one chat turn or one run
   is instead built per request and registered into that request's registry
   copy (`builtins/main_loop.rs` is the model).

### 7.3 MCP Servers

External MCP servers are declared in `config/mcp.toml`; a missing file
simply means no MCP servers.

```toml
[defaults]
connect_timeout_secs = 30      # per-server overridable
request_timeout_secs = 30
max_reconnect_attempts = 3
reconnect_backoff_ms = 100

[servers.fs]
transport = "stdio"            # or "http"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
# enabled = true               # the server's ENABLE bit; the toggle writes it
```

- A stdio server takes `env` (literal values) and `env_from` (the name of a
  daemon environment variable to read the value from — use this for
  secrets).  An HTTP server takes `url`, `auth` (bearer token or API-key
  header, with `*_env` variants), `extra_headers` and `extra_headers_from`.
  A named variable that is not set fails that server's start.
- Servers can also be declared and removed at runtime:
  `openalpaca ext mcp add …`, `openalpaca ext mcp remove <name>` (the
  server must be turned off first), or `POST /v1/extensions/mcp` and
  `DELETE /v1/extensions/mcp/{id}`.
  `openalpaca ext reload mcp <id>` re-applies an edited block or a rotated
  credential.
- Discovered tools register as **`<server>__<tool>`** (e.g.
  `fs__read_file`) with author `mcp:<server>`, so they cannot collide with
  built-ins or other servers.  Server-provided annotations are preserved
  and participate in confirmation gating and virtual capabilities.
- Enabled servers are brought up in parallel at boot.  Per-server failures
  (bad config, connect timeout, list failure) leave a `failed` row with a
  reason — never fatal to daemon startup.  A `config/mcp.toml` that does
  not parse is not fatal either.
- A server whose connection is lost for good is marked `failed`; its tools
  are refused with that reason until it is reloaded.
- Tool-call errors (`is_error` results) surface as tool errors to the LLM.
- **Limitations:** non-text MCP content (images, resources, audio) is
  replaced with a bracketed placeholder; MCP *resources* and *prompts* are
  not implemented — only tools are imported.  Server mode (exposing
  OpenAlpaca's tools over MCP) is a non-goal.

### 7.4 Plugins

Out-of-process plugins (child processes speaking JSON-RPC 2.0 over stdio)
can also contribute tools, registered as **`<plugin>::<tool>`** with a
`ToolBackend::Plugin` that proxies calls to the plugin process.  A plugin
must be **approved** before it first loads (consent), and is then switched
on and off like an MCP server.  Both are managed through
`openalpaca ext …` (with `openalpaca plugin …` as a plugin-shaped shortcut)
and the `/v1/extensions` routes.  See the *Extensions* section of
[Daemon_Manual.md](../Daemon_Manual.md) and the `ext` command in
[CLI_Manual.md](../CLI_Manual.md).

From the tool system's perspective a plugin tool is another registry entry
with author `plugin:<name>`.  Plugins can also contribute skills and agent
templates.  Plugin *connector* and *LLM-provider* bridges exist in code but
are not wired into the daemon — treat those plugin types as non-functional.

### 7.5 Skill-Bundled Scripts

Skills can bundle executable scripts in their `SKILL.md` frontmatter:

```yaml
scripts:
  - name: "analyze"
    file: "analyze.py"            # relative to <skill>/scripts/
    description: "Analyze the input data and report findings"   # required
    interpreter: "python3"        # optional — without it the file is
                                  # executed directly (exec bit + shebang)
    timeout_secs: 30              # optional — default 30
    parameters:                   # optional JSON Schema; default {}
      type: object
      properties:
        input:
          type: string
```

These become invocable as `skill_script:<name>` during that skill's
execution.  Arguments are passed as `--key=value` CLI flags; the script
runs with the skill directory as its working directory.  Path traversal
protection (canonicalize + prefix check) ensures scripts stay within
`<skill>/scripts/`, and stdout/stderr are capped at 512 KB.  The full
reference is [Skill_Template_Reference.md](../Skill_Template_Reference.md)
§4.9.

---

## 8. Execution Modes & Tool Availability

### 8.1 How Tools Reach Each Caller

| Caller | Tool surface | Allow list |
|--------|--------------|------------|
| **Chat main loop** | `start_workflow`, `task_status`, `memory_store`/`memory_forget`/`memory_search` (with a database), `read_result` (with a session log), every tool of every enabled extension, `invoke_skill`, keyword-suggested built-ins; plus `steer_workflow` and `queue_followup` while the lane has a running workflow and steering is on | Exactly the surface it was handed |
| **Lead agent** | `spawn_subagent`, `spawn_subagents_batch` (when `batch_spawn_enabled`, the default), `check_subagent_status`, `wait_for_subagents`, `post_update` and `queue_followup` (when steering is on), workspace tools, `memory_search`, `artifact_write` and `read_result` (when its template grants them), every tool of every enabled extension, `invoke_skill` | The template's capabilities plus the assembled surface; template denials still win |
| **Subagents** | `resolve_agent_tools()` — the template's capabilities resolved to tools, plus the workspace tools | The template's capabilities plus the workspace tools, plus the surface those capabilities resolved to — a template that grants `web_access` admits `web_search` and `web_fetch`. The widening adds that surface and nothing else, and template denials still win — see [TECHNICAL.md §11.2](./TECHNICAL.md#112-sandboxpolicy) |
| **Skills** | `requires_capabilities` or `tools.allow` resolved to tools, minus `tools.deny`, plus the skill's own `skill_script:*` and `invoke_skill:*` | Exactly the resolved surface |

`[orchestrator.routing] tool_selection = "full"` swaps the chat loop's
keyword-suggested picks for the whole shared registry; extension tools of a
disabled extension are dropped in both modes.

The lead agent is the only multi-agent dispatch mode.  Its coordination
tools are registered into a per-run copy of the registry
(`register_coordination_tools`), so they pass through the same
sandbox/registry path as every other tool.  `check_subagent_status` and
`wait_for_subagents` are flagged `exempt_from_timeout` — they manage their
own deadlines and must not be killed by the per-tool sandbox timeout.

### 8.2 Budget Enforcement

| Limit | Scope | Default | Configurable Via |
|-------|-------|---------|-----------------|
| `max_tools_per_round` | Per LLM round | 5 (subagent), 3 (lead), 3 (skill), 4 (chat) | `[execution.agent_defaults]`, `[execution.lead_agent_defaults]`, `[execution.skill_defaults]`, `[orchestrator.routing] main_loop_max_tools_per_round` |
| `max_rounds` | Per agentic loop | 15 (subagent), 18 (lead), 6 (skill), 8 (chat) | Same sections; `main_loop_max_rounds` for chat; per template `max_rounds` |
| `max_tool_calls` | Per agent run / skill invocation | Unset | Agent template frontmatter (`config/agents/*.md`); a skill's `tools.rate_limit.max_calls` |
| `max_tool_runtime_secs` | Per individual tool | 60 s (300 s for the lead) | `daemon.toml`; per template `timeout_seconds` |
| `confirmation_timeout_secs` | Per confirmation prompt | 300 s | `[execution.agent_defaults]` |
| `max_cost` | Per agent run / per chat turn | $1.00 ($5.00 for a lead-agent workflow) | `daemon.toml`; per template `max_cost_per_task` |
| `drain_timeout_secs` | In-flight calls at disable/reload | 10 s | `[extensions]` |

There is no daily budget for turns or workflows: cost is capped per turn
($1) and per workflow ($5).  Three background jobs do carry small daily
ceilings of their own under `[orchestrator.costs]` — memory extraction
(`extract_max_daily_cost_usd`), conversation summaries
(`summary_max_daily_cost_usd`) and task-output extraction
(`task_extract_max_daily_cost_usd`).

### 8.3 Output Limits

| Stage | Cap |
|-------|-----|
| Tool result fed back to the LLM | `tool_result_inline_bytes`, default 32 KB — spilled to `results/` with a 2 KB preview when the loop has a session log, otherwise cut with a notice |
| `read_result` page | 8 KB default, 64 KB maximum |
| HTTP backend response body | 1 MB streamed read, then first 8192 chars returned |
| Command backend stdout/stderr | 512 KB each |
| `shell_execute` stdout/stderr | 512 KB each |
| Skill script stdout/stderr | 512 KB each |
| Workspace entry content | 32 KB |
| Artifact body | `max_artifact_bytes`, default 10 MB |
| `file_read` / `file_write` | 10 MB |

---

## 9. Observability

### 9.1 Events

All of these are `SystemEvent`s on the `EventBus`, bridged to WebSocket
clients as `ServerEvent`s:

| Event | When |
|-------|------|
| `ToolExecuted` | Every tool execution: `agent_id`, `tool_name`, `success`, `duration_ms`, `task_id`, `session_id`, `tool_use_id` |
| `SecurityViolation` | A call the sandbox refused, with the reason |
| `CircuitBreakerTripped` | A breaker opened |
| `ToolConfirmationRequested` | A prompt was raised: request ID, tool name, arguments, `stream_id`/`lane_key`/`task_id` routing hints |
| `ToolConfirmationResolved` | A prompt ended: `approved`, `denied`, `timed_out` or `cancelled` |
| `ExtensionStateChanged` | An MCP server or plugin changed state |
| `ExtensionCapabilityWithheld` | A caller asked for a tool or capability an extension is withholding |
| `ExtensionCapabilityWithdrawn` | A disable, crash or reload took capabilities away; names the skills and templates affected |
| `ArtifactWritten` | `artifact_write` (or the workspace spill) saved a version |

### 9.2 Telemetry Storage

| Table | Retention | Content |
|-------|-----------|---------|
| `tool_execution_log` | 7 days | Per call: agent, tool, success, duration, error, plus run/session ids and argument/result previews |
| `skill_execution_log` | 90 days | Per skill invocation: status, rounds, tool calls, tokens, cost, model, route score |
| `event_log` | **Never pruned** | Every server event except heartbeats, plus the sandbox's `security_violation`, `tool_auto_approved` and `tool_approval_unavailable` rows |

Automated cleanup of the first two runs daily via
`spawn_telemetry_cleanup()`.  `event_log` rows go away only with a factory
reset or a project purge.

### 9.3 Inspection

- `GET /v1/tools` — the tool catalog with today's call counts (GUI:
  Settings → Tools).
- `GET /v1/extensions` — every MCP server and plugin with its state and
  reason (`openalpaca ext list`, GUI: Settings → Extensions).
- `GET /v1/skills`, `GET /v1/skills/health` — the skill catalog and
  per-skill metrics.
- `GET /v1/chat/confirmations` — approval prompts still waiting.

---

## 10. Future Considerations

| Area | Current State | Potential Enhancement |
|------|--------------|----------------------|
| **MCP resources & prompts** | Stubbed (tools only) | Import server resources/prompts into agent context |
| **Non-text MCP content** | Replaced with placeholder text | Surface images/resources to multimodal models |
| **Per-field authorization** | Tool-name granularity | JSON path-based argument restrictions |
| **Hot-reload of TOML tools** | Startup-only load | Watch `config/tools/` and re-register at runtime |
| **Result streaming** | Buffered, then spilled or cut | Streaming for long-running tools |
| **Sandboxed execution** | Process-level timeout | Container/WASM isolation for command tools |
| **`event_log` retention** | Grows without bound | A retention pass, if one is ever wanted |
| **Plugin connector / LLM-provider types** | Bridges exist, not wired | Wire into the connector manager and the LLM router |
