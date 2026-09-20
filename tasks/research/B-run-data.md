# Lens B — Run / Task Data Gaps

**Scope:** GAP-02 (steering endpoint) · GAP-03 (follow-up queue API) · GAP-06 (rerun/start) ·
GAP-07 (empty title/name on status events) · GAP-08 (cost, three sub-parts) ·
GAP-09 (subagent timeline) · GAP-10 (per-run event log) · GAP-23 (message→run/artifact links).

**Status:** research + plan only. No production code written.
**Branch:** `feat/ui-rework`. All line numbers are from the tree at the time of writing.

---

## 0. Ground truth — corrections to the brief

Four claims in `tasks/gui-api-requirements.md` / `API_MAP.md §3` do not survive contact with the
code. Two of them make the work *easier*; one makes GAP-09 *harder* than described.

### 0.1 `DagNodeStarted` / `DagNodeCompleted` are still produced. The brief says they are dead.

> "`DagNodeStatus` has the right shape but its producer was deleted in Routing V2 Phase 5 — do not
> build on it." — `tasks/gui-api-requirements.md`, Tier 3, GAP-09

That is false. The **lead-agent subagent spawn path is the producer today**:

- `crates/openalpaca_core/src/runner/lead_agent/tools.rs:232-240` — emits `SystemEvent::DagNodeStarted`
  with `node_id = Uuid::new_v4()`, `node_title = objective[..80]`, `agent_id = <template id>`.
- `crates/openalpaca_core/src/runner/lead_agent/tools.rs:614-628` — emits `DagNodeCompleted` on the
  LLM path (and `:529-543` on the plugin-backed path) with `agent_id = <instance id>`.
- `apps/openalpacad/src/event_bridge.rs:219-258` bridges both to `ServerEvent::DagNodeStatus`.
- `apps/openalpacad/src/events/persistence.rs:260-275` persists them to `event_log` as
  `event_type = "dag_node_status"`, `agent_id` column set.

The comment at `tools.rs:232` says so explicitly: *"Emit DagNodeStarted (reusing event, node_id = UUID)"*.

**Consequence for the plan:** the `spawn` rows the GUI's `run-events.ts` already renders
(`apps/openalpaca-gui/src/components/work/run-events.ts:63-71`, `case "dag_node_status"`) are real
and arriving today. But `DagNodeStatus` is still the wrong foundation for the swimlanes, for three
reasons that have nothing to do with it being dead:

1. **No persistence of the span.** `node_id` is a fresh UUID per spawn held only in the tokio task's
   closure. Nothing writes it to a table, so a client that reloads or connects late sees nothing.
2. **`agent_id` means two different things across the pair.** `DagNodeStarted` passes the *template*
   id (`tools.rs:236`, `agent_id: agent_id.to_string()` where `agent_id` is the template);
   `DagNodeCompleted` passes the *instance* id (`tools.rs:618`, `agent_id: instance_id.clone()`).
   A client cannot join start to completion on `agent_id`; only on `node_id`, which is not persisted.
3. **No lead lane, no blocked state, no progress.** The design's `lead` lane and its
   `awaiting you` / `12 files read` detail strings have no source at all.

So: **keep `DagNodeStatus` (it is live and the UI consumes it), and add the span resource beside it.**
Do not resurrect a DAG executor — none is needed; the spawn site already exists.

### 0.2 `agent_task_history` is not merely missing `started_at` — there is no row until completion.

`crates/openalpaca_core/src/orchestrator/dispatcher/usage.rs:76-101` (`record_agent_history`) is
called from exactly two places, both **after** the subagent's loop returns:
`runner/lead_agent/tools.rs:548-556` (plugin path) and `:642-650` (LLM path). The table
(`crates/openalpaca_storage/src/migrations/007_subagents.sql:27-37`) has
`id, agent_id, task_id, role, status, runtime_seconds, completed_at` and no start column.

Therefore an in-flight span is not "underivable" — it is **absent**. A `started_at` column alone
fixes nothing; the write must move to spawn time and be updated at completion.

Two further traps in that table:

- `agent_id TEXT NOT NULL REFERENCES agent(id) ON DELETE CASCADE`, and
  `PRAGMA foreign_keys = ON` (`crates/openalpaca_storage/src/database/mod.rs:62`). The value written
  is the *template* id, which must already exist as an `agent` row. A new span table must not repeat
  this coupling.
- `add_history` writes `completed_at` as `"%Y-%m-%d %H:%M:%S"` (`repository/subagent/mod.rs:232`) —
  no timezone. `run-model.ts:parseTimestamp` compensates by appending `Z`
  (`apps/openalpaca-gui/src/components/work/run-model.ts:38-47`), but new columns should be RFC 3339.

### 0.3 GAP-10 is cheaper than described: `task_id` is already at the emit site.

`SandboxManager::execute_tool` takes `ctx: &ToolContext`
(`crates/openalpaca_core/src/security/sandbox/mod.rs:134-139`), and `ToolContext` carries
`task_id: Option<String>` — set for every subagent (`runner/lead_agent/tools.rs:250-266`) and for the
lead agent. The three emitters that the API_MAP says "carry an `agent_id` and no `task_id`" all sit
*inside* that function or its helpers:

- `emit_security_violation` — `sandbox/mod.rs:352-376`
- `emit_tool_executed` — `sandbox/mod.rs:378-386`
- `ToolConfirmationRequested` — `sandbox/mod.rs:227-236`

Adding `task_id` to those three events is a parameter passthrough, not a plumbing project.

And the *workflow* half of the log is already being written with the task id — into the `detail`
JSON blob rather than a column. `events/persistence.rs` logs `task_status` (`:59-80`),
`dag_node_status` (`:260-275`), `workflow_started` / `workflow_steered` / `workflow_progress`
(`:308-341`), every one of them with `"task_id"` inside `detail` and `None` for the `agent_id`
column. So GAP-10 is mostly **a column and an index over rows that already exist**, not a new
capture path.

### 0.4 GAP-23 needs no new table, and `delegation` is in scope at the persist site.

- `crates/openalpaca_core/src/gateway/router/mod.rs:255-285`: `persist_assistant_message` is called
  at `:260`, and `result.delegation` is read at `:285` — **eleven lines apart, same match arm.**
  Persistence is genuinely the only missing piece for `task_id`.
- `conversation_message_attachments` already has a
  `role TEXT NOT NULL DEFAULT 'attachment'` column (`migrations/028_message_attachments.sql:7`).
  Outputs can be linked with `role = 'artifact'`; no new table is required for `artifact_ids`.

---

## 1. Assumptions about Lens A (state them, do not build on guesses)

I assume Lens A delivers, in **migration 035**:

- An `artifacts` table with at minimum `id TEXT PRIMARY KEY`, `task_id TEXT NULL REFERENCES task(id)`,
  `agent_instance_id TEXT NULL`, `name`, `kind`, `mime_type`, `size_bytes`, `path TEXT NOT NULL`,
  `created_at`, `updated_at` — where `path` is a **file address inside the project's `.openalpaca/`
  directory**, per the architectural directive: the DB stores the address, never the bytes.
- A path convention that makes a run's outputs findable by a human. **Request to Lens A:** make it
  run-addressable, e.g. `<project>/.openalpaca/runs/<task_id>/artifacts/<name>`, so
  "open this run's files in Finder" is one path join and not a query. Lens B's per-run surfaces
  (run detail Files section, the `RunReportCard` chips) become a `WHERE task_id = ?` on that table.
- `GET /v1/artifacts?task_id=` exists, returning the `Artifact` shape already typed at
  `apps/openalpaca-gui/src/lib/api/unbacked.ts:39-56`.

Everything below that touches artifacts is written against `artifacts.id` and `artifacts.task_id`.
**If Lens A names them differently, only §8 (GAP-23) and the `artifact` tag in §7 (GAP-10) change.**

**Migration numbering (needs the synthesizer to arbitrate):** current head is **034**
(`crates/openalpaca_storage/src/migrations/034_drop_context_compaction_log.sql`). I claim:

| # | File | Owner | Contents |
|---|---|---|---|
| 035 | `035_artifacts.sql` | Lens A | artifacts table |
| 036 | `036_artifact_versions.sql` | Lens A | version history |
| **037** | **`037_run_observability.sql`** | **Lens B** | `subagent_span` table; `event_log.task_id`; `task.source_task_id`; (`task.workspace_id`, pending Q2) |
| **038** | **`038_message_run_links.sql`** | **Lens B** | `conversation_messages.task_id` |

037 and 038 could be merged into one file; they are split because 038 is independently useful and
038's write path (Gateway) is unrelated to 037's (dispatcher/sandbox).

---

## 2. GAP-07 — empty `title` / `name` on status events

**Fix size: XS. Do this first; GAP-09's live events depend on the same bridge.**

### Current state

`apps/openalpacad/src/event_bridge.rs`:

| Line | Arm | What it passes |
|---|---|---|
| `:34-41` | `TaskCreated` | real `title` ✅ |
| `:42-60` | `TaskUpdated` | `""` |
| `:61-73` | `TaskCompleted` | `""` |
| `:74-81` | `TaskFailed` | `""` |
| `:82-99` | `AgentStatusChanged` | `""` for `name` |

`ServerEvent::TaskStatus` *has* a `title: String` field and `AgentStatus` has `name: String`
(`crates/openalpaca_api/src/events/mod.rs`), and `EventBroadcaster::task_status` /
`agent_status` (`apps/openalpacad/src/events/handlers.rs:10-36`, `:41-62`) faithfully forward
whatever they are given.

### Rejected approach: pass `SharedContext` into the bridge

The requirements doc suggests resolving from `task_registry`. That would work
(`TaskRegistry::get` → `TaskEntry { title, .. }`, `context/shared/mod.rs:39-45, 103`) but the bridge
is spawned at `apps/openalpacad/src/main.rs:245-250`, **before** `SharedContext` exists — it is
created inside `services::initialize_services` at `main.rs:306`
(`apps/openalpacad/src/services/mod.rs:56`). Moving the bridge spawn later risks dropping events
published during service init, for no benefit.

### Chosen approach: carry the value on the `SystemEvent`

Add `title: String` to the three task variants and `name: String` to `AgentStatusChanged` in
`crates/openalpaca_core/src/events.rs`. There are only five non-test producer sites, and **every one
already has the value or a `SharedContext` in scope**:

| Producer | Line | Source of the title |
|---|---|---|
| `apply_task_action` | `orchestrator/task_ops.rs:153-159` | `shared_context.task_registry.get(task_id)`; the DB-fallback branch at `:73-79` already loaded the `Task` |
| `spawn_lead_agent_execution` | `orchestrator/dispatcher/lead_agent.rs:237-243` | `task_title` is a parameter (`:177`) |
| `finalize_task` (completed) | `orchestrator/dispatcher/outcome.rs:268-275` | `ctx: &SharedContext` param at `:240` |
| `finalize_task` (failed) | `orchestrator/dispatcher/outcome.rs:294-299` | same |
| `require_router` | `orchestrator/dispatcher/mod.rs:171-176` | `self.shared_context` |

For `AgentStatusChanged`, the four producers
(`runner/lead_agent/tools.rs:217-224`, `runner/lead_agent/guard.rs:50`,
`dispatcher/lead_agent.rs:80-87` and `:403`, `routes/agents.rs:206`) all hold a `SubAgent` or the
`agent_registry`; `AgentRegistry::get_instance` (`agent/registry/mod.rs:218`) returns
`SubAgent { name, .. }`.

Helper to add next to `finalize_task`:

```rust
// crates/openalpaca_core/src/orchestrator/dispatcher/outcome.rs (or context/shared)
pub(crate) fn title_for(ctx: &SharedContext, task_id: &str) -> String {
    ctx.task_registry.get(task_id).map(|e| e.title).unwrap_or_default()
}
```

**Known limit, worth a doc comment:** after a daemon restart the registry has no entry for a DB-only
task, so the title falls back to `""` — exactly today's behaviour, never worse. The client already
tolerates `""` (`run-events.ts:29`, `event.title === "" ? "workflow started" : …`).

### Wire shape

No shape change. `ServerEvent::TaskStatus.title` and `AgentStatus.name` simply stop being empty.

### Test

Extend the existing bridge test at `apps/openalpacad/src/event_bridge.rs:540-560` with a
`TaskUpdated` case asserting a non-empty title.

---

## 3. GAP-02 — `POST /v1/tasks/{id}/steer`

**Fix size: S. Pure reuse — the inbox, the push helper and the event all exist.**

### Current state

- The only entry point is the literal prefix: `orchestrator/handlers.rs:161-177` strips `"/steer "`
  and calls `handle_steer_prefix`.
- `orchestrator/task_ops.rs:360-430` resolves `shared_context.workflows_for_lane(lane_key)` and
  refuses when the lane has 0 or ≥2 workflows (`:377-378`, `:411-428`) — it takes **no `task_id`**.
- The real primitive is already correct and task-addressed:
  `crates/openalpaca_core/src/runner/steering.rs:60-77`
  ```rust
  pub fn push_steering(
      shared_context: &SharedContext, bus: &EventBus,
      task_id: &str, lane_key: &str, msg: SteeringMsg,
  ) -> Result<usize, SteeringPushError>   // Ok(depth) | Err(Full) | Err(Closed)
  ```
  It emits `SystemEvent::WorkflowSteered` on success (`steering.rs:71-76`).
- The inbox is registered per running lead-agent task, gated on `steering_enabled`
  (`dispatcher/lead_agent.rs:210-222`), and closed + drained at exit (`:308-315`).
- `grep -rn steer apps/openalpacad/src/router.rs` → nothing.

### Route

```
POST /v1/tasks/{id}/steer
Body:  { "message": "Also check the MCP resource stubs." }

200  { "task_id": "b41c…", "accepted": true, "inbox_depth": 2, "lane_key": "junpei:gui" }
400  { "error": "message must be 1-8000 characters" }
404  { "error": "Task not found" }
409  { "error": "Steering queue is full", "code": "STEERING_INBOX_FULL", "cap": 16 }
409  { "error": "Task is not a running workflow",  "code": "TASK_NOT_STEERABLE" }
503  { "error": "Steering is disabled", "code": "STEERING_DISABLED" }
```

Error envelope: **`{"error": "..."}` plus an optional `code`** — matches the sibling
`task_action_handler` (`routes/tasks.rs:237-290`), not the `{error:{code,message}}` style used by
settings/chat. Do not mix the two on the same resource.

### Rust

New handler in `apps/openalpacad/src/routes/tasks.rs`, request type in `tasks_types.rs`:

```rust
#[derive(Debug, Deserialize)]
pub struct SteerTaskRequest { pub message: String }

pub async fn steer_task_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SteerTaskRequest>,
) -> impl IntoResponse
```

Body:

1. Trim + length-check `message` (1..=8000), 400 otherwise.
2. `if !state.daemon_config.load().orchestrator.routing.steering_enabled { 503 }`.
3. Resolve the lane. `push_steering` needs a `lane_key` *only* for the event payload, so read it from
   the persisted task: `TaskRepository::new(&state.db).get(&id)?` → `task.source_lane`; 404 when
   absent. (Do **not** require the caller to supply a lane — the GUI addresses a run, not a lane.)
4. Build the `SteeringMsg`:
   ```rust
   SteeringMsg {
       text, request_id: Uuid::new_v4(),
       principal: Principal::User { global_id: state.local_user_id.clone() },
       scope: Scope::Global,
       workspace_path: None,        // see note
       received_at: Utc::now(),
   }
   ```
   `AppState.local_user_id` exists (`apps/openalpacad/src/state.rs:32`). The principal/scope matter
   because leftovers are converted to `unprocessed_steering` follow-ups at workflow exit
   (`dispatcher/lead_agent.rs:316-350`) and re-enter through the front door.
   **Uncertainty:** `workspace_path` — the GUI knows its project dir but does not send one on this
   route today. I would accept an optional `workspace_path` in the body and thread it through, so a
   converted leftover re-enters scoped to the same project. Flagging rather than guessing.
5. `match push_steering(&state.gateway.shared_context, &state.gateway.bus, &id, &lane_key, msg)`
   → `Ok(depth)` 200; `Err(Full)` 409 with `cap` from
   `daemon_config.orchestrator.routing.steering_inbox_cap`; `Err(Closed)` 409 `TASK_NOT_STEERABLE`.

Registration, beside the existing task routes (`apps/openalpacad/src/router.rs:58-64`):

```rust
.route("/v1/tasks/{id}/steer", post(crate::routes::steer_task_handler))
```

### Events

None new. `WorkflowSteered` already fires from `push_steering` and already bridges
(`event_bridge.rs` → `ServerEvent::WorkflowSteered`), and the GUI already renders it as a `steer`
row (`run-events.ts:41-49`).

### Client change

`apps/openalpaca-gui/src/lib/api/unbacked.ts:215-218` (`steerWorkflow`) drops its `/steer …` chat
workaround and becomes a real `POST`. `run-actions.ts:69-77` drops `STEER`'s gap tooltip.

### Leave the prefix path alone

`"/steer "` through `/v1/chat` stays — it is the CLI's and Telegram's only channel.

---

## 4. GAP-03 — follow-up queue routes

**Fix size: M, almost all of it route-shaped. Storage, the claim protocol, the runner and the event
all exist; only HTTP is missing.**

### Current state

- `migrations/033_lane_followups.sql` — table with
  `id, lane_key, kind CHECK(followup|unprocessed_steering), content, principal_json, workspace_path, source_task_id, status CHECK(queued|running|done|cancelled), created_at, updated_at`
  and `idx_lane_followups_lane_status(lane_key, status, id)`.
- `crates/openalpaca_storage/src/repository/followup/mod.rs` — `queue` (`:65-89`), `get` (`:92-105`),
  `list_queued_by_lane` (`:108-120`), `claim_next` (`:126-165`, CAS `queued → running`, kind-filtered
  to `followup` so `unprocessed_steering` never auto-runs), `mark_done` (`:168`),
  `mark_cancelled` (`:172`).
- Writers today: `QueueFollowupTool` (`runner/lead_agent/tools.rs:1146-1189`) and the
  steering-leftover conversion (`dispatcher/lead_agent.rs:316-350`).
- Auto-start at workflow finalize: `dispatcher/lead_agent.rs:514-557`.
- `SystemEvent::FollowupQueued` (`events.rs:392-398`) → `ServerEvent::FollowupQueued`
  (`openalpaca_api/src/events/mod.rs:242-249`).
- Zero routes.

### Routes

New file `apps/openalpacad/src/routes/followups.rs`.

```
GET    /v1/lanes/{lane_key}/followups?status=queued&kind=followup&limit=50
200 { "followups": [FollowupView], "total": 3 }

POST   /v1/lanes/{lane_key}/followups
Body:  { "content": "…", "source_task_id": "b41c…" }        // source_task_id optional
201 { "followup_id": 42, "lane_key": "junpei:gui", "kind": "followup", "status": "queued" }

DELETE /v1/lanes/{lane_key}/followups/{followup_id}
200 { "followup_id": 42, "status": "cancelled" }
409 { "error": "Cannot cancel a followup in 'running' state" }
404 { "error": "Follow-up not found" }
```

`{lane_key}` contains a `:` (`"junpei:gui"`). Axum path segments accept it unencoded, but the client
must `encodeURIComponent` it anyway — **note this in the client adapter**, because a lane key with a
`/` would break the route. Lane keys are `"{user}:{source}"` (`gateway/router/mod.rs`
`derive_user_and_source`), so `/` does not occur in practice; still worth a validation guard that
rejects `lane_key.contains('/')` with 400.

### `FollowupView` — never serialize `principal_json`

`FollowupRecord` derives `Serialize` (`repository/followup/mod.rs:19-34`) **including
`principal_json`**, which is an identity blob. Do not return the record directly. The client's typed
shape (`apps/openalpaca-gui/src/lib/api/unbacked.ts:225-235`) already omits it, and it omits
`workspace_path` too:

```rust
// apps/openalpacad/src/routes/followups_types.rs
#[derive(Debug, Serialize)]
pub struct FollowupView {
    pub id: i64,
    pub lane_key: String,
    pub kind: String,            // "followup" | "unprocessed_steering"
    pub content: String,
    pub source_task_id: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}
impl From<FollowupRecord> for FollowupView { /* drops principal_json + workspace_path */ }
```

### Repository additions (`crates/openalpaca_storage/src/repository/followup/mod.rs`)

`list_queued_by_lane` is hardcoded to `status = 'queued'` and does not filter by kind, so the GET
above cannot be served by it. Add:

```rust
/// List a lane's follow-ups, newest last, optionally filtered.
pub fn list_by_lane(
    &self,
    lane_key: &str,
    status: Option<&str>,
    kind: Option<&str>,
    limit: usize,
) -> Result<Vec<FollowupRecord>>;

/// Cancel only if still queued (CAS, mirroring `claim_next`).
/// Ok(true) = cancelled, Ok(false) = row was not in 'queued'.
pub fn cancel_if_queued(&self, id: i64) -> Result<bool>;
```

`cancel_if_queued` matters: `mark_cancelled` (`:172`) is an unconditional `UPDATE`, so a naive
DELETE route would cancel a follow-up the runner has already claimed and spawned
(`dispatcher/lead_agent.rs:527-542`) — the turn would still run while the row read "cancelled".
CAS on `status = 'queued'` closes that race the same way `claim_next` does.

### POST handler body

```rust
pub async fn queue_followup_handler(
    State(state): State<Arc<AppState>>,
    Path(lane_key): Path<String>,
    Json(req): Json<QueueFollowupRequest>,
) -> impl IntoResponse
```

1. Validate `content` (1..=10_000) and `lane_key` (non-empty, no `/`).
2. `principal_json = serde_json::to_string(&Principal::User { global_id: state.local_user_id.clone() })?`
   — the same identity the daemon's own HTTP callers use. Kind is always `"followup"`; the API must
   **not** let a client mint `unprocessed_steering` rows (the CHECK constraint would accept it, but
   `claim_next` deliberately never claims that kind — a client-created one would sit forever).
3. `FollowupRepository::queue(&lane_key, "followup", &content, &principal_json, None, source_task_id)`.
4. Publish `SystemEvent::FollowupQueued { lane_key, followup_id, kind: "followup", timestamp }` on
   `state.gateway.bus` — identical to `tools.rs:1180-1185`, so the GUI's existing
   `followup_queued` badge lights up for HTTP-queued items too.

### Should DELETE emit an event?

Yes — add `SystemEvent::FollowupCancelled { lane_key, followup_id, timestamp }` and the matching
`ServerEvent::FollowupCancelled { lane_key, followup_id, ts, instance_id }`, so a second GUI window
drops the row without polling. Small, symmetric with `FollowupQueued`, and the bridge arm is four
lines. (Alternatively skip it and let the client refetch — but the queue list is exactly the surface
where a stale row misleads.)

### Registration

```rust
.route("/v1/lanes/{lane_key}/followups",
       get(crate::routes::list_followups_handler).post(crate::routes::queue_followup_handler))
.route("/v1/lanes/{lane_key}/followups/{followup_id}",
       delete(crate::routes::cancel_followup_handler))
```

---

## 5. GAP-06 — `rerun` and `start` verbs

**Fix size: S for `start`, S–M for `rerun` (needs one new public entry point on the dispatcher).**

### Current state

- `apply_task_action` (`orchestrator/task_ops.rs:61-159`) matches exactly
  `"cancel" | "pause" | "resume"` at `:82-107`, everything else → `TaskActionError::UnknownAction`.
  The HTTP error text is generated at `routes/tasks.rs:283-287`.
- `POST /v1/tasks` (`routes/tasks.rs:53-125`) persists the row, registers it in `task_registry`,
  creates a task lane, publishes `TaskCreated` — **and returns.** No dispatcher call. Confirmed by
  reading the whole handler.
- The only real dispatch entry is `TaskDispatcher::dispatch_lead_agent`
  (`orchestrator/dispatcher/lead_agent.rs:23-166`), which is `pub(crate)` and is reached from
  exactly one place: the model's `StartWorkflowTool` (`tools/builtins/start_workflow.rs:119-127`).
- `AppState` (`apps/openalpacad/src/state.rs:17-40`) holds `gateway: Arc<Gateway>` but **no
  orchestrator and no dispatcher**. `Gateway` (`gateway/router/mod.rs:149-155`) holds only
  `shared_context`, `lane_manager`, `handler: Arc<dyn MessageHandler>`, `bus`, `persistence`.

### What a correct `rerun` does

`dispatch_lead_agent` creates a **new** task id, spawns a lead-agent instance, registers the task,
creates a lane, persists a `Queued` row, initialises `state_json`, and spawns the background
execution which immediately flips it to `Running` (`lead_agent.rs:236-246`). That is precisely
"re-run", provided we feed it the original task's `description`, `title`, `created_by` and
`source_lane`.

So `rerun` is: **read the old row, call `dispatch_lead_agent` with its fields, record the lineage.**
It is *not* a mutation of the old task — matching the API_MAP's deliberate 201-with-new-id asymmetry.

Three pieces of wiring:

**(a) Make the dispatcher reachable.** Two options:

- *Preferred:* add `pub fn task_dispatcher(&self) -> Arc<TaskDispatcher>` to `Orchestrator`
  (field at `orchestrator/mod.rs:155`), and add `pub orchestrator: Arc<Orchestrator>` to `AppState`.
  The orchestrator is already constructed at `apps/openalpacad/src/main.rs:365` and `AppState` is
  built after it (`main.rs:438`), so this is an added field, not a reordering.
- *Alternative:* add `pub fn rerun_task(&self, task_id: &str) -> Result<DispatchOutcome, RerunError>`
  on `Orchestrator` and keep the dispatcher private. Cleaner encapsulation; slightly more code.
  I lean to this one — it keeps the "read the old row" logic in core where the dispatcher lives, and
  the daemon route stays a thin translation to HTTP.

```rust
// crates/openalpaca_core/src/orchestrator/task_ops.rs (new)
pub enum RerunError { NotFound, NotTerminal { current: &'static str }, NoDescription, Db(String), Dispatch(String) }

impl Orchestrator {
    /// Clone a terminal task's request into a fresh lead-agent workflow.
    pub fn rerun_task(&self, task_id: &str) -> Result<DispatchOutcome, RerunError>;
}
```

Behaviour: 404 if no DB row; 409 unless `status ∈ {Completed, Failed, Cancelled}`
(`TaskEntryStatus::is_terminal`, `context/shared/mod.rs:32`, as used at `task_ops.rs:83`); 422 if `description` is `NULL`
(a `POST /v1/tasks`-created row has no goal text to re-run — the dispatcher needs
`description` as the objective). Then
`dispatch_lead_agent(&description, format!("{title}"), &created_by, &source_lane, "gui", None)`.

**Uncertainty — `workspace_id`:** `dispatch_lead_agent`'s last parameter is
`workspace_id: Option<String>` and it is threaded into every subagent's `ToolContext`
(`runner/lead_agent/tools.rs:255`). The original task's row does **not** store it — `task` has no
workspace column. A rerun therefore loses workspace scoping unless we either (i) accept an optional
`workspace_path` on the rerun request body and derive the id the way `handlers.rs` does, or
(ii) add a `workspace_id` column to `task` in migration 037. **I recommend (ii)** — it is one column,
it makes reruns faithful, and Lens A's artifact paths want a per-task project dir anyway. Flagging
it as a decision rather than assuming.

**(b) Record the lineage.** The client's `RerunResult` wants `source_task_id`
(`apps/openalpaca-gui/src/lib/api/unbacked.ts:181-185`). Echoing it in the response is enough for the
immediate UI, but the Work list wants to show "re-run of …" after a reload. Add to migration 037:

```sql
ALTER TABLE task ADD COLUMN source_task_id TEXT;   -- set only by rerun
CREATE INDEX IF NOT EXISTS idx_task_source ON task(source_task_id);
```

and set it on the new row. This requires `dispatch_lead_agent` to accept it — rather than widen an
already 6-argument function, add a small options struct or a follow-up
`TaskRepository::set_source_task(&new_id, &old_id)` call from `rerun_task` right after dispatch.
The latter is the smaller change.

**(c) `start`.** This one is genuinely a new arm in `apply_task_action` — *except* that
`apply_task_action` is a pure state-transition function with no dispatcher access
(`task_ops.rs:61-68` takes `shared_context, lane_manager, bus, db, task_id, action`). Promoting a
queued task means *dispatching* it, not relabelling it.

So `start` must not go through `apply_task_action`. Handle it in the route, next to `rerun`:
read the row; 409 unless `status == Queued`; then dispatch it. Two sub-cases:

- A row created by `POST /v1/tasks` was never dispatched → dispatch it now via the same
  `dispatch_lead_agent` path. **Caveat: that creates a *new* task id**, because
  `dispatch_lead_agent` mints one at `lead_agent.rs:32`. The API_MAP promises `start` mutates in
  place (200, same id). To honour that, `dispatch_lead_agent` needs an id-injection variant:

  ```rust
  pub(crate) fn dispatch_lead_agent_with_id(
      &self, task_id: String, description: &str, title: String,
      created_by: &str, lane_key: &str, source: &str, workspace_id: Option<String>,
  ) -> Result<DispatchOutcome, String>;
  ```
  with the existing `dispatch_lead_agent` becoming a one-line wrapper that passes
  `Uuid::new_v4().to_string()`. The persist step at `lead_agent.rs:97-136` then needs to
  `update` rather than `create` when the row already exists — `TaskRepository::create` would fail on
  the PK. Simplest: attempt `create`, and on a uniqueness error fall through to `update_status(Queued)`
  + `update_state`. **This is the least clean part of the plan; call it out in review.**
- A row created by `dispatch_lead_agent` is only `Queued` for the microseconds before the spawned
  task sets `Running` (`lead_agent.rs:236-246`). `start` on such a row is effectively a no-op race.
  Guard it: if `shared_context.cancellation_tokens` has an entry for the id, the task is already
  live → 409 `"Task is already dispatched"`.

### Route

```
POST /v1/tasks/{id}/action
{ "action": "rerun" }
201 { "task_id": "<new uuid>", "status": "queued", "source_task_id": "b41c…" }
409 { "error": "Can only re-run a finished task, current state: 'running'" }
422 { "error": "Task has no description to re-run" }

{ "action": "start" }
200 { "task_id": "3ac55f19", "status": "running" }
409 { "error": "Can only start a queued task, current state: 'running'" }
```

Same path and body as today (`TaskActionRequest { action: String }`,
`routes/tasks_types.rs:25-28`) — the handler branches on the verb before falling through to
`apply_task_action` for the three existing ones. Update the `UnknownAction` message at
`routes/tasks.rs:287` to list all five.

`TaskActionError::UnknownAction`'s wording lives in the daemon, not core, so core needs no change for
the message — but if `rerun`/`start` were ever routed through `apply_task_action`, its match at
`task_ops.rs:107` would swallow them. They must be intercepted **before** the `apply_task_action`
call at `routes/tasks.rs:245`.

---

## 6. GAP-08 — cost, three independent fixes

### 8a — `daily_cost_usd` hardcoded

`apps/openalpacad/src/routes/settings.rs:314`:

```rust
let daily_cost_usd = 0.0; // Cost tracker requires async access via LlmRouter
```

The comment is stale on both counts: `get_orchestrator_config` is already `async`
(`settings.rs:291`), and `service.router().cost_tracker` is awaited three functions later at
`settings.rs:488` (`get_provider_usage`).

**Fix:**

```rust
let daily_cost_usd = service.router().cost_tracker.total_cost().await;
```

`CostTracker::total_cost` (`crates/openalpaca_llm/src/routing/cost_tracker/mod.rs:237-245`) sums
`agent_usage`, which is seeded at boot from *today's* `llm_usage_daily` rows via
`restore_cost_tracker` (`apps/openalpacad/src/services/llm.rs:201-260`, called at `main.rs:326` with
`cost_tracker_date` = today at `main.rs:328`).

**Caveat to document:** the in-memory tracker is not reset at local midnight, so on a daemon that has
been up across a date boundary `total_cost()` is "since boot", not "today". The GUI's
`useTodaySpend` (`apps/openalpaca-gui/src/hooks/useUsage.ts:33-42`) already sums
`GET /v1/llm/usage/daily?date=` client-side and is *more* correct. Two honest options:

- (i) Serve `daily_cost_usd` from the DB instead — `LlmUsageRepository::query_daily_usage(None, Some(today), …)`
  summed — which is date-correct and needs no `await` on the tracker at all.
- (ii) Serve the tracker and rename the field. Renaming breaks the client type
  (`apps/openalpaca-gui/src/lib/api/types.ts:414`).

**Recommend (i).** It matches what the UI already computes, it is correct across midnight, and it
removes the "why are these two numbers different" bug before it exists. §8c's rollup then becomes the
single server-side source and the client's manual sum goes away.

### 8b — `task_id` on `GET /v1/llm/usage`

`LlmUsageRepository::get_task_usage(task_id, limit)` exists
(`crates/openalpaca_storage/src/repository/llm_usage/mod.rs:95-110`) and `llm_call_log` has an
indexed `task_id` column (`migrations/008_llm_usage.sql:8,22`). `LlmUsageQuery`
(`apps/openalpacad/src/routes/settings_types.rs:19-24`) has `{ agent_id, key_id, limit }`, and
`get_llm_usage` (`settings.rs:367-391`) never calls `get_task_usage`.

**Fix — two lines:**

```rust
// settings_types.rs
pub struct LlmUsageQuery {
    pub agent_id: Option<String>,
    pub key_id: Option<String>,
    pub task_id: Option<String>,     // ← new
    pub limit: Option<usize>,
}

// settings.rs get_llm_usage — precedence: task_id first (most specific)
let result = if let Some(ref task_id) = query.task_id {
    repo.get_task_usage(task_id, limit)
} else if let Some(ref agent_id) = query.agent_id { … };
```

The GUI already sends the param and documents that it is currently ignored
(`apps/openalpaca-gui/src/lib/api/usage.ts:8-16, 28`) — it starts working with zero client change.

**Do also add the per-run total,** because the Work *list* needs `$0.41` per row and cannot issue one
`/v1/llm/usage?task_id=` per row:

```rust
// crates/openalpaca_storage/src/repository/llm_usage/mod.rs
pub struct TaskCost { pub task_id: String, pub cost_usd: f64,
                      pub tokens_in: i64, pub tokens_out: i64, pub requests: i64 }

/// SELECT task_id, SUM(cost_usd), SUM(input_tokens), SUM(output_tokens), COUNT(*)
/// FROM llm_call_log WHERE task_id IN (…) GROUP BY task_id
pub fn cost_for_tasks(&self, task_ids: &[String]) -> Result<Vec<TaskCost>>;
```

and enrich `GET /v1/tasks` / `GET /v1/tasks/{id}` with a `cost` object beside the existing
`assigned_agents` enrichment (`routes/tasks.rs:158-176`). That is one grouped query per list page.
`run-model.ts:runMeta` (`apps/openalpaca-gui/src/components/work/run-model.ts:248-258`) then appends
the cost segment it currently omits by design.

### 8c — the cap is not served

`execution.agent_defaults.max_cost = 1.00` and `execution.lead_agent_defaults.max_cost = 5.0`
(`crates/openalpaca_core/src/daemon_config/execution.rs:20,36,49,69`; `config/daemon.toml:2,11`),
plus `orchestrator.costs.*_max_daily_cost_usd` (`daemon_config/orchestrator.rs:177-208`). The only
daemon-config route is `/v1/daemon/config/providers`, which returns web-search settings.

Note what these actually are: **per-workflow** caps, not a daily spend ceiling. The design's
`$0.0184 of $5.00 cap` in Settings → Connection is therefore reading the lead-agent per-run cap as if
it were a daily budget. That is a product question, not just an API one — flag it.

**Route:**

```
GET /v1/usage/summary?window=today
200 {
  "window": "today",
  "date": "2026-09-01",
  "cost_usd": 0.0184,
  "tokens_in": 34120,
  "tokens_out": 7005,
  "requests": 88,
  "runs_started": 15,
  "caps": {
    "lead_agent_max_cost_usd": 5.0,
    "agent_max_cost_usd": 1.0,
    "summary_max_daily_cost_usd": 0.5,
    "extract_max_daily_cost_usd": 0.25
  },
  "by_provider": [ { "provider": "anthropic", "cost_usd": 0.0184, "tokens": 41125, "requests": 71 } ]
}
```

`caps` is a nested object, deliberately, so the client cannot mistake a per-run cap for a daily one:
the field names carry their own semantics. `window` accepts `today` only for now; reject anything
else with 400 rather than silently defaulting.

**Rust** — new `apps/openalpacad/src/routes/usage.rs`:

```rust
pub async fn usage_summary_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<UsageSummaryQuery>,      // { window: Option<String> }
) -> impl IntoResponse
```

Sources: `LlmUsageRepository::query_daily_usage(None, Some(&today), 365)` for the totals;
`state.daemon_config.load().execution` + `.orchestrator.costs` for the caps;
`router.cost_tracker.all_provider_usage().await` for `by_provider` (same call
`get_provider_usage` already makes at `settings.rs:488`) — **note that provider usage is lifetime,
not today** (`API_MAP.md:208` says so). Either label the field `by_provider_lifetime`, or derive
today's per-provider figures from `llm_call_log` with a
`SELECT provider, SUM(cost_usd), … WHERE timestamp >= date('now') GROUP BY provider`. I recommend
the latter — it is one more query and it makes the whole payload mean one thing.

`runs_started`: `SELECT COUNT(*) FROM task WHERE created_at >= <today 00:00 local>`. Needs a small
`TaskRepository::count_created_since(&DateTime<Utc>)`. **Timezone caveat:** `llm_usage_daily.date` is
written from the daemon's local date (`main.rs:328`, `format("%Y-%m-%d")` on `Utc::now()` — i.e.
*UTC* date), while `todayIsoDate()` in the client is *local*
(`apps/openalpaca-gui/src/lib/api/usage.ts:52-57`). These disagree for up to 12 hours a day for a
non-UTC user. Serving the summary server-side fixes it by making one clock authoritative; say
explicitly in the response which (`"date"` field) so the client can render "today (UTC)" if needed.

Registration: `.route("/v1/usage/summary", get(crate::routes::usage_summary_handler))`.

---

## 7. GAP-09 — subagent timeline (the hardest)

### What the UI actually consumes

`apps/openalpaca-gui/src/components/work/ParallelWork.tsx` converts a `TaskTimeline`
(`src/lib/api/unbacked.ts:134-141`) into `Lane[]` percentages:

- `lanesFromTimeline` (`ParallelWork.tsx:61-81`) needs, per lane: `label`, `started_at`,
  `ended_at | null`, `state`, `detail | null`, and optional `steps_current`/`steps_total`.
- The axis (`axisLabels`, `:85-102`) needs the run's `started_at`, `now`, `completed_at | null`.
- `laneState` (`:38-50`) maps `done → done`, `blocked → block`, everything else → `run`.
  `failed`/`cancelled` are drawn neutral with the state spelled into the detail text — so the API
  must send those states, not fold them into `done`.
- Lanes are keyed by `label` (`:129`, `:170`), so **labels must be unique within a run.**
  Two `explore_agent` spawns need `explore·1` and `explore·2`, not two `explore`s.

### Design: one span table + one live event, written at the existing spawn site

#### Migration 037 (part 1)

Do **not** extend `agent_task_history`. It is FK-bound to `agent(id)` with template ids
(`007_subagents.sql:29`), it writes a timezone-less `completed_at` (`repository/subagent/mod.rs:232`),
and it is consumed by `GET /v1/tasks(/{id})`'s legacy `assignments`/`assigned_agents` payload
(`routes/tasks.rs:32-46`, `:203-214`). Widening it drags that contract along. A purpose-built table
is smaller in blast radius and can be indexed for the timeline query.

```sql
-- 037_run_observability.sql (part 1 of 2)

-- One row per subagent span within a lead-agent workflow, written at SPAWN
-- and updated at completion. No FK to agent(id): agent_instance_id is an
-- ephemeral in-memory instance, not a persisted agent row.
CREATE TABLE IF NOT EXISTS subagent_span (
    id                TEXT PRIMARY KEY,           -- the spawn's node_id (UUID)
    task_id           TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    template_id       TEXT NOT NULL,              -- e.g. "research_agent"
    agent_instance_id TEXT NOT NULL,              -- e.g. "research_agent::7c11"
    label             TEXT NOT NULL,              -- "explore·2" — unique per task
    objective         TEXT NOT NULL,              -- first 200 chars, for hover/detail
    state             TEXT NOT NULL DEFAULT 'running'
                      CHECK(state IN ('running','done','failed','blocked','cancelled')),
    detail            TEXT,                       -- "awaiting you", "12 files read"
    started_at        TEXT NOT NULL,              -- RFC 3339
    ended_at          TEXT,                       -- RFC 3339, NULL while live
    duration_ms       INTEGER,
    output_preview    TEXT
);
CREATE INDEX IF NOT EXISTS idx_subagent_span_task
    ON subagent_span(task_id, started_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_subagent_span_label
    ON subagent_span(task_id, label);
```

`id` is deliberately the existing `node_id` UUID minted at `runner/lead_agent/tools.rs:233`, so the
already-live `DagNodeStatus` events and the persisted spans share a key — a client can correlate the
socket with the fetched timeline for free.

#### Repository

```rust
// crates/openalpaca_storage/src/repository/subagent_span/mod.rs  (new)
pub struct SubagentSpan {
    pub id: String, pub task_id: String, pub template_id: String,
    pub agent_instance_id: String, pub label: String, pub objective: String,
    pub state: String, pub detail: Option<String>,
    pub started_at: DateTime<Utc>, pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>, pub output_preview: Option<String>,
}

impl<'a> SubagentSpanRepository<'a> {
    pub fn open(&self, span: &SubagentSpan) -> Result<()>;
    pub fn finish(&self, id: &str, state: &str, ended_at: DateTime<Utc>,
                  duration_ms: i64, output_preview: Option<&str>) -> Result<()>;
    pub fn set_state(&self, id: &str, state: &str, detail: Option<&str>) -> Result<()>;
    pub fn list_for_task(&self, task_id: &str) -> Result<Vec<SubagentSpan>>;
    /// Next ordinal for a template within a task → the "·2" in "explore·2".
    pub fn next_ordinal(&self, task_id: &str, template_id: &str) -> Result<u32>;
    /// Crash recovery: any span still 'running' for a terminal task is stale.
    pub fn close_orphans(&self, task_id: &str, at: DateTime<Utc>) -> Result<usize>;
}
```

#### Where exactly to write — `runner/lead_agent/tools.rs`

**Open the span** immediately after the existing `DagNodeStarted` publish, `tools.rs:232-240`:

```rust
// 4. Emit DagNodeStarted (reusing event, node_id = UUID)
let node_id = Uuid::new_v4().to_string();
// ── NEW: persist the span and derive the lane label ──
let label = self.span_label(agent_id);           // "explore·2" (see below)
if let Some(ref db) = self.db {
    let _ = SubagentSpanRepository::new(db).open(&SubagentSpan {
        id: node_id.clone(), task_id: self.task_id.clone(),
        template_id: agent_id.to_string(), agent_instance_id: instance_id.clone(),
        label: label.clone(), objective: objective.chars().take(200).collect(),
        state: "running".into(), detail: None,
        started_at: Utc::now(), ended_at: None, duration_ms: None, output_preview: None,
    });
}
self.bus.publish(SystemEvent::SubagentSpan { /* phase: "started", … */ });
self.bus.publish(SystemEvent::DagNodeStarted { … });   // unchanged, keep for compat
```

`self.db` is already a field (`tools.rs:48`), so no signature change.

**Label derivation.** `SpawnSubagentTool` already holds `spawn_count: AtomicUsize` (`tools.rs:71`,
incremented at `:241`). Per-template ordinals need a per-template counter; the cheapest correct
source is `next_ordinal(task_id, template_id)` from the DB (the unique index makes a collision an
error we can retry once). Label = the template's short name + `·` + ordinal, where the short name is
`template_id` with a trailing `_agent` stripped — `research_agent` → `research·1`. Without a DB,
fall back to `format!("{template_id}·{}", spawn_count)`.

**Close the span** at the two completion sites, alongside the existing `DagNodeCompleted` publishes:

- plugin path — `tools.rs:524-543` (before the `record_agent_history` call at `:548`)
- LLM path — `tools.rs:610-628` (before `record_llm_usage` at `:631`)

Both already compute `duration_ms`, `agent_success`/`outcome.success()`, `now` and an
`output_preview`. State mapping:

```rust
let state = if cancelled_token_fired { "cancelled" }
            else if agent_success     { "done" }
            else                      { "failed" };
```
`agent_success` is derived at `tools.rs:598-604` from `LoopFinishReason`; `Cancelled` currently
folds into the `false` branch, so distinguish it explicitly rather than reporting a cancel as a
failure (the UI has separate copy for each — `ParallelWork.tsx:52-57`).

Also add a **cancelled-before-start** close at `tools.rs:477-486` (the early-return branch when the
parent token is already cancelled), otherwise those spans stay `running` forever.

**The `lead` lane.** The design shows `lead` as the first swimlane. It has no span row because the
lead agent is not spawned through `SpawnSubagentTool`. Two options:

- (a) Write a `lead` span in `spawn_lead_agent_execution` (`dispatcher/lead_agent.rs:224-246`), where
  `lead_agent.id`, `lead_agent.template_id`, `task_id` and the start instant are all in scope, and
  close it in the same block that calls `finalize_task_with_outcome` (`:507-512`).
- (b) Synthesise it at the route from `task.created_at`/`completed_at`.

**Recommend (a).** It costs ~10 lines, and it makes the lead lane's `state` honest (`blocked` when
the lead itself is awaiting a confirmation) instead of a derived approximation. `steps_current` /
`steps_total` for the lead lane come from `task.progress_current` / `progress_total`, which
`dispatcher/lead_agent.rs:240-241` already publishes.

**The `blocked` state.** `ConfirmationBroker` (`crates/openalpaca_core/src/security/confirmation.rs:39-63`)
stores only `DashMap<request_id, oneshot::Sender<_>>` — no metadata, so nothing can currently ask
"which agent is waiting?". Minimal change:

```rust
// security/confirmation.rs
pending: DashMap<String, (ConfirmationRequest, oneshot::Sender<ConfirmationResponse>)>,
pub fn pending_requests(&self) -> Vec<ConfirmationRequest>;
```

and add `task_id: Option<String>` + `agent_instance_id: Option<String>` to `ConfirmationRequest`
(built at `sandbox/mod.rs:212-220`, where `ctx.task_id` is in scope — see §0.3). The timeline route
then overlays `state = "blocked"`, `detail = "awaiting you"` on any span whose
`agent_instance_id` has a pending request. This keeps the blocked state derived rather than
persisted, which is right: it is inherently ephemeral.

**Note on `ToolContext.agent_id`.** For subagents it is set to the **template** id
(`tools.rs:251`, `agent_id: Some(agent_id.to_string())`), while the loop runs under `instance_id`
(`tools.rs:585`). To overlay confirmations onto the right lane, add
`agent_instance_id: Option<String>` to `ToolContext` and set it at `tools.rs:250-266`. Do **not**
repurpose `agent_id` — it is what `CapabilityManager::check_agent_capability` reports in violations
(`security/capabilities/mod.rs:91-116`) and what agent-scoped tools read.

#### The route

```
GET /v1/tasks/{id}/timeline
200 {
  "task_id": "b41c8e02",
  "started_at": "2026-09-01T14:22:41Z",
  "now":        "2026-09-01T14:33:07Z",
  "completed_at": null,
  "lanes": [
    { "lane_id": "lead", "label": "lead", "template_id": "lead_agent",
      "agent_instance_id": "lead_agent::9f21",
      "started_at": "2026-09-01T14:22:41Z", "ended_at": null,
      "state": "running", "detail": "orchestrating",
      "steps_current": 5, "steps_total": 8 },
    { "lane_id": "3f0c…", "label": "review·1", "template_id": "review_agent",
      "agent_instance_id": "review_agent::7c11",
      "started_at": "2026-09-01T14:27:44Z", "ended_at": null,
      "state": "blocked", "detail": "awaiting you" }
  ]
}
404 { "error": "Task not found" }
```

`lane_id` = `subagent_span.id`. This matches `TimelineLane`
(`apps/openalpaca-gui/src/lib/api/unbacked.ts:121-132`) field-for-field, so
`getTaskTimeline` in `unbacked.ts:147-150` becomes a real fetch with no view changes.

Handler in `apps/openalpacad/src/routes/tasks.rs`:

```rust
pub async fn task_timeline_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse
```
1. `TaskRepository::get(&id)` → 404 if none. `started_at = task.created_at`,
   `completed_at = task.completed_at`, `now = Utc::now()`.
2. `SubagentSpanRepository::list_for_task(&id)`.
3. Overlay `blocked` from `state.confirmation_broker.pending_requests()`.
4. If the task is terminal, any span still `running` is stale (daemon crash) —
   report it as `cancelled` with `detail: "interrupted"` rather than a lane that never ends.
   `close_orphans` can also be run once at boot over terminal tasks.

Register: `.route("/v1/tasks/{id}/timeline", get(crate::routes::task_timeline_handler))`.

#### The live event

```rust
// crates/openalpaca_core/src/events.rs  (SystemEvent)
SubagentSpan {
    task_id: String,
    lane_id: String,                 // = span id = node_id
    label: String,
    template_id: String,
    agent_instance_id: String,
    /// "started" | "progress" | "blocked" | "unblocked" | "finished"
    phase: String,
    /// "running" | "done" | "failed" | "blocked" | "cancelled"
    state: String,
    detail: Option<String>,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    timestamp: DateTime<Utc>,
}
```

```rust
// crates/openalpaca_api/src/events/mod.rs  (ServerEvent) — same fields, `ts` + `instance_id`
SubagentSpan { task_id, lane_id, label, template_id, agent_instance_id,
               phase, state, detail, started_at, ended_at, ts, instance_id }
```

Bridge arm in `apps/openalpacad/src/event_bridge.rs` next to the DAG arms (`:219-258`), builder
`EventBroadcaster::subagent_span` next to `dag_node_status`
(`apps/openalpacad/src/events/handlers.rs:158-180`), and a `persist` arm next to the
`DagNodeStatus` one (`apps/openalpacad/src/events/persistence.rs:260-275`) writing
`event_type = "subagent_span"` with the new `task_id` column from §7 below.

Emitted at three points: span open (§ above), `blocked`/`unblocked` transitions (from the sandbox's
confirmation request/response, `sandbox/mod.rs:227` and `:250-…`), and span close.

**Keep `DagNodeStatus` emitting.** It is live today and the GUI renders it
(`run-events.ts:63-71`). Deprecate it in a later pass once `SubagentSpan` has shipped and the client
has switched — not in the same change.

### Scope note

This is the largest item in Lens B: ~1 migration, 1 repository, ~6 edit sites in `tools.rs`, 1 in
`dispatcher/lead_agent.rs`, 2 small changes in `security/` (broker metadata, `ToolContext`), 1 route,
1 event through 4 layers. It is worth doing as one milestone with GAP-10, which shares the migration
and the `ToolContext` change.

---

## 8. GAP-10 — per-run event log

### Current state

- `event_log` (`migrations/001_init.sql:17-28`): `id, timestamp, agent_id, event_type, detail, result`.
  No `task_id`.
- `GET /v1/events/history` (`apps/openalpacad/src/routes/events_history.rs:18-51`) accepts
  `{ limit, agent_id }`, calls `EventLogRepository::by_agent` or `recent`
  (`repository/event_log/mod.rs:44-79`), and returns a **bare array**.
- The tool/LLM/security events genuinely lack a task attribution *on the wire* — but see §0.3:
  `ctx.task_id` is already at the emit site.
- The workflow events are worse than "missing": `TaskStatus` (`events/persistence.rs:59-80`),
  `DagNodeStatus` (`:260-275`), `WorkflowStarted`/`WorkflowSteered`/`WorkflowProgress`
  (`:308-341`) all **already carry the task id** — buried in the `detail` JSON blob, with the
  `agent_id` column set to `None`. The rows exist; there is no column to filter them by.

### Migration 037 (part 2)

```sql
-- 037_run_observability.sql (part 2 of 2)
ALTER TABLE event_log ADD COLUMN task_id TEXT;
CREATE INDEX IF NOT EXISTS idx_event_log_task ON event_log(task_id, id DESC);
```

Nullable, no backfill — historical rows simply have no run. `(task_id, id DESC)` because pagination
is keyset-on-`id`, not on `timestamp` (`timestamp` is a TEXT column written in two different formats,
see `repository/event_log/mod.rs:96-105`; ordering on it is fragile, ordering on the autoincrement id
is not).

### Storage

```rust
// crates/openalpaca_storage/src/repository/event_log/mod.rs
pub fn log_for_task(
    &self, event_type: &str, agent_id: Option<&str>, task_id: Option<&str>,
    detail: Option<&serde_json::Value>, result: Option<&serde_json::Value>,
) -> Result<i64>;

pub struct EventQuery<'q> {
    pub task_id: Option<&'q str>,
    pub agent_id: Option<&'q str>,
    pub event_type: Option<&'q str>,
    pub before_id: Option<i64>,     // keyset cursor
    pub limit: usize,
}
pub fn query(&self, q: &EventQuery<'_>) -> Result<Vec<EventLog>>;
```

Keep `log`, `recent` and `by_agent` as thin wrappers so no existing caller changes.
`EventLog` (`crates/openalpaca_storage/src/models/core.rs`) gains
`pub task_id: Option<String>`.

### Event plumbing (no further SQL)

Add `task_id: Option<String>` to these `SystemEvent` variants and their `ServerEvent` twins:

| Variant | Emitter | `task_id` source |
|---|---|---|
| `ToolExecuted` | `security/sandbox/mod.rs:378-386` | `ctx.task_id` |
| `SecurityViolation` | `security/sandbox/mod.rs:352-359` | `ctx.task_id` |
| `ToolConfirmationRequested` | `security/sandbox/mod.rs:227-236` | `ctx.task_id` |
| `CircuitBreakerTripped` | (breaker call sites in `sandbox/mod.rs`) | `ctx.task_id` |
| `LlmCallCompleted` | `orchestrator/dispatcher/usage.rs:66-73` | `task_id` — **already a parameter** at `:19` |

`emit_security_violation` (`sandbox/mod.rs:352`) and `emit_tool_executed` (`:379`) are private
helpers taking `agent_id: &str`; give each a `task_id: Option<&str>` parameter and pass
`ctx.task_id.as_deref()` from the seven call sites, all inside `execute_tool` where `ctx` is in
scope: violations at `:152`, `:165`, `:285` and the timeout path at `:343`; executions at `:292`,
`:325`, `:331`.

Then update `EventBroadcaster::persist` (`apps/openalpacad/src/events/persistence.rs`) to call
`log_for_task` with the id for every variant that now has one, plus `TaskStatus` (`:59-80`) and the
new `SubagentSpan`.

**Coverage note, and it is worth stating plainly:** the WS stream forwards far more variants than
`persist` writes. After this change the *run-scoped* subset is complete — task status, subagent
spans, tool executions, security violations, confirmations, LLM calls — which is exactly the
`tag ∈ tool | steer | artifact | spawn | run` vocabulary the UI asks for
(`apps/openalpaca-gui/src/lib/api/unbacked.ts:154`). The `artifact` tag needs Lens A's artifact
creation to publish an event; assume it does and persist it with `task_id`.

`WorkflowStarted` / `WorkflowSteered` / `WorkflowProgress` **are** already persisted
(`events/persistence.rs:308-341`) — but each buries `task_id` inside the `detail` JSON and passes
`None` for the `agent_id` column, so a run-scoped query cannot reach them without a JSON scan.
Switch those three (and `FollowupQueued`, `:343-354`, on `source_task_id`) to `log_for_task` with the
id in the new column. Same for `TaskStatus` (`:59-80`) and `DagNodeStatus` (`:260-275`). This is the
bulk of GAP-10's value: the `steer` and `run` rows the design shows already exist in the table and
are merely unqueryable.

### Route

```
GET /v1/events/history?task_id=b41c…&limit=200&before=8812&event_type=tool_executed
200 { "events": EventLog[], "next_before": 8791 }
```

`next_before` is the smallest `id` in the page, or `null` when the page is short of `limit`.

**Back-compat:** the un-filtered call (`?limit=`, `?agent_id=`) keeps returning a **bare array** —
the CLI parses it that way today. Return the envelope **only** when `task_id` or `before` is present.
That is slightly ugly but it is the honest smallest change; the alternative is a `/v2` route.
The GUI's `RunEventPage` type (`unbacked.ts:164-167`) already expects `{ events, next_before }`.

```rust
// apps/openalpacad/src/routes/events_history.rs
#[derive(Debug, Deserialize)]
pub struct HistoryParams {
    pub limit: Option<usize>,
    pub agent_id: Option<String>,
    pub task_id: Option<String>,      // new
    pub event_type: Option<String>,   // new
    pub before: Option<i64>,          // new — keyset cursor
}
```

---

## 9. GAP-23 — message → run / artifact links

**Persistence is indeed the only missing piece for `task_id`. Artifacts need one link-table write.**

### `task_id` on the message

`crates/openalpaca_core/src/gateway/router/mod.rs:255-285`: the `Ok(result)` arm calls
`persist_assistant_message(&lane_key_str, &result.content, Some(duration_ms), &source_name)` at
`:260-265` and then reads `result.delegation` at `:285` to build the `GatewayResponse`. The
`DelegationInfo { task_id, title }` (`:26-29`) is right there and simply not written.

**Migration 038:**

```sql
-- 038_message_run_links.sql
ALTER TABLE conversation_messages ADD COLUMN task_id TEXT;
CREATE INDEX IF NOT EXISTS idx_conv_msg_task ON conversation_messages(task_id);
UPDATE schema_version SET version = 38 WHERE version = 37;
```

**Model:** `ConversationMessage` (`crates/openalpaca_storage/src/models/conversation.rs:7-20`)
gains `pub task_id: Option<String>`. This is a **wide struct-literal change** — every construction
site must add the field. Verified sites:
`gateway/persistence.rs:34-46`, `:106-119`, `:156-168`, plus `ConversationRepository::insert` /
`insert_with_structured` / the row mappers, and the daemon's chat history serialization. Adding
`#[serde(default)]` on the model does not help construction; a `Default`-based builder or
`..Default::default()` would, if the struct gains `#[derive(Default)]`. Worth doing at the same time
— three literal sites in `persistence.rs` alone.

**Write path:**

```rust
// crates/openalpaca_core/src/gateway/persistence.rs
pub fn persist_assistant_message(
    &self, lane_key: &str, content: &str,
    duration_ms: Option<i64>, source: &str,
    task_id: Option<&str>,                 // ← new
) -> Result<i64>
```
and at `gateway/router/mod.rs:260`, pass
`result.delegation.as_ref().map(|d| d.task_id.as_str())`. That is a two-line change once the
signature moves.

**Read path:** `GET /v1/chat/history` and `GET /v1/conversations/{id}/messages` serialize
`ConversationMessage` directly, so `task_id` appears with no route change. Confirm the GUI's
`ChatMessage` type gains the field.

### `artifact_ids` on the message

**No new table.** `conversation_message_attachments`
(`migrations/028_message_attachments.sql:2-11`) already carries
`role TEXT NOT NULL DEFAULT 'attachment'`. Inputs are written with the default role today
(`crates/openalpaca_storage/src/repository/file_asset/mod.rs:115-130`, called from
`gateway/persistence.rs:122-131`). Outputs get `role = 'artifact'`.

```rust
// crates/openalpaca_storage/src/repository/file_asset/mod.rs
pub fn link_to_message_with_role(
    &self, message_id: i64, file_id: &str, sort_order: i32,
    role: &str, caption: Option<&str>,
) -> Result<()>;
// existing link_to_message becomes a wrapper passing role = "attachment"
```

**Uncertainty — where the ids come from.** `HandleResult`/`DelegationInfo` carry no artifact ids
today, and the delegation is emitted *when the workflow starts*, long before it produces anything.
The assistant message that starts a run cannot know its artifacts. Two shapes, and this is a real
design fork:

- **(A) Link at completion, not at send.** The workflow's completion report is itself posted to the
  lane as a message (`dispatcher/lead_agent.rs` finalize path → `persist_conversation`). *That*
  message is the one that should carry both `task_id` and `role='artifact'` links, because by then
  Lens A's `artifacts` table has rows with `task_id = <this task>`. This is the shape the design's
  `RunReportCard` actually needs (`apps/openalpaca-gui/src/components/chat/RunReportCard.tsx`) —
  it is a *finished*-run card.
- **(B) Link the starting message and let the client join.** Store `task_id` on the delegating
  message only, and let the client fetch `GET /v1/artifacts?task_id=` on demand.

**Recommend (A) for artifacts and (B) for `task_id`, i.e. both:** the delegating message gets
`task_id` (so "which turn started this run" survives a reload), and the completion message gets
`task_id` **plus** `role='artifact'` links (so the recap card rebuilds without a second fetch).
Since the artifact link is `message_id → artifacts.id`, it depends on Lens A's table existing;
if `artifacts` and `file_assets` are separate tables, the FK on
`conversation_message_attachments.file_id REFERENCES file_assets(id)` blocks reuse and a sibling
`conversation_message_artifacts` table is needed instead. **Flagging this as the single hardest
coordination point with Lens A.**

Serialization: expose `artifact_ids: string[]` on the message payload by joining the link table in
the history query, so the client does not need a second round trip.

---

## 10. Suggested order

| Step | Items | Why |
|---|---|---|
| 1 | GAP-07, GAP-08a, GAP-08b | XS; no migration; unblocks titles and per-run cost immediately |
| 2 | GAP-02, GAP-03 | S/M; pure route work over existing storage and primitives |
| 3 | migration 037 + GAP-10 | Adds `event_log.task_id` and the `ToolContext`/sandbox `task_id` passthrough that GAP-09 also needs |
| 4 | GAP-09 | Reuses 037's table and the sandbox change from step 3 |
| 5 | GAP-06 | Needs `task.source_task_id` (037) and the dispatcher entry point; the `start` id-injection is the messiest bit and benefits from landing last |
| 6 | migration 038 + GAP-23 | Depends on Lens A's `artifacts` shape being settled |
| 7 | GAP-08c (`/v1/usage/summary`) | Nice-to-have; the client already computes it |

---

## 11. Open questions for the synthesizer / user

1. **Migration numbers.** 035/036 to Lens A, 037/038 to Lens B — confirm, or renumber.
2. **`task.workspace_id`** (§5b). Adding it makes `rerun` faithful and gives Lens A a per-task project
   dir to hang `.openalpaca/runs/<task_id>/` off. I recommend adding it; it is one nullable column.
3. **`start` id-injection** (§5c). Honouring "200, same task id" requires
   `dispatch_lead_agent_with_id` and a create-or-update in the persist step. The alternative — `start`
   returns a *new* id like `rerun` — is cleaner code and a small client change. Which?
4. **`caps` semantics** (§6c). `execution.lead_agent_defaults.max_cost = 5.0` is a **per-workflow**
   cap; the design reads `$X of $5.00 cap` as a **daily** budget. Do we want a genuine daily spend
   cap in `[orchestrator.costs]`, or should the UI relabel?
5. **Steering `workspace_path`** (§3). Should `POST /v1/tasks/{id}/steer` accept one, so converted
   leftovers re-enter scoped to the project?
6. **Artifact→message links** (§9). Are Lens A's artifacts rows in `file_assets` (reuse
   `conversation_message_attachments`) or a new `artifacts` table (needs a sibling link table)?
7. **`DagNodeStatus` deprecation.** Keep emitting alongside `SubagentSpan` indefinitely, or remove it
   one release after the client switches?
