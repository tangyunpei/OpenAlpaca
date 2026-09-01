# Lens S — Session persistence (rev 2 new pillar)

**Status:** design, no code written. · **Date:** 2026-09-01 · **Branch:** `feat/ui-rework`
**Directive served:** "Local persistence of session + project association + session recovery" — SQLite tables + per-session append-only JSONL event log + filesystem tiers.
**Operates under the rev 2 decisions:** D1 (single root — `app_dir()` itself moves to `~/.openalpaca/`), D2 (uploads move in too), D4 (`OPENALPACA_HOME_STORE` override), and directive 1 (app is not distributed → legacy-compat measures are dropped *and cleared*).
**Coordinates with:** Lens R (root taxonomy — this lens claims the `sessions/` namespace under the root; R must reserve it), rev 1 `tasks/api-fix-plan.md` (migrations 035–037, GAP-10/21/23 interplay, §1.7's `TaskWorkspace` spill).

---

## 0. Recommendations at a glance

| # | Question | Recommendation |
|---|---|---|
| S1 | What is a session? | **A session is a conversation: one resumable transcript, bound to at most one workspace.** It is an *epoch of a lane* — the existing `conversations` table already is the session table, minus three defects (one-row-per-lane `UNIQUE`, no workspace column, no lifecycle). Rebuild it as `session`, allow many per `lane_key`, at most one `active`. |
| S2 | Runtime vs persistence | **Runtime stays lane-keyed; persistence becomes session-keyed.** `SharedContext` (steering inboxes, cancellation tokens, `active_workflows_by_lane`) does not change. The gateway resolves `lane_key → active session id` at persist time. |
| S3 | The user's table list | `conversations` → rebuilt as `session` (038). `messages`/`tasks`/`memories`/`agents` → already exist; gain `session_id` where useful. `tool_calls`+`tool_results` → **one** evolved table: extend `tool_execution_log`, payloads in JSONL. `plans` → **no new table** — post-planner-deletion, a "plan" is `task.description` + plan-kind artifacts + `outcome_json`; a plans table would rebuild the deleted DAG planner's storage with no producer. |
| S4 | JSONL role | **JSONL is authoritative for loop-interior events** (tool calls/results, rounds, steering drains, compaction, subagent spans' narrative); **SQLite stays authoritative for chat content, task state, and addresses**; **files hold bulk payloads**. One source of truth per event class (§4.4); no payload is ever written twice. |
| S5 | Where session data lives | `~/.openalpaca/sessions/<session-id>/{log.jsonl, results/, snapshots/}` — under the **home root, never the project dir**. Project association is a DB column, not a file location: transcripts contain persona/memory/cross-project content and must not be committable into a repo by accident. |
| S6 | Recovery of in-flight workflows | Supersede the boot orphan sweep in two steps: **S-phase 1** — non-terminal tasks become a new `interrupted` status (not `failed`), undrained steering recovered from JSONL into `lane_followups`; restart = existing `rerun`. **S-phase 2** — true transcript replay resume from JSONL, opt-in. |
| S7 | Hot path | Loop emits log records over an unbounded-in-practice mpsc `try_send` to a per-session writer task; `BufWriter` flush per record, `fsync` only at turn/workflow boundaries + 5 s timer. No per-token anything. |
| S8 | HTTP surface | `/v1/sessions` family **replaces** `/v1/conversations` (deleted, not aliased — directive 1). `GET /v1/chat/history` is retargeted to the lane's *active* session. rev 1's GAP-21 (conversation rename/delete) is re-homed onto sessions instead of being built twice. |

---

## 1. Ground truth — what exists today (verified)

### 1.1 Conversations and messages

- `conversations`: `id TEXT PK, lane_key TEXT NOT NULL UNIQUE, source, title, message_count, last_message_at, created_at, updated_at` (`crates/openalpaca_storage/src/migrations/011_unified_conversations.sql:3-12`) plus summary columns from 014 (`summary, summary_version, last_summarized_message_id, summary_updated_at` — `014_conversation_summary.sql:1-4`). **The `UNIQUE` on `lane_key` is the whole reason multi-conversation doesn't exist**: one row per lane, forever. It is a *column-level* constraint, so removing it requires a table rebuild, not an index drop.
- `conversation_messages`: keyed by **`lane_key`, not conversation id** (`009_conversation_messages.sql:1-13`; `source` added at `011:19`, `content_json`/`display_text` at `028_message_attachments.sql:13-14`). Messages have no session identity at all.
- Writers: `GatewayPersistence::persist_user_message` calls `get_or_create_conversation(lane_key, source)` then inserts by lane (`crates/openalpaca_core/src/gateway/persistence.rs:30-49`); the workflow completion report is persisted the same way (`orchestrator/dispatcher/lead_agent.rs:463-494`).
- Readers: the main loop's prompt window is `list_recent_by_lane(lane_key, prompt_recent_messages)` (`orchestrator/context_builder.rs:30-38`, repo at `repository/conversation/mod.rs:95-110`); the GUI reads `GET /v1/chat/history` (`routes/chat.rs:214-228`) and `GET /v1/conversations(/{id}/messages)` (`router.rs:144-150`).
- The in-memory `ConversationLane` is **caches only** — key, counters, compose-engine caches (`lane/types/mod.rs:93-100`). Nothing durable lives there; lanes already rebuild lazily after restart. Chat persistence is in decent shape; it lacks *identity* (sessions), not durability.

### 1.2 Tasks and the blob offender

- `task`: 006 base + `state_json`/`state_version` (016) + `outcome_json`/`outcome_kind`/`artifact_count` (029). No session or workspace column today (rev 1's 035 adds `workspace_id`, 036 adds `source_task_id`). `TaskStatus` is `Queued|Running|Completed|Failed|Cancelled|Paused` (`models/task.rs:9-16`) and `task.status` has **no CHECK constraint** (`006_tasks.sql:4-18`) — adding a status is a code-only change.
- `state_json` holds the whole `TaskWorkspace` — up to 50 entries × 32 KB inline content (`task_state/workspace.rs:20-47`), rewritten under optimistic locking per mutation. rev 1 §1.7 already fixes the artifact-kind entries by spilling to the artifact store; this lens **does not duplicate that** and adds nothing to `state_json`.
- `lane_followups` (033): lane-keyed queue of `followup` / `unprocessed_steering` rows, claimed CAS-style and auto-started at workflow finalize (`dispatcher/lead_agent.rs:506-545`, runner in `apps/openalpacad/src/followup.rs`).

### 1.3 Telemetry tables (what the user's `tool_calls`/`tool_results` must not duplicate)

- `event_log` (001): `id, timestamp, agent_id, event_type, detail JSON, result` (`001_init.sql:17-31`). Written by the daemon's event broadcaster persist hook (`apps/openalpacad/src/events/persistence.rs:9-…`; skips heartbeats, logs task/workflow/followup events with the task id buried in `detail` — `:308,:330,:343`). rev 1 migration 036 adds `event_log.task_id` + `GET /v1/events/history?task_id=` (GAP-10).
- `llm_call_log` + `llm_usage_daily` (008): every LLM call with tokens/cost, task-indexed (`008_llm_usage.sql:6-35`).
- `tool_execution_log` (030): `request_id, agent_id, tool_name, success, duration_ms, error_message, timestamp` (`030_skill_tool_execution_log.sql:29-40`). **No arguments, no results** — tool payloads are persisted *nowhere* today.
- `agent` (001, widened by 007) + `agent_metrics` + `agent_task_history` (007) — completion-time-only rows (rev 1 §5's correction); rev 1's `subagent_span` (036) is the live per-run record. This lens adds no agent tables.
- `memory` (015): owner/kind/scope model, workspace-scoped via `scope_id` = canonical path. Sessions touch memory only through the existing workspace scoping; no schema change.

### 1.4 The restart story, today

1. **Boot orphan sweep fails everything non-terminal.** `sweep_orphaned_tasks` runs right after the DB opens and marks every `queued|running|paused` task `failed` with "daemon restarted — task orphaned" (`apps/openalpacad/src/bootstrap/migration.rs:21-27` → `TaskRepository::fail_all_non_terminal`, `repository/task/mod.rs:187-202`). Correct given that nothing can be resumed — and exactly the thing session recovery supersedes.
2. **The loop transcript is memory-only.** The agentic loop holds its conversation as `Arc<Vec<ChatMessage>>` (`runner/agentic_loop/mod.rs:205`) and `LoopResult` returns **only** `final_content` + counters — no transcript (`runner/agentic_loop/config.rs:263-276`). A daemon crash mid-workflow loses every round, every tool call, every tool result; even a *successful* run discards them.
3. **Steering dies with the process.** `SteeringInbox` is in-memory on `SharedContext` (`context/shared/mod.rs:134-142`); the graceful exit converts leftovers to `unprocessed_steering` follow-ups (`dispatcher/lead_agent.rs:300-355`), but a crash converts nothing — the messages are simply gone.
4. **Clients have nothing to resume onto.** `GET /v1/tasks` shows the orphan-failed rows; the GUI reloads lane history; the CLI has no resume verb at all (`apps/openalpaca/src/commands/chat.rs` — send-a-message only).

### 1.5 Workspace association

`handle_message_internal` resolves `workspace_id` from the request's `workspace_path` else **the daemon CWD** (`orchestrator/handlers.rs:88-97`); `workspace_id` **is** the canonical root path (`memory/workspace.rs:60-65`). No client sends `x-workspace-path` today (rev 1 §1.8, verified there). Everything in this lens rides on rev 1's fix: GUI (and now CLI) send the header; the CWD fallback is never used for placement.

---

## 2. The session concept

### 2.1 Definition

> **A session is one conversation transcript: an epoch of a lane, bound to at most one workspace, with a lifecycle (`active` → `archived`) and a durable event log.**

Rejected alternatives:

- *Session = workflow run.* That object already exists — it is `task`, with its own id, state, outcome, and (036) spans. Renaming it buys nothing; a conversation spawns many tasks.
- *Session = client connection.* Connector lanes (Telegram, iMessage) are connectionless; the GUI reconnects constantly. A connection-scoped session would fragment one conversation into dozens of "sessions".
- *Session = brand-new object beside conversations.* Would leave two overlapping transcript containers and force every reader to join both. The `conversations` row **is** the session; it just needs the `UNIQUE(lane_key)` broken and a lifecycle added.

This matches the Claude Code model the sketch pointed at: a session is a resumable transcript bound to a project. Mapping onto what exists:

| Claude Code notion | OpenAlpaca object |
|---|---|
| session | `session` row (rebuilt `conversations`) |
| project | `workspace_id` = canonical root path (already the memory-scoping key) |
| transcript | `conversation_messages` (now `session_id`-keyed) + the session's JSONL for loop detail |
| a task/subagent run inside a session | `task` row (+ 036 `subagent_span`) with `task.session_id` |
| `--resume` | `POST /v1/sessions/{id}/activate` + CLI/GUI pickers |

### 2.2 Lifecycle

- **Begin:** a session is created (a) explicitly — GUI "New chat" / `POST /v1/sessions` — or (b) implicitly, when a message arrives on a lane with no active session (`get_or_create_active_session`, the evolution of `get_or_create_conversation` at `repository/conversation/mod.rs:146`). Creation archives any previously-active session on that lane — the partial unique index makes "at most one active per lane" a DB invariant, not a convention.
- **Workspace binding:** set from the first `x-workspace-path` seen on the session, updatable via `PATCH`. One workspace per session; changing project = new session (matches the GUI's mental model and keeps memory scoping coherent). `NULL` = no project.
- **End:** `archived` — explicit, or implicit via a new active session on the lane. Archived sessions are fully readable and can be re-activated. **Connector lanes get one perpetual active session for now**; an idle-auto-archive knob (`[orchestrator.sessions] idle_archive_hours`, default off) is reserved, not built.
- **Runtime stays lane-keyed (S2).** Steering, cancellation, `active_workflows_by_lane`, followup claiming — all unchanged in `SharedContext`. Sessions are a persistence identity, resolved once per turn at the gateway. This keeps the change surface out of the loop's hot path and out of the steering rail entirely.

### 2.3 What "recovery" means, concretely

**(a) Chat lanes.** Already durable; recovery is an *identity* fix, not a durability fix. After 038 the GUI lists sessions per workspace, reopens the active one, and fetches its transcript by `session_id` — instead of one endless per-lane history. The in-memory lane rebuilds lazily as today (`lane/types/mod.rs:93-100` is caches only — nothing to recover).

**(b) In-flight workflows.** Two phases:

- **S-phase 1 — honest interruption.** The boot sweep stops lying: non-terminal tasks become `interrupted` (new `TaskStatus` variant; code-only — `models/task.rs:9-16` + `as_str`/parse + GUI copy; no CHECK constraint blocks it, §1.2). `interrupted` is terminal for lane bookkeeping (workflow-context block, followup autostart) but carries a `restartable` affordance: the GUI's "Restart" is rev 1's `rerun` (GAP-06, new id, `source_task_id` link). Additionally the sweep **reads each open session's JSONL tail**: steering records with no subsequent `steering_drained` marker are converted to `lane_followups(kind='unprocessed_steering')` — the exact rows the graceful path already writes (`dispatcher/lead_agent.rs:300-355`) — so a crash no longer silently eats interjections. Sweep ordering guarantee (before ingress — `bootstrap/migration.rs:13-18`) is preserved; the JSONL read adds only file I/O.
- **S-phase 2 — replay resume (opt-in).** For an `interrupted` task whose session log contains a complete record chain (delegation → rounds → last durable record), `POST /v1/tasks/{id}/action {"action":"resume"}` rebuilds the loop's `Vec<ChatMessage>` from the log (§4.6) and re-enters `run_agentic_loop_routed` under the **same task id** (consistent with D5's same-id philosophy). This requires the JSONL to carry full `tool_use` arguments and tool results (or spill refs) — which is why they are authoritative there (§4.4). Gated by `[orchestrator.sessions] resume_enabled` (default off until trusted); `rerun` remains the fallback.

**(c) GUI/CLI after daemon restart.** The clients recover from the DB, not from daemon memory: `GET /v1/sessions?workspace_id=` + `GET /v1/sessions/{id}/messages` + `GET /v1/tasks?status=interrupted`. The GUI reopens its last session (its own localStorage keeps the id; the server list is the fallback); the CLI gains `--resume`/`--session` (§6.3). No daemon-side state is needed beyond what 038 persists.

---

## 3. SQLite layer — migration 038

### 3.1 Mapping the user's table list onto reality

| Sketch table | Verdict | How |
|---|---|---|
| `conversations` | **exists → rebuilt** | becomes `session` (§3.2). One table, renamed while it is being rebuilt anyway. |
| `messages` | exists | `conversation_messages` + new `session_id` (backfilled), on top of 037's `task_id`. |
| `tasks` | exists | + `session_id`. (`workspace_id` already lands in 035, `source_task_id` in 036.) |
| `plans` | **not created** | post-planner-deletion a plan is: the spec (`task.description`), plan-kind artifacts (`ArtifactKind::Plan` → `.md`, rev 1 §1.3), and the outcome (`task.outcome_json` + the completion-report message, which 037 links via `task_id`). A `plans` table would resurrect the deleted DAG planner's storage with no writer. If a first-class step-checklist is ever wanted it is a `WorkspaceEntryType` addition, reserved. |
| `tool_calls`, `tool_results` | **one evolved table** | extend `tool_execution_log` (§3.3) — it already has the single writer in the right place (the sandbox execute path), and a call has at most one result. Payloads go to JSONL/files; rows are the queryable index. |
| `agents` | exists | `agent`/`agent_metrics`/`agent_task_history` (001/007) + 036's `subagent_span`. Nothing added. |
| `memories` | exists | `memory` (015). Nothing added; sessions inherit workspace scoping. |
| `sessions` | **new** = the rebuild | §3.2. |

### 3.2 `038_sessions.sql`

```sql
-- Migration 038: sessions. Rebuilds `conversations` as `session`
-- (drops the column-level UNIQUE(lane_key); adds workspace + lifecycle),
-- keys messages/tasks/followups by session, and turns tool_execution_log
-- into the tool-call index for the session event log.

CREATE TABLE session (
    id             TEXT PRIMARY KEY,               -- UUIDv4 (same ids carried over)
    lane_key       TEXT NOT NULL,                  -- routing address, no longer UNIQUE
    source         TEXT NOT NULL,
    title          TEXT DEFAULT '',
    workspace_id   TEXT,                           -- canonical project root; NULL = none
    status         TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
    message_count  INTEGER DEFAULT 0,
    last_message_at TEXT,
    summary        TEXT NOT NULL DEFAULT '',       -- carried from 014
    summary_version INTEGER NOT NULL DEFAULT 0,
    last_summarized_message_id INTEGER NOT NULL DEFAULT 0,
    summary_updated_at TEXT,
    ended_at       TEXT,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO session (id, lane_key, source, title, workspace_id, status, message_count,
                     last_message_at, summary, summary_version, last_summarized_message_id,
                     summary_updated_at, created_at, updated_at)
SELECT id, lane_key, source, title, NULL, 'active', message_count,
       last_message_at, summary, summary_version, last_summarized_message_id,
       summary_updated_at, created_at, updated_at
FROM conversations;
DROP TABLE conversations;

-- The invariant: at most one active session per lane.
CREATE UNIQUE INDEX idx_session_active_lane ON session(lane_key) WHERE status = 'active';
CREATE INDEX idx_session_workspace ON session(workspace_id, updated_at DESC);
CREATE INDEX idx_session_updated   ON session(updated_at DESC);
CREATE INDEX idx_session_source    ON session(source);

ALTER TABLE conversation_messages ADD COLUMN session_id TEXT;
UPDATE conversation_messages
   SET session_id = (SELECT s.id FROM session s WHERE s.lane_key = conversation_messages.lane_key);
CREATE INDEX idx_conv_msg_session ON conversation_messages(session_id, id);

ALTER TABLE task ADD COLUMN session_id TEXT;
CREATE INDEX idx_task_session ON task(session_id);

-- Follow-ups remember the conversation they came from (§3.4).
ALTER TABLE lane_followups ADD COLUMN session_id TEXT;

-- tool_execution_log → the tool-call index. Payloads live in the session
-- JSONL (or its results/ spill tier); rows hold previews and the pointer.
ALTER TABLE tool_execution_log ADD COLUMN session_id TEXT;
ALTER TABLE tool_execution_log ADD COLUMN task_id TEXT;
ALTER TABLE tool_execution_log ADD COLUMN log_seq INTEGER;      -- seq of the tool_call record
ALTER TABLE tool_execution_log ADD COLUMN args_preview TEXT;    -- ≤ 2048 chars
ALTER TABLE tool_execution_log ADD COLUMN result_preview TEXT;  -- ≤ 2048 chars
ALTER TABLE tool_execution_log ADD COLUMN result_ref TEXT;      -- 'log:<seq>' | 'file:results/<...>'
CREATE INDEX idx_tel_session ON tool_execution_log(session_id, id);
CREATE INDEX idx_tel_task    ON tool_execution_log(task_id, id);

UPDATE schema_version SET version = 38 WHERE version = 37;
```

Notes, honestly stated:

- **The rebuild is the unavoidable cost of breaking `UNIQUE(lane_key)`** — SQLite cannot drop a column constraint. Since the table is being copied anyway, renaming to `session` costs only the SQL strings inside `ConversationRepository` (376 lines, one file — `repository/conversation/mod.rs`) plus the handful of raw references in the daemon routes. **Rust type names (`Conversation`, `ConversationRepository`, `ConversationMessage`) do not rename in this migration's PR** — that is churn across gateway/chat/orchestrator with zero behavior change; do it opportunistically or never.
- **Struct churn is already paid.** 037 (rev 1 Phase 5) adds `#[derive(Default)]` to `ConversationMessage` precisely because adding a field breaks every literal; `session_id` lands as `..Default::default()`-absorbed. **038 therefore sequences after 037.**
- **No FKs on the new columns.** `ADD COLUMN REFERENCES` is legal only with a NULL default and `foreign_keys=ON` would enforce it (`database/mod.rs:62`) — fine — but session deletion is repo-level transactional anyway (matching rev 1's GAP-21 guidance, which found 011 declares no cascade). One transaction deletes the session row, its messages, and nulls `task.session_id`; leaving cascades out keeps the delete path in one visible place.
- Backfill: one session per existing lane, `status='active'` — exactly today's semantics, so nothing observable changes until a client creates a second session.
- `llm_call_log` already has `task_id` (`008:6-19`); joining call costs to a session goes through `task` or the message row — no column needed.

### 3.3 Why extend `tool_execution_log` instead of new `tool_calls`/`tool_results` tables

- **The single writer already exists at the single right place** — the sandboxed execute path that every tool call funnels through (rev 1 §5 GAP-10: `SandboxManager::execute_tool` has `ctx.task_id` at every emit site). A parallel table would need the same writer and would duplicate `tool_name/success/duration/timestamp` row-for-row.
- **A call has at most one result** — two tables buy a join and nothing else.
- **Payloads don't belong in rows** (the directive's own rows/JSONL/files split): `args_preview`/`result_preview` serve list views; `log_seq`/`result_ref` address the authoritative record. GAP-18's `invocations_today` aggregation keeps working unchanged.

### 3.4 Follow-ups and sessions

A queued follow-up is a promise to continue *that* conversation. Rule: **the follow-up turn runs in its originating session** — `spawn_followup` re-activates `lane_followups.session_id` if it has been archived (one `UPDATE … SET status='active'` + archive of the usurper, inside the claim transaction). Without this, a follow-up queued in conversation A would silently append its turn to whatever conversation B the user opened since — visibly wrong in a session-aware GUI. Rows with `session_id IS NULL` (pre-038) fall back to the lane's active session.

### 3.5 The rows / JSONL / files rule (the whole layer in three lines)

- **Rows**: anything a list filters, sorts, joins, or resolves — session metadata, message content (the GUI transcript), task state, tool-call previews + pointers, artifact addresses (rev 1).
- **JSONL**: the ordered narrative of what happened in a session — authoritative for loop-interior events that rows only index. Append-only, never updated.
- **Files**: any payload over the spill threshold, and all binary payloads.

`TaskWorkspace` stays fixed by rev 1 §1.7 (artifact-kind entries spill to the artifact store; `state_json` keeps ≤512-char previews). This lens deliberately adds nothing to `state_json` — the JSONL logs workspace *mutations* as events, but current-state remains the DB's job.

---

## 4. The JSONL event log

### 4.1 Placement and layout

```
~/.openalpaca/                       ← the single root (D1); OPENALPACA_HOME_STORE honored (D4)
  sessions/                          ← namespace this lens claims from Lens R
    <session-id>/                    ← UUID dir; one per session, created lazily on first record
      log.jsonl                      ← the active segment
      log.<first_seq>-<last_seq>.jsonl  ← rotated segments, if any (§4.5)
      results/                       ← spilled tool results: <seq>-<tool-slug>.<ext>
      snapshots/                     ← reserved (§5.3)
```

- **Home root, never the project dir** (S5). A session bound to `/Users/x/dev/proj` still logs under `~/.openalpaca/sessions/` — transcripts carry persona, memory, and cross-project content, and a project dir can be committed to git. The project link is `session.workspace_id`.
- Path helpers in `openalpaca_storage::paths`: `sessions_dir()`, `session_dir(id)`, `session_log_path(id)`, `session_result_path(id, seq, tool, ext)` — pure, beside rev 1's artifact helpers. Lens R's root README lists `sessions/` as "conversation event logs — machine-readable, do not edit".
- `DELETE /v1/sessions/{id}` removes the directory in the same operation as the row transaction (row first, then dir; a leftover dir with no row is swept opportunistically at boot). No automatic GC otherwise — session logs are the user's history.

### 4.2 Record envelope

One JSON object per line:

```json
{"v":1,"seq":184,"ts":"2026-09-01T10:22:03.114Z","type":"tool_result",
 "task_id":"3f2a1b7c-…","agent":"research_agent::a1b2c3d4","data":{…}}
```

- `v` — envelope version (1). `seq` — per-session, strictly monotonic, gap-free, assigned by the single writer task; the resume cursor for `/events` and the pointer target for `tool_execution_log.log_seq`.
- `ts` — RFC 3339 UTC, millisecond precision.
- `type` — §4.3. `task_id` — present on workflow-interior records, absent on main-loop/chat records. `agent` — runtime instance id where applicable.
- `data` — type-specific payload. Hard cap 64 KB per record; larger payloads spill (§4.5) and `data` carries the spill ref instead.

### 4.3 Event catalog

| `type` | When | `data` (essentials) |
|---|---|---|
| `session_start` | first record | lane_key, source, workspace_id |
| `session_end` | archive/delete | reason |
| `user_msg` | gateway persists a user turn | msg_id (row id), preview ≤512, attachment file_ids |
| `assistant_msg` | gateway persists an assistant turn / completion report | msg_id, preview ≤512, model, task_id if report |
| `delegation` | `start_workflow` fires | task_id, title, spec preview |
| `round` | each agentic-loop LLM round completes | round #, model, tokens in/out, stop reason, assistant text, `tool_use` blocks **verbatim** (id, name, full input JSON) |
| `tool_call` | sandbox dispatches a tool | tool_use_id, name, full args (or spill ref) |
| `tool_result` | tool returns | tool_use_id, status, duration_ms, full result (or spill ref) |
| `steering` | `push_steering` accepts a message | text, request_id |
| `steering_drained` | loop injects interjections at a round boundary | request_ids |
| `confirmation_req` / `confirmation_res` | broker request / resolution | confirmation id, tool, scope, decision |
| `subagent_open` / `subagent_close` | lead spawns / closes a subagent | span id (= 036 span id), template, label, state |
| `compaction` | context compaction runs | before/after token estimates |
| `artifact_written` | mirror of rev 1's `ArtifactWritten` event | artifact_id, rel_path, version |
| `followup_queued` | `queue_followup` | followup row id, preview |
| `workflow_done` | finalize | task_id, outcome_kind, success |
| `error` | loop/LLM/tool hard error | class, message |

`round` carrying the assistant's `tool_use` blocks verbatim is what makes S-phase 2 replay possible — the Anthropic-style transcript alternates assistant(`tool_use`) / user(`tool_result`), and both sides must be reconstructible bit-for-bit.

### 4.4 One source of truth per event class (no triple-writing)

Three sinks exist today: the WS broadcast (`/v1/events`, ephemeral — `router.rs:268-270`), the `event_log` table (persist hook, `events/persistence.rs`), and after 036, `subagent_span`/`event_log.task_id`. The rule:

| Event class | Source of truth | Others carry |
|---|---|---|
| Chat message **content** | `conversation_messages` row | JSONL: msg_id + preview only. Content is written once. |
| Task current state / outcome | `task` row | JSONL narrates transitions; WS streams them. |
| Tool call/result **payloads** | **JSONL** (or its `results/` spill) | `tool_execution_log`: previews + `log_seq` pointer. Nothing else ever stores the payload. |
| Loop narrative (rounds, steering drains, compaction, errors) | **JSONL** | WS streams live; `event_log` does **not** grow new copies. |
| Subagent span state | `subagent_span` (036) | JSONL `subagent_open/close` reference the span id — narrative, not state. |
| System audit (connector status, keys, wake, commands) | `event_log` | unchanged; sessions don't touch it. |
| Artifact content/addresses | filesystem + `file_assets` (rev 1) | JSONL references artifact_id. |

Consequence for rev 1 Phase 3 (GAP-10): **unchanged and unblocked** — `event_log.task_id` ships first and keeps serving `GET /v1/events/history?task_id=` from the rows that already exist. Once the JSONL is live, the *payload-bearing* per-run detail moves to `GET /v1/sessions/{id}/events`; `event_log` stays what it structurally is — a small-detail audit table. The demotion is a non-event: no writer is added to `event_log` by this lens, and none needs removing.

### 4.5 Size, rotation, spill, fsync

- **Spill threshold: 64 KB** per record (also the inline cap). A larger tool result goes to `results/<seq>-<tool-slug>.<ext>` (`.json` if it parses, else `.txt`/`.bin`), and the record carries `{"spill":{"rel":"results/000184-web_fetch.json","bytes":412300,"sha256":"…"}}`. `tool_execution_log.result_ref` = `file:results/000184-web_fetch.json`. Write protocol: tmp + rename, same as the artifact store.
- **Rotation:** when `log.jsonl` exceeds **64 MB**, rename it to `log.<first>-<last>.jsonl` and start fresh. Readers list segments, sort by first seq, and stream. At the observed prompt sizes a segment holds tens of thousands of records; most sessions will never rotate.
- **fsync policy (S7):** the writer task owns a `BufWriter`; it flushes (write syscall, page cache) after every record, and calls `sync_data` only on: `session_start`, `assistant_msg`, `workflow_done`, `confirmation_res`, `session_end`, and a 5-second timer while dirty. Crash exposure: at most the tail of the current round — acceptable, because the DB (messages, task state) is WAL-synced independently and replay tolerates a truncated tail (§4.6). **Never** per token; the token stream never touches the log (only completed rounds do).
- **Torn tails:** a crash can leave a partial last line. Readers treat an unparseable final line as end-of-log. The writer, on reopening an existing log, scans the last line and truncates a torn tail before appending.

### 4.6 Replay / recovery algorithms

**GUI transcript (everyday path):** SQLite only — `GET /v1/sessions/{id}/messages` (+ 037's `task_id` and attachment links). The JSONL powers the *expanded* turn view: `GET /v1/sessions/{id}/events?after_seq=&types=&limit=` seeks segments by seq and streams envelopes; spilled payloads are fetched by their `result_ref` on demand. The GUI needs no JSONL to render a conversation — only to open its hood.

**Boot sweep (S-phase 1):** for each task marked `interrupted`, resolve `task.session_id` → open the log → scan from the task's `delegation` record: collect `steering` records with no later `steering_drained` containing their request_id → insert as `lane_followups(kind='unprocessed_steering', session_id=…)`. Idempotent: the sweep records `swept_seq` in the session row's metadata (or simply relies on the follow-up insert being keyed by request_id — add a `request_id` uniqueness guard on insert).

**Resume (S-phase 2):** rebuild the loop input for task T in session S:
1. Compose the system/persona layers **fresh** (they are regenerated every dispatch anyway — prompt composition is pure w.r.t. the DB).
2. Seed the user objective from `task.description`.
3. Scan S's log from T's `delegation`: each `round` appends the assistant message (text + verbatim `tool_use` blocks); each `tool_result` appends the corresponding `user` tool-result message, inlining spilled payloads; `steering_drained` appends the interjection messages.
4. Stop at the last *complete* round (a `round` with all its `tool_result`s). Anything after is discarded — the model re-does at most one round.
5. Append a synthetic `<user_interjection>`: "the daemon restarted; you are resuming — verify the state of any side effects from your last round before repeating them." (Tool side effects between the last durable record and the crash are unknowable; telling the model beats pretending.)
6. Re-enter `run_agentic_loop_routed` with the rebuilt messages under the same task id; status `interrupted → running`; steering inbox re-registered fresh.

---

## 5. Filesystem tiers

### 5.1 `attachments/` — is D2's uploads tier, not a per-session copy

The sketch's `attachments/` is satisfied by the store's uploads area (Lens R's territory under D2: uploads move from `app_dir()/assets/` into the root). **Do not** materialize per-session attachment copies: upload dedup is owner-scoped sha256 (`routes/files.rs:172-185`, load-bearing per rev 1 D2 discussion), and a per-session copy breaks it and multiplies bytes. Sessions link attachments the way they already do — `conversation_message_attachments.file_id` — and the JSONL's `user_msg` records carry the file ids. Coordination point for Lens R: whatever `uploads/` layout D2 lands, `file_id` remains the join key; this lens needs nothing else from it.

### 5.2 `results/` — large tool results

Defined in §4.5: per-session, seq-named, spill-linked from both the JSONL record and `tool_execution_log.result_ref`. This is the sketch's `large-tool-results/` tier, renamed shorter because it lives inside the session dir where context is obvious.

### 5.3 `snapshots/` — reserve now, build one real thing later

What is *real and useful now*: **pre-edit images of user files the agent overwrites.** `file_write` (and rev 1's `artifact_write` for non-artifact paths) overwrites workspace files with no undo; artifacts get `.versions/` (rev 1 §1.2) but plain `file_write` into the user's project does not. S-phase 3: before an overwriting `file_write`, copy the target to `snapshots/<seq>-<slug-of-rel-path>` and log a `file_snapshot` record. That gives "what did the agent clobber?" answers and a manual restore path.

What is deliberately **not** designed: task-state checkpoints (redundant — `state_json` is already in the DB and the JSONL narrates mutations) and whole-workspace snapshots (a VCS's job, and the user has git). The namespace is reserved; nothing else is promised.

---

## 6. APIs and consumers

### 6.1 HTTP surface (replaces `/v1/conversations` — directive 1)

```http
GET    /v1/sessions?workspace_id=&source=&status=&q=&limit=&offset=   200 { "sessions": [SessionView], "total": n }
POST   /v1/sessions {source?, workspace_path?, title?}                201 SessionView   (archives the lane's previous active)
GET    /v1/sessions/{id}                                              200 SessionView | 404
GET    /v1/sessions/{id}/messages?limit=&before_id=                   200 { "messages": […], "total": n }
GET    /v1/sessions/{id}/events?after_seq=&types=&limit=              200 { "events": [envelope…], "next_after_seq": n }
POST   /v1/sessions/{id}/activate                                     200   (archives the current active on that lane)
POST   /v1/sessions/{id}/archive                                      200
PATCH  /v1/sessions/{id} {title?, workspace_path?}                    200
DELETE /v1/sessions/{id}                                              204 | 409 SESSION_HAS_ACTIVE_WORKFLOWS
```

- `SessionView`: id, lane_key, source, title, workspace_id, status, message_count, last_message_at, created_at + derived `active_task_count` (from `SharedContext::active_workflows_by_lane` when the lane matches) and `interrupted_task_count` (one grouped query).
- Envelope/pagination follow rev 1 §9's codified rule (paginated ⇒ `{items,total}`); errors use Phase 0's shared `api_error()`.
- **`POST /v1/chat` gains optional `session_id`** — appends to that session, auto-activating it (409 if it belongs to a different lane_key than the principal's). Absent ⇒ the lane's active session, created on demand — today's behavior exactly.
- **`GET /v1/chat/history` retargets** to the lane's active session (`WHERE session_id = active`) — same shape, correct scoping. The GUI's transcript view moves to `/v1/sessions/{id}/messages`.
- **rev 1 GAP-21 re-homes here**: rename = `PATCH`, delete = `DELETE` above (transactional, 409 on active workflows — same rules rev 1 specified). If sessions land before Phase 6, GAP-21 is simply this section; the `/v1/conversations` handlers (`router.rs:144-150`) are **deleted**, not aliased, and the GUI/CLI callers updated in the same PR. Nothing outside this repo calls them (directive 1).

### 6.2 Daemon/loop changes (the write path)

1. **`SessionLogService`** (new, `crates/openalpaca_core/src/session_log/`): `DashMap<SessionId, SessionLogHandle>`; `handle_for(session_id)` lazily spawns the per-session writer task (mpsc receiver → BufWriter, seq assignment, rotation, spill, fsync per §4.5); handles idle-close after N minutes. `SessionLogHandle::emit(Record)` is a non-blocking `try_send`; on a full channel it drops with a `tracing::warn!` counter — the log is an observability record, and stalling the loop for it is the wrong trade.
2. **Gateway**: `GatewayPersistence` resolves the active session once per turn and emits `user_msg`/`assistant_msg` (+ `delegation` beside where `result.delegation` is read, `gateway/router/mod.rs:285` per rev 1 §7). This is also where `conversation_messages.session_id` gets written.
3. **Loop**: `LoopConfig` gains `session_log: Option<SessionLogHandle>` next to the existing `steering` field — the identical pattern to the steering inbox. Emit points in `run_agentic_loop_inner` (`runner/agentic_loop/mod.rs`): after each LLM response (`round`), around tool dispatch in the sandbox path (`tool_call`/`tool_result` — same sites GAP-10 instruments), at the steering drain (`steering_drained`), on compaction, and at every exit (`workflow_done`/`error`). The lead runner (`runner/lead_agent/tools.rs`) adds `subagent_open`/`subagent_close` beside the 036 span writes. Main-loop turns log the same way with `task_id` absent.
4. **Steering**: `push_steering` (`runner/steering.rs:61-79`) additionally emits `steering` to the task's session log — one line, and the crash-recovery of §2.3(b) exists.
5. **Dispatch**: `dispatch_lead_agent` (`dispatcher/lead_agent.rs:23-32`) takes `session_id` (resolved by the gateway) and persists it on the task row; the completion report (`lead_agent.rs:463-494`) persists into **that** session, not the lane's currently-active one — the report belongs to the conversation that started the run.

### 6.3 CLI

- `openalpaca sessions [--workspace <path>] [--all]` — list (default: sessions for the cwd's resolved workspace).
- `openalpaca chat --resume` — activate + continue the most recent session for the cwd's workspace; `--session <id>` targets one; both print a transcript tail (`/v1/sessions/{id}/messages?limit=10`) before the prompt.
- The CLI starts sending `workspace_path` on `/v1/command` (the field exists — `routes/command.rs:78-80`; no caller sets it), completing rev 1 §1.8's client story for both clients.

### 6.4 GUI

Session sidebar per workspace (list/create/rename/archive/delete = §6.1), "Interrupted" badge + Restart (rerun) on `status=interrupted` tasks, and the expanded-turn tool timeline fed by `/v1/sessions/{id}/events`. The WS stream is unchanged; a new `ServerEvent::SessionChanged { session_id, lane_key, status, ts, instance_id }` keeps a second window honest on create/activate/archive.

---

## 7. Legacy clears in this lens's territory (directive 1)

Cleared — deleted, not merely bypassed:

1. **`resolve_local_user_id`'s `gui_user` adoption** (`bootstrap/migration.rs:50-56`): the legacy-history check fires only when `identity.local_user_id` was never persisted; every live install has persisted it on first run. Delete the branch; keep UUID generation.
2. **`migrate_preference_summaries`** (`bootstrap/migration.rs:77+`): a run-once shim that runs every boot. Delete after one final run on the user's own DB.
3. **rev 1's dual-shape `GET /v1/events/history`** (§5 of rev 1 — bare array without filters, envelope with): drop the compat; always return the envelope; update the one CLI call site (`apps/openalpaca/src/commands/tasks.rs:310`). *This revises rev 1.*
4. **`/v1/conversations` routes** (`router.rs:144-150`): deleted with the 038 PR (§6.1), and rev 1's GAP-21 is built once, on sessions, instead of twice.

Explicitly **not** legacy — do not clear: `conversation_map` (live platform-chat-id → lane mapping used by every connector — `crates/openalpaca_connectors/src/{telegram,imessage,discord}/…`), `lane_key` on `conversation_messages` (still the routing address and the index the prompt window reads — `context_builder.rs:30-38`), and `event_log` (system audit, §4.4).

---

## 8. Sequencing and effort

Dependencies, stated as prerequisites:

| Phase | Contents | Depends on | Effort |
|---|---|---|---|
| **S0 — schema + surface** | Migration 038; `get_or_create_active_session`; `/v1/sessions` family; `POST /v1/chat session_id`; `task.session_id` at dispatch; completion report into originating session; follow-up session pinning (§3.4); `/v1/conversations` deleted; GUI sidebar; CLI `sessions`/`--resume` | rev 1 Phase 5 (037's `Default` on `ConversationMessage`); D1 root move only for path *constants* (falls back cleanly if R lands later); clients sending workspace (rev 1 §1.8) | **L** (schema+repo M, routes S, GUI M) |
| **S1 — the log** | `SessionLogService` + writer; emit points (gateway, loop, lead runner, steering); `tool_execution_log` columns used; `results/` spill; `GET …/events`; `interrupted` status; sweep rewrite (mark interrupted + steering recovery); `SessionChanged` event | S0 (session ids); D1 (the `sessions/` dir lives under the moved root); coordinates with rev 1 Phase 3 (shares the sandbox instrumentation sites — do the emit-point work in the same PRs where practical) | **L** |
| **S2 — replay resume** | §4.6 algorithm; `action:"resume"`; `resume_enabled` config; synthetic resume interjection | S1 (needs verbatim `tool_use`/`tool_result` in the log) | **L / day+** — the only speculative piece; everything before it is useful without it |
| **S3 — snapshots** | `file_write` pre-edit images (§5.3) | S1 (seq + log records) | **M**, deferrable indefinitely |

Interleaving with rev 1's phases: S0 slots cleanly **after Phase 5** (it consumes 037's churn payment and GAP-23's `task_id` links) and can run in parallel with Phase 6; S1's loop instrumentation overlaps Phase 3's GAP-10 sites — sequence S1 after Phase 3 so the sandbox passthrough (`ctx.task_id`, `agent_instance_id`) exists to reuse. Migration ledger extends: **038 = sessions** (this lens), leaving 035–037 exactly as rev 1 numbered them. `database/tests.rs:11`'s head-version assertion updates with it.

*Verify (per phase):* S0 — two sessions on one lane with interleaved messages produce two clean transcripts; the partial unique index rejects a second `active`; a workflow's completion report lands in its originating (archived) session; follow-up autostart re-activates its session. S1 — a two-subagent run yields a well-formed log (seq gap-free, every `tool_call` matched by `tool_result` or the loop exit); a >64 KB tool result spills and `result_ref` resolves; kill -9 mid-round leaves a parseable log (torn tail truncated on reopen) and the sweep marks the task `interrupted` and recovers an undrained steering message into `lane_followups`. S2 — resume of an interrupted run replays to the last complete round and finishes; resume with a gutted log falls back to a clean 409 pointing at `rerun`.

---

## 9. Open questions (genuinely open — everything else above is decided)

1. **Connector session boundaries.** One perpetual active session per Telegram/iMessage lane is shipped; is an idle-auto-archive (e.g. 72 h) wanted, and should a chat command (`/new`) open a session from inside a connector? Reserved knob exists; needs a product call.
2. **Should `assistant_msg` JSONL records carry full content** instead of msg_id + preview? Cost: every transcript byte twice. Benefit: the log alone reconstructs a session without the DB. Current call is *no* (DB is authoritative for content); flip it only if export-a-session-as-one-file becomes a feature.
3. **Retention.** Session dirs are never GC'd. If `results/` spill ever measurably grows the root, a size-capped LRU over *archived* sessions' `results/` (never `log.jsonl`) is the pressure valve — config-gated, default off, same posture as rev 1's "produced artifacts are never garbage-collected".
