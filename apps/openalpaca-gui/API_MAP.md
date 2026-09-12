# OpenAlpaca GUI — API Map (UI ↔ Daemon)

Maps every data need of the new React UI (design canvas `OpenAlpaca.dc.html`, 1440×900,
four views: **Chat**, **Work**, **Library**, **Settings** + command palette) onto the
daemon's real HTTP/SSE/WS surface.

**Sources verified (all field names below are read from these files, not inferred):**

| What                                     | File                                                                                                                                              |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| All routes                               | `apps/openalpacad/src/router.rs`                                                                                                                  |
| Chat req/resp                            | `apps/openalpacad/src/routes/chat.rs`, `chat_types.rs`                                                                                            |
| Tasks                                    | `apps/openalpacad/src/routes/tasks.rs`, `tasks_types.rs`                                                                                          |
| Settings/LLM/usage/models                | `apps/openalpacad/src/routes/settings.rs`, `settings_types.rs`                                                                                    |
| Agents/templates/instances               | `apps/openalpacad/src/routes/agents.rs`, `agents_types.rs`                                                                                        |
| Files                                    | `apps/openalpacad/src/routes/files.rs`, `files_types.rs`                                                                                          |
| Extensions / connectors / skills / tools | `routes/extensions.rs`, `routes/connectors.rs`, `routes/skills.rs`, `routes/tools.rs` (`routes/plugins.rs` was deleted with `/v1/plugins*` in C7) |
| Telemetry                                | `routes/events_history.rs`, `routes/orchestrator_latency.rs`, `routes/dispatch_decisions.rs`                                                      |
| WS event union                           | `crates/openalpaca_api/src/events/mod.rs` (`ServerEvent`)                                                                                         |
| Internal→WS bridge                       | `apps/openalpacad/src/event_bridge.rs`                                                                                                            |
| SSE event union                          | `crates/openalpaca_core/src/chat/stream_manager/mod.rs` (`ChatStreamEvent`)                                                                       |
| SSE lifecycle                            | `crates/openalpaca_core/src/chat/service.rs`, `apps/openalpacad/src/background.rs`                                                                |
| Storage models                           | `crates/openalpaca_storage/src/models/*.rs`, `src/repository/*`                                                                                   |
| Existing client                          | `apps/openalpaca-gui/src/lib/daemon.ts`, `src/lib/api/*.ts`                                                                                       |
| Tauri host                               | `apps/openalpaca-gui/src-tauri/src/lib.rs`, `crates/openalpaca_storage/src/discovery/mod.rs`                                                      |

> **Security note.** The design file was read as _data only_. It contains no text
> directed at the agent; all strings in it are UI copy and mock fixtures. Nothing in it
> was treated as an instruction.

---

## 1. Connection & auth

### 1.1 How it works today

The daemon writes `~/.openalpaca/state/discovery.json`
(`crates/openalpaca_storage/src/discovery/mod.rs`):

```jsonc
{
  "schema": 1,
  "instance_id": "<uuid v4>",
  "pid": 51823,
  "started_at": "<RFC3339 UTC>",
  "listen": { "host": "127.0.0.1", "port": 51823 },
  "auth": {
    "token": "<43-char base64url, 32 bytes>",
    "issued_at": "...",
    "expires_at": "...",
  },
  "build": { "version": "0.4.1", "protocol": 1 },
}
```

The webview never reads this file. Two Tauri commands do
(`apps/openalpaca-gui/src-tauri/src/lib.rs`, registered in `tauri::generate_handler!`):

| Tauri command           | Behavior                                                                                                                                                                                                                                                                                                                                             | Returns          |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------- |
| `get_connection_info`   | `discovery::read_discovery()` → `discovery::ensure_not_expired()` → `ConnectionInfo::from(&d)`. Errors if missing/expired.                                                                                                                                                                                                                           | `ConnectionInfo` |
| `ensure_daemon_running` | Reads discovery; probes liveness by TCP-connecting to `listen.host:listen.port` with a 300 ms timeout (`daemon_is_alive`); if dead, spawns the `openalpacad` sidecar detached (`setsid` on unix, `DETACHED_PROCESS` on windows) with `OPENALPACA_CONFIG_DIR=<app_dir>/config`, then polls `read_discovery()+daemon_is_alive` 25 × 200 ms (≈5 s cap). | `ConnectionInfo` |

`ConnectionInfo` (Rust, serialized to the webview in **camelCase**):

```ts
interface ConnectionInfo {
  baseUrl: string;
  token: string;
  instanceId: string;
}
// baseUrl = `http://{listen.host}:{listen.port}`
```

Auth on the wire:

- **HTTP** — every `/v1/*` route except `/` and `/v1/health` sits behind
  `crate::middleware::auth_middleware`; send `Authorization: Bearer <token>`.
- **WebSocket** `/v1/events` — token as a **query param** (`?token=...`), validated
  inline in `events_handler` (browsers cannot set WS headers).
- **SSE** `/v1/chat/stream/{stream_id}` — token as a **query param**, validated inline
  in `chat_stream_handler` (`EventSource` cannot set headers). This route is _merged
  outside_ the auth middleware layer for exactly this reason.
- CORS is `CorsLayer::permissive()`.

Liveness / identity guards the React client must reproduce:

- `GET /v1/health` (no auth) → `{ status, version, pid, instance_id }`. Compare
  `instance_id` against the cached `ConnectionInfo.instanceId`; a mismatch means the
  daemon restarted → re-bootstrap (drop stream state, refetch everything).
- The existing `daemon.ts` reconnect loop does this: on WS close it re-invokes
  `get_connection_info` and, if `instanceId` changed, calls `connectToDaemon()`
  (full re-bootstrap) instead of just reopening the socket.
- Backoff: base 1000 ms, ×2, cap 30 000 ms, ±20 % jitter, reset on `onopen`.

### 1.2 What the React client must reproduce

1. A `connection` module wrapping `invoke("ensure_daemon_running")` on boot and
   `invoke("get_connection_info")` on reconnect. Keep `{baseUrl, token, instanceId}`
   in a store/context; **all** fetches read from it.
2. An `apiFetch(path, init)` helper that injects `Authorization: Bearer`.
3. A `wsUrl(path)` helper: `baseUrl.replace(/^http/, "ws") + path + "?token=" + encodeURIComponent(token)`.
4. An `sseUrl(streamId)` helper (token in query string).
5. The instance-id guard above, driving the Settings → **Connection** panel's
   `Daemon connected` / `Reconnect` state.
6. Keep the design's "connected · 7f3a" chip fed by `instanceId.slice(0, 4)` +
   WS `connectionState`.

---

## 2. Endpoint map

`✅` = exists and fits · `⚠️` = exists but awkward / partial · `❌` = **gap** (see §3).

### 2.1 Chat view

| UI element (design)                                        | Endpoint / event                                                                                                                                                                                                                                                                                               | Request                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | Response (real field names)                                                                                                                                                                                                                                      |
| ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Send composer                                              | ✅ `POST /v1/chat`                                                                                                                                                                                                                                                                                             | `{ content: string, attachments?: AttachmentRef[], session_id?: string, activate?: bool }` — `AttachmentRef { file_id, caption? }`. Optional header `x-workspace-path`. `session_id` is sent only for a **resumed** conversation: an unnamed turn lands in the lane's active session, which is what lets the daemon open a new one when the project changed (R48); a named one takes that conversation's project, overriding the header (R49). Without `activate`, an archived target answers `409 SESSION_ARCHIVED` | `{ stream_id: string, lane_key: string }`; `409 SESSION_ARCHIVED` \| `SESSION_LANE_MISMATCH`, `404` for an unknown **or foreign** id                                                                                                                             |
| Streaming reply                                            | ✅ `GET /v1/chat/stream/{stream_id}?token=` (SSE)                                                                                                                                                                                                                                                              | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | events `thinking` / `delta` / `done` / `error` / `confirmation_requested` — see §4                                                                                                                                                                               |
| Assistant meta line `sonnet-4-6 · 3.8s · 1284/612 tok`     | ✅ SSE `done`                                                                                                                                                                                                                                                                                                  | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | `{ content, model, tokens_in, tokens_out, duration_ms, attachments_used?: string[], delegation?: { task_id, title } }`                                                                                                                                           |
| "Started a background workflow" card                       | ✅ SSE `done.delegation` + WS `workflow_started`                                                                                                                                                                                                                                                               | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | `delegation { task_id, title }`; WS `{ type:"workflow_started", task_id, lane_key, title, ts, instance_id }`                                                                                                                                                     |
| Transcript on load / scrollback                            | ✅ `GET /v1/chat/history?limit&offset&lane_key&session_id` — retargeted to a **session** by migration 039; omitting `session_id` reads the lane's active one                                                                                                                                                   | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | `{ messages: ConversationMessage[], total: i64, lane_key: string, session_id: string \| null }` — `session_id` is what the sidebar highlights, and `null` on a lane that never held a turn                                                                       |
| `ConversationMessage` fields                               | —                                                                                                                                                                                                                                                                                                              | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | `{ id: i64, lane_key, role, content, source?, model?, tokens_in?, tokens_out?, duration_ms?, created_at: string, content_json?, display_text?, task_id?, artifacts: MessageArtifact[] }` — `MessageArtifact { id, name, kind? }` (GAP-23)                        |
| Clear lane                                                 | ✅ `DELETE /v1/chat/history?lane_key=`                                                                                                                                                                                                                                                                         | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | `{ deleted: u64 }` (also clears the conversation summary)                                                                                                                                                                                                        |
| **Conversation sidebar** — the lane's list                 | ✅ `GET /v1/sessions?source=gui&limit=` — live conversation first, then archived by `updated_at`; the lane restriction is client-side (the route filters by workspace/source/status/`q`, not lane)                                                                                                             | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | `{ sessions: SessionView[], total: i64 }` — see §2.4                                                                                                                                                                                                             |
| `New chat`                                                 | ✅ `POST /v1/sessions { source: "gui", workspace_path? }` — **archives the lane's previous active** in the same transaction                                                                                                                                                                                    | body carries the window's project, or omits it entirely (`""` is the daemon's _unbind_ spelling, not "none")                                                                                                                                                                                                                                                                                                                                                                                                         | `201 SessionView`                                                                                                                                                                                                                                                |
| resume a conversation (click a row)                        | ✅ `POST /v1/sessions/{id}/activate` — archives the incumbent; skipped for a row that is already active, where it would announce a transition that did not happen                                                                                                                                              | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | `200 SessionView`; `404 SESSION_NOT_FOUND` (unknown **or** another owner's)                                                                                                                                                                                      |
| rename / archive / delete a row                            | ✅ `PATCH /v1/sessions/{id} { title }`, `POST /v1/sessions/{id}/archive`, `DELETE /v1/sessions/{id}` — GAP-21, closed                                                                                                                                                                                          | `PATCH` sends only the named fields: a rename carrying the session's own `workspace_path` would be refused for no visible reason                                                                                                                                                                                                                                                                                                                                                                                     | `200 SessionView` / `204`; `409 SESSION_HAS_ACTIVE_WORKFLOWS`, `409 SESSION_WORKSPACE_BOUND` (R48)                                                                                                                                                               |
| project change ⇒ new conversation                          | ✅ **daemon-side (R48)** — the gateway archives the active session and opens one bound to the new project on the turn whose `x-workspace-path` differs. The client must **not** create one; it drops its resume pointer (so the turn names no session and the switch is detectable) and refreshes on the event | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | WS `session_changed { session_id, lane_key, status: "active"\|"archived"\|"deleted"\|"interrupted", task_id?, ts, instance_id }` — `interrupted` is not a session state: it names a run the boot sweep found in flight (§5.6b)                                   |
| per-turn event log (expanded turn)                         | ✅ `GET /v1/sessions/{id}/events?after_seq=&types=&limit=&agent=&span_id=` → `{events, next_after_seq}` — the per-session JSONL (T42); the GUI does not read it yet                                                                                                                                            | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | —                                                                                                                                                                                                                                                                |
| Confirmation card `Confirmation required · shell_execute`  | ✅ SSE `confirmation_requested` **and** WS `tool_confirmation_requested`                                                                                                                                                                                                                                       | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | SSE `{ request_id, tool_name, tool_arguments }`; WS adds `{ agent_id, stream_id?, lane_key? }`                                                                                                                                                                   |
| `Approve ↵` / `Deny esc`                                   | ✅ `POST /v1/chat/confirmations/{request_id}`                                                                                                                                                                                                                                                                  | `{ approved: boolean }`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | `200 OK` (empty) / `404 { error: { code:"NOT_FOUND", message } }`                                                                                                                                                                                                |
| `Always allow` (allowlist)                                 | ✅ same route, `approval_scope: "entire_tool"` — GAP-01, resolved `88e8a3b`                                                                                                                                                                                                                                    |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |                                                                                                                                                                                                                                                                  |
| 👍/👎 on a message                                         | ✅ `PUT\|GET\|DELETE /v1/chat/messages/{message_id}/feedback`                                                                                                                                                                                                                                                  | PUT `{ feedback: "positive"\|"negative", comment?: string }`                                                                                                                                                                                                                                                                                                                                                                                                                                                         | `{ message_id, feedback, comment? }` / `{ deleted: bool }`                                                                                                                                                                                                       |
| Model picker (`claude-sonnet-4-6 ▴`)                       | ✅ **GAP-13, closed** — `GET /v1/models` lists them and `POST /v1/chat`'s optional `model` runs **this turn** on the pick; nothing is persisted, so the picker no longer writes the daemon default (that is Settings → Models & keys' `PUT /v1/orchestrator/config`)                                           |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | `/v1/models` → `available_models()` (`DiscoveredModel[]`); chat POST → `{ stream_id, lane_key, model_used }`, `400 UNKNOWN_MODEL` for an id the registry does not hold (a disabled provider's models included), `503 LLM_NOT_CONFIGURED` when there is no router |
| `{{ spend }} today` footer                                 | ✅ `GET /v1/orchestrator/config.daily_cost_usd` — GAP-08a, resolved `7dbb988` (sums today's UTC `llm_usage_daily` rows server-side)                                                                                                                                                                            |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | `OrchestratorConfigResponse.daily_cost_usd: f64`                                                                                                                                                                                                                 |
| `steer → connector audit` user bubble                      | ✅ **GAP-02, closed** — `POST /v1/tasks/{id}/steer`, addressed at the run the composer is aimed at; not a chat turn, so the bubble is session-local (the daemon persists no message for a steer)                                                                                                               | `{ message, workspace_path? }` — the client omits `workspace_path`, so the run's own project applies                                                                                                                                                                                                                                                                                                                                                                                                                 | `{ task_id, accepted, inbox_depth, lane_key }`; `409 STEERING_INBOX_FULL`\|`TASK_NOT_STEERABLE`, `503 STEERING_DISABLED`, `400 EMPTY_MESSAGE`, `404 NOT_FOUND`                                                                                                   |
| `Queue follow-up` composer mode                            | ✅ **GAP-03, closed** — `POST /v1/lanes/{lane_key}/followups`, parked on the lane the composer is on; like a steer the bubble is session-local, because the daemon persists no message for an item that has not run yet                                                                                        | `{ content, kind?, source_task_id? }` + `x-workspace-path` — a follow-up **does** carry the picker's project (unlike a steer): the user is queueing from here and now                                                                                                                                                                                                                                                                                                                                                | `201` `FollowupView { id, lane_key, kind, content, source_task_id, status, created_at, updated_at }`; `400 EMPTY_CONTENT` / `INVALID_KIND` / `INVALID_LANE_KEY`, `404 NOT_FOUND` (including a lane the local user does not own — R42)                            |
| Attachments (drag-drop)                                    | ✅ `POST /v1/files/upload` (multipart, 100 MB `DefaultBodyLimit`)                                                                                                                                                                                                                                              | multipart                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | `{ id, filename, mime_type, size_bytes, status }`                                                                                                                                                                                                                |
| Attachment count cap                                       | ✅ enforced server-side                                                                                                                                                                                                                                                                                        | —                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | `400 TOO_MANY_ATTACHMENTS` when `> upload.max_files_per_message`                                                                                                                                                                                                 |
| Inline artifact card (`MD connector-audit-findings.md v2`) | ✅ `artifact_written` → `WrittenArtifactCard`, linking into the Library                                                                                                                                                                                                                                        |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |                                                                                                                                                                                                                                                                  |

**Lane keys.** Format is `"{user_id}:{source_name}"`. `is_lane_owned_by` (chat_types.rs)
rejects any lane not prefixed `"{local_user_id}:"` with `403 FORBIDDEN`. The GUI's
default lane is `state.default_lane_key`; the client learns it from the `lane_key` in
the `POST /v1/chat` response. `GET /v1/me` (GAP-16, resolved `26b3eaf`) now also serves
`default_lane_key` directly for a caller that wants it before any chat history exists.

### 2.2 Work view (runs)

| UI element                                                                             | Endpoint / event                                                                                                                                                                                                                                                                             | Request                                       | Response                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| -------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Run list, `{{ activeCount }} active · {{ doneCount }} done`                            | ✅ `GET /v1/tasks?status=active\|<status>&created_by=&limit=`                                                                                                                                                                                                                                | —                                             | **array** of serialized `Task` + injected `outcome` + injected `cost_usd: f64` (GAP-08b, resolved `a827dcf`) + `subagent_count: i64` (R38); the `assigned_agents[]` array was deleted with **P8** — a run's agents are lanes on `GET /v1/tasks/{id}/timeline`, and the list row keeps only their count                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `Task` fields                                                                          | —                                                                                                                                                                                                                                                                                            | —                                             | `{ id, title, description?, status, priority, progress_current?, progress_total?, result_summary?, created_by, source_lane, created_at, updated_at, completed_at?, state_version, outcome_kind?, artifact_count, workspace_id?, source_task_id? }` (`state_json`/`outcome_json` are `#[serde(skip_serializing)]`; `source_task_id` is set only on a row `rerun` created, and names the run it was copied from) — list route only, adds `cost_usd: f64` (`0.0` for a task with no logged calls, never omitted) and `subagent_count: i64` (spans this run opened, from one grouped `SELECT task_id, COUNT(*) FROM subagent_span WHERE task_id IN (…) GROUP BY task_id`; `0` for a run that spawned none, never omitted — in-flight agents included, since a span opens at spawn); the detail route's `Task` (row below) has neither field |
| `TaskStatus` values                                                                    | —                                                                                                                                                                                                                                                                                            | —                                             | `queued` / `running` / `paused` / `completed` / `failed` / `cancelled` (parsed by `str::parse::<TaskStatus>()`; `status=active` is a special list mode → `repo.list_active`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| Run detail header (`{{ sel.title }}`, `started {{ sel.started }}`)                     | ✅ `GET /v1/tasks/{id}`                                                                                                                                                                                                                                                                      | —                                             | `{ task: Task, outcome?: ParsedOutcomeFields }` — the legacy `assignments` key is gone (**P8**)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `ParsedOutcomeFields`                                                                  | —                                                                                                                                                                                                                                                                                            | —                                             | `{ outcome_summary?, outcome_kind: string, artifact_count: i32, artifacts: Value[], no_artifact_reason? }`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `Pause` / `Resume` / `Cancel run`                                                      | ✅ `POST /v1/tasks/{id}/action`                                                                                                                                                                                                                                                              | `{ action: "cancel" \| "pause" \| "resume" }` | `{ task_id, status }`; `409` on illegal transition with a human message; `400 Unknown action` otherwise (the refusal lists `start` too — see the `Start now` row)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `Start now` (queued run)                                                               | ✅ **GAP-06, closed** — `POST /v1/tasks/{id}/action`, answered before `apply_task_action` (it is a dispatch, not a transition)                                                                                                                                                               | `{ action: "start" }`                         | `200 { task_id, status }` — `task_id` is the id you sent (**D5**); `409 TASK_NOT_STARTABLE` (the run has already finished — re-run it, R43), `409 TASK_ALREADY_RUNNING`, `422 TASK_NOT_DISPATCHABLE` (the row has no description), `503 DISPATCH_FAILED`, `404 NOT_FOUND` (owner-scoped, R40)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `Re-run`                                                                               | ✅ **GAP-06, closed** — `POST /v1/tasks/{id}/rerun`, its own route because it answers a run you have not seen                                                                                                                                                                                | — (the goal comes from the stored row)        | `201 { task_id, source_task_id, title, status }` — `task_id` is **new**, `source_task_id` is the id you sent and is stored on the row; `409 TASK_NOT_TERMINAL`, `422 TASK_NOT_RERUNNABLE`, `503 DISPATCH_FAILED`, `404 NOT_FOUND`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Live status ticks                                                                      | ✅ WS `task_status`                                                                                                                                                                                                                                                                          | —                                             | `{ type:"task_status", task_id, title, status, progress_current, progress_total, result_summary, outcome_kind?, artifact_count?, outcome_summary?, ts, instance_id }` — `title` is filled at every producer site (GAP-07, resolved `298bad3`), no longer `""` on updates                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `{{ r.meta }}` = `11m 04s · 5/8 steps · $0.41`                                         | ✅ duration derivable (`created_at`→`updated_at`/`completed_at`), steps = `progress_current`/`progress_total`, cost = list row's `cost_usd` (GAP-08b, resolved `a827dcf`)                                                                                                                    |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `Parallel work` swimlanes (`lead`, `explore·1`, `code·2`, `review·3` with start/end %) | ✅ **GAP-09, closed** — `GET /v1/tasks/{id}/timeline` serves a lane per `subagent_span` row, `started_at`/`ended_at` against the run's own window                                                                                                                                            |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Lane detail strings (`12 files read`, `awaiting you`)                                  | ✅ **GAP-09, closed** — the lane's `detail` (`"waiting on <tool_name>"` for a derived `blocked` state; the completion detail otherwise)                                                                                                                                                      |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `Files · {{ r.fileCount }}` per run                                                    | ✅ `GET /v1/artifacts?task_id=` (the run cards still read the outcome blob)                                                                                                                                                                                                                  |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Per-run `Event log` (`{{ e.tag }} {{ e.text }} {{ e.at }}`)                            | ✅ **GAP-10, closed** — `GET /v1/events/history?task_id=` serves the run’s persisted rows, tool calls included; `tag` is a projection of `event_type`                                                                                                                                        |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `Steer` button                                                                         | ✅ **GAP-02, closed** — the button aims the composer (§4.4); the send is `POST /v1/tasks/{id}/steer`. Rendered **disabled with the reason** when the row's `steerable` (R40) is `false`                                                                                                      |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `Queue follow-up` button                                                               | ✅ **GAP-03, closed** — the button aims the composer (§4.4); the send is `POST /v1/lanes/{lane_key}/followups`. Deliberately **not** gated on `steerable`: a run that has stopped taking steering can still have work parked behind it                                                       |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Steering acknowledgement                                                               | ✅ WS `workflow_steered` `{ task_id, lane_key, ts, instance_id }`                                                                                                                                                                                                                            |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Progress narration                                                                     | ✅ WS `workflow_progress` `{ task_id, lane_key, message, ts, instance_id }`                                                                                                                                                                                                                  |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Follow-up queued badge                                                                 | ✅ WS `followup_queued` `{ lane_key, followup_id: i64, kind: "followup"\|"unprocessed_steering", ts, instance_id }`; the read-back list is `GET /v1/lanes/{lane_key}/followups` (**GAP-03, closed**) and WS `followup_cancelled` `{ lane_key, followup_id, ts, instance_id }` retires a chip |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |

### 2.3 Library view (artifacts)

The view is backed end to end by `GET /v1/artifacts` (list, detail, content,
versions, diff, pin — GAP-04/05/11/12, closed in Phase 3; see those sections
below for what shipped). The design's `ARTS[]` fixture shape is:

```ts
{ id, name, kind: "md"|"code"|"term"|"table"|"plan"|"image"|"html",
  badge, run, runName, agent, version: "v2 of 2", stamp, time, sub,
  versions: [{ v, note, by, when }] }
```

| UI element                                                            | Status                                                                                                                                                                                                                                                  |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Library · {{ artCount }} files` list                                 | ✅ `GET /v1/artifacts` — `total` is the unpaged count; `limit`/`offset` page it.                                                                                                                                                                        |
| Kind filter chips (`All/Docs/Code/Output/Data/Media/Plans`)           | ✅ client-side over the loaded page (two labels cover two artifact kinds; `?kind=` takes one)                                                                                                                                                           |
| `run` / `runName` / `agent` attribution                               | ✅ **GAP-04, closed** — `GET /v1/artifacts` and `/{id}` serve `task_id`, `task_title`, `agent_id` and `agent_template_id` (`routes/artifacts.rs`); `LibraryRow` renders the subtitle from them.                                                         |
| Preview tab                                                           | ✅ `GET /v1/artifacts/{id}/content` — `?token=` for `<img>`, header auth for text. HTML/SVG render as source (deferred review)                                                                                                                          |
| `Diff v1→v2` tab                                                      | ✅ `GET /v1/artifacts/{id}/diff?from=&to=`                                                                                                                                                                                                              |
| `History` tab (`versions[]`)                                          | ✅ `GET /v1/artifacts/{id}/versions`, with the stored `+a −r` per version                                                                                                                                                                               |
| `★ Pin`                                                               | ✅ `PUT /v1/artifacts/{id}/pin`                                                                                                                                                                                                                         |
| `Export`                                                              | ⚠️ client can fetch content and save via Tauri fs plugin (already a dependency) — no server route needed                                                                                                                                                |
| `Reveal` (in Finder)                                                  | ⚠️ `POST /v1/files/{id}/open` exists but _opens with the default app_ (`opener::open` on a staged copy in `$TMPDIR/openalpaca-open/{id}-{safe_name}`), it does not reveal — response `{ id, status: "opened" }`. Reveal should be a Tauri-side command. |
| `+41 −6` diff stats, `exit 0 · 1.4s`, `1440 × 900`, `3 rows` sublines | ⚠️ the diff counts are served per version; the per-kind sublines (exit code, dimensions, row count) are still unrecorded                                                                                                                                |

### 2.4 Settings view

| Section                                                                 | Endpoint(s)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | Notes                                                                                                                                                |
| ----------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Connection** — `Daemon connected`                                     | ✅ `GET /v1/health` → `{ status, version, pid, instance_id }`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |                                                                                                                                                      |
| `uptime 4d 02h`                                                         | ✅ **GAP-14, closed** — `GET /v1/status` → `started_at` + `uptime_secs`, stamped at the top of the daemon's `async_main`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `Instance 7f3a91c4`                                                     | ✅ `health.instance_id` / `ConnectionInfo.instanceId`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |                                                                                                                                                      |
| `Endpoint 127.0.0.1:51823`                                              | ✅ `ConnectionInfo.baseUrl`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |                                                                                                                                                      |
| `Schema v33`                                                            | ✅ **GAP-14, closed** — `GET /v1/status.schema_version`, read from the open database (`Database::schema_version()`), never a compile-time count of migration files                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `Reconnect`                                                             | ✅ client-side (re-invoke `ensure_daemon_running`, reopen WS)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |                                                                                                                                                      |
| `Copy log path`                                                         | ✅ **GAP-14, closed** — `GET /v1/status.log_path` = `state/logs/daemon.log` when the CLI wrote one, else `null` (the button is inert with the reason). The CLI rotates it at 16 MB, keeping three generations                                                                                                                                                                                                                                                                                                                                                                                                                     |
| **Storage** — `uploads` / `produced`                                    | ✅ `GET /v1/status.upload_bytes` / `.produced_bytes` — §4.8's two numbers, never one: agent output is informational and never charged against the upload cap                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| session-log sweep line                                                  | ✅ `GET /v1/status.sessions.last_sweep` (§5.4's boot pass — bytes freed, sessions evicted, `over_cap_after`) and `.dropped_records`. Not in the design: the caps are invisible until they bite, so the panel says what the last pass did                                                                                                                                                                                                                                                                                                                                                                                          |
| `Today: {{ spend }} of $5.00 cap · 15 runs · 41k tokens`                | ✅ **GAP-08c, closed (T50)** — spend and tokens from `GET /v1/usage/summary?window=today`; run count from `GET /v1/tasks?limit=…` filtered client-side **on the summary's UTC `date`**, so all three are one day. There is no `of $5.00 cap` denominator to draw: spend is not capped daily by design (N4), and the card names the two caps that are — per workflow and per agent turn                                                                                                                                                                                                                                            |
| **Models & keys** — provider rows                                       | ✅ `GET /v1/settings/llm` (full `LlmConfig`), `GET /v1/settings/llm/status` (`key_health()`), `GET /v1/settings/llm/providers/usage` → `ProviderUsageSummary[] { provider, total_cost_usd, total_tokens, total_requests, health, external_usage? }`                                                                                                                                                                                                                                                                                                                                                                               | ⚠️ `health` is hardcoded `"healthy"` in `get_provider_usage`                                                                                         |
| model chips per provider                                                | ✅ `GET /v1/models`, `POST /v1/models/refresh`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |                                                                                                                                                      |
| `key added 12 Jul`                                                      | ⚠️ present inside the `GET /v1/settings/llm` config payload; verify the field survives redaction before relying on it                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `41k tok today` per provider                                            | ✅ **GAP-08c, closed (T50)** — `GET /v1/usage/summary.by_provider[] { provider, usd, calls, tokens }`, grouped out of that UTC day's `llm_call_log` rows. `ProviderUsageSummary.total_tokens` on `/v1/settings/llm/providers/usage` is still lifetime and is no longer shown as "today"                                                                                                                                                                                                                                                                                                                                           |
| `Add provider` / key CRUD                                               | ✅ `PUT /v1/settings/llm` (upsert), `DELETE /v1/settings/llm/keys/{provider}/{key_id}`, `PUT .../keys/reorder`, `PUT .../keys/priority`, `POST .../validate`, `GET .../credentials`, `POST .../credentials/rescan`, `GET .../cli-backends`                                                                                                                                                                                                                                                                                                                                                                                        |                                                                                                                                                      |
| provider on/off toggle                                                  | ✅ `PUT /v1/settings/llm/providers/{provider}/enabled` `{ enabled }` → `200 { id, enabled, loaded, warning }`; `404 PROVIDER_NOT_FOUND`, `409 PROVIDER_IS_DEFAULT` when it serves the default model, `409 DEFAULT_MODEL_UNRESOLVED` when the default model places nowhere. Writes `llm.toml` through the one atomic writer, then unloads or reloads the provider live                                                                                                                                                                                                                                                             |
| **Connectors** rows                                                     | ✅ **GAP-17 detail half, closed (T49)** — `GET /v1/connectors` → `[{ id, name, status, configured, source, registered, messages_7d }]`. `name` is the connector's own (`ConnectorFactory::display_name`), `source` is the attribution token `messages_7d` was grouped by, `registered` is whether the manager holds a spawned handle                                                                                                                                                                                                                                                                                              | MCP servers and plugin-declared connectors still do not appear — a plugin that declares one and never registers it is the client-side `unwired` join |
| toggle / delete                                                         | ✅ `POST /v1/connectors/{id}/action` `{ action: "enable"\|"disable"\|"delete" }`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |                                                                                                                                                      |
| config / settings                                                       | ✅ `POST /v1/connectors/{id}/config` `{ token }`; `GET\|PUT /v1/connectors/{id}/settings` (`{ settings: Record<string,string> }`, keys must be `"{id}."`-prefixed and present in `config_schema::CONFIG_KEYS`)                                                                                                                                                                                                                                                                                                                                                                                                                    |                                                                                                                                                      |
| `184 calls 7d`                                                          | ✅ **GAP-17 counts half, closed (T49)** — `messages_7d`, one grouped query over `conversation_messages.source` for the last seven UTC days. The panel renders `184 messages · 7d`, and `No messages · 7d` for a quiet week                                                                                                                                                                                                                                                                                                                                                                                                        | messages attributed to the connector, not tool calls — the daemon counts no per-connector "calls" anywhere                                           |
| `unwired` tag                                                           | ✅ client-side join — an extension row's `connector` that appears in no `GET /v1/connectors` row (never a daemon gap)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |                                                                                                                                                      |
| `Connect service`                                                       | ❌ **GAP-17** (narrowed to this)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |                                                                                                                                                      |
| live status                                                             | ✅ WS `connector_status` `{ id, status, ts, instance_id }`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |                                                                                                                                                      |
| **Tools** rows (`shell_execute`, `file_edit`, `github__create_issue`…)  | ✅ `GET /v1/tools` → `[{ name, description, source, origin, provides_capabilities, requires_confirmation, invocations_today, version, author }]` (ADR-030 §8). The design calls the section "Skills"; its rows are **tools**                                                                                                                                                                                                                                                                                                                                                                                                      |
| skill health                                                            | ✅ `GET /v1/skills/health` → `SkillHealthMetrics[] { skill_id, total_invocations, clean_success_rate, clean_success_rate_7d, repair_rate, repair_effectiveness, degraded_rate, avg_duration_ms, avg_cost_usd, avg_rounds, last_invoked_at?, user_satisfaction_rate?, feedback_count, feedback_coverage }`                                                                                                                                                                                                                                                                                                                         | lifetime metrics; the name comes from the catalog row beside it                                                                                      |
| skill names / descriptions / triggers                                   | ✅ **GAP-18, closed** — `GET /v1/skills` → `[{ id, name, description, source, origin, requires_capabilities, triggers: { slash, keywords }, schedule, invocations_today, version, author }]`. The health rows join it on the id and show the name; a row the catalog no longer holds keeps its id                                                                                                                                                                                                                                                                                                                                 |                                                                                                                                                      |
| `asks` (requires confirmation) badge                                    | ✅ `requires_confirmation` on the tool row                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |                                                                                                                                                      |
| `9 uses today`                                                          | ✅ `invocations_today` on both catalog rows — `tool_execution_log` / `skill_execution_log` counted from today's local midnight converted to UTC. `SkillHealthMetrics.total_invocations` stays lifetime                                                                                                                                                                                                                                                                                                                                                                                                                            |                                                                                                                                                      |
| skill lifecycle events                                                  | ✅ WS `skill_catalog_updated`, `skill_invocation_started` `{ request_id, skill_id, query_preview }`, `skill_completed` `{ request_id, skill_id, duration_ms, output_preview }`, `skill_failed` `{ request_id, skill_id, error }`                                                                                                                                                                                                                                                                                                                                                                                                  |                                                                                                                                                      |
| **Extensions** rows (MCP servers + plugins)                             | ✅ `GET /v1/extensions?include_orphaned=true` → the 23-field row of ADR-030 §8 (`kind, id, version, transport, enabled, consent, state, reason, actionable, detail, hint, missing_config_keys, added_capabilities, tools, skipped_tools, withdrawn_by_server, tools_changed_at, declared, skills, agents, connector, provider, since`)                                                                                                                                                                                                                                                                                            |                                                                                                                                                      |
| enable/disable/reload/approve/deny/config/remove                        | ✅ `POST /v1/extensions/{kind}/{id}/{enable\|disable\|reload\|approve\|deny}`; `GET\|POST /v1/extensions/{kind}/{id}/config`; `DELETE /v1/extensions/plugin/{id}` (orphans only — the uninstall is the same path with `?uninstall=true`). `/v1/plugins*` was deleted in C7                                                                                                                                                                                                                                                                                                                                                        |                                                                                                                                                      |
| `Add extension` (install / uninstall)                                   | ✅ **GAP-24, closed** — `POST /v1/extensions/plugin` `{ source: "path", path }` copies a directory in and answers `201 { extension, manifest }`; `POST /v1/extensions/mcp` `{ name, transport, … }` writes a `[servers.<name>]` block; `POST /v1/extensions/plugin/validate` `{ path }` is the dry run; `PUT /v1/extensions/plugin/{id}` replaces a tree; `DELETE /v1/extensions/{kind}/{id}?uninstall=true[&keep_data=false]` is the real removal. An install grants nothing (`unapproved`/`never_seen`; `approve` starts it) and an uninstall deletes nothing (moved to `plugins/.trash/`)                                      |                                                                                                                                                      |
| `Declares 2 connectors, 0 registered` warn tag                          | ⚠️ derivable: `extensions[].connector` non-null vs. absent from `GET /v1/connectors` — client-side join                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| extension lifecycle                                                     | ✅ WS `extension_state_changed` `{ kind, id, state, generation, tools_changed, ts, instance_id }`, `extension_capability_withheld`, `extension_capability_withdrawn` — the six `plugin_*` variants were deleted with `/v1/plugins*` in C7, and this family carries `ts`/`instance_id` on every frame (former GAP-22)                                                                                                                                                                                                                                                                                                              |                                                                                                                                                      |
| **Agents** rows (templates)                                             | ✅ `GET /v1/agent-templates?window=7d\|30d\|all` → `TemplateResponse[] { id, name, description, icon?, singleton, capabilities[], denied_capabilities[], temperature, verbosity, model?, fallback_models[], max_tool_calls?, timeout_seconds?, max_cost_per_task?, require_confirmation_for[], persona, body, run_count: i64, last_run_at?, window: string }`                                                                                                                                                                                                                                                                     | plus `GET\|PUT\|DELETE /v1/agent-templates/{id}`, `POST /v1/agent-templates`                                                                         |
| running instances                                                       | ✅ `GET /v1/agent-instances` → `InstanceResponse[] { id, template_id, name, status, current_task? }`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |                                                                                                                                                      |
| agent CRUD/config/action                                                | ✅ `GET\|POST /v1/agents`, `POST /v1/agents/from-toml`, `GET\|DELETE /v1/agents/{id}`, `GET\|PUT /v1/agents/{id}/config` (`{ config: AgentConfigFile, config_version: u64 }` — optimistic lock), `POST /v1/agents/{id}/action` `{ action: "pause"\|"resume" }`                                                                                                                                                                                                                                                                                                                                                                    |                                                                                                                                                      |
| `12 runs 7d` per template                                               | ✅ **GAP-20 counts half, closed (T48)** — `run_count` / `last_run_at` / `window` on each `TemplateResponse`, from one grouped `SELECT template_id, COUNT(*), MAX(started_at) FROM subagent_span WHERE state != 'running' [AND started_at >= ?] GROUP BY template_id`. `?window=7d\|30d\|all` (default `7d`) picks the cutoff; an unrecognised value is `400 UNKNOWN_WINDOW`. The count is _completed_ runs only within that window — a run still in flight is not one yet — and the row carries the window it was computed over, so the card reads `12 runs · 7d` exactly. (`AgentMetrics` remains instance-keyed and unrelated.) |
| template on/off toggle                                                  | ❌ **GAP-20**, remaining half — no `enabled` field, and nothing would enforce one where subagents are spawned (P-31)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |                                                                                                                                                      |
| **Conversations** rows                                                  | ✅ `GET /v1/sessions?workspace_id&source&status&q&limit&offset` → `{ sessions: SessionView[], total: i64 }` (P19 deleted `/v1/conversations`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |                                                                                                                                                      | `SessionView { id, lane_key, source, title, workspace_id?, status: "active"\|"archived", message_count: i64, last_message_at?, created_at, updated_at, ended_at?, active_task_count: i64, interrupted_task_count: i64 }` — covers `142 messages`, `29 Aug`. The summary columns are the compactor's bookkeeping and are deliberately **not** served; `active_task_count` is lane-scoped, and `interrupted_task_count` counts the runs §5.6b's boot sweep marked `interrupted` |
| messages of one conversation                                            | ✅ `GET /v1/sessions/{id}/messages?limit&offset&before_id` → `{ messages: ConversationMessage[], total: i64 }` — the same rows as `/v1/chat/history`, `task_id` and `artifacts` included (GAP-23); `before_id` is the keyset cursor for "load older"                                                                                                                                                                                                                                                                                                                                                                              |                                                                                                                                                      |                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| delete / rename a conversation                                          | ✅ **GAP-21, closed** — `PATCH /v1/sessions/{id} { title? }` renames; `DELETE /v1/sessions/{id}` removes one transactionally (`204`, or `409 SESSION_HAS_ACTIVE_WORKFLOWS` while a run **this** conversation started is in flight). The controls live on the chat view's conversation sidebar, not here; this list stays a read-only inventory across every lane                                                                                                                                                                                                                                                                  |                                                                                                                                                      |                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| **Event log** section                                                   | ✅ `GET /v1/events/history?task_id&agent_id&event_type&before&limit` → `{ events: EventLog[] { id, timestamp, agent_id?, task_id?, event_type, detail?: Value, result?: Value }, next_before }`                                                                                                                                                                                                                                                                                                                                                                                                                                   | ⚠️ the WS stream still carries more event variants than the daemon persists (**GAP-10** closed the run scoping, not the breadth)                     |
| **Orchestrator observability** (not in the design; available if wanted) | ✅ `GET /v1/orchestrator/latency?mode&from&to&limit` → `{ records: [...] }`; `GET /v1/orchestrator/latency/aggregate?from&to` → `{ aggregates: [...] }`; `GET /v1/orchestrator/decisions?mode&from&to&limit` → `{ records: [...] }`                                                                                                                                                                                                                                                                                                                                                                                               | `mode` values are historical strings (`two_phase_*`, `planner_*`, `fast_path`, `no_llm`) plus current Routing V2 modes                               |

### 2.5 Command palette (⌘K)

Every palette command maps onto an existing action already covered above
(`New background job` → composer, `Steer …` → `POST /v1/tasks/{id}/steer`
(GAP-02, closed), `Approve pending shell_execute` → `POST /v1/chat/confirmations/{id}`,
`Go …` → client routing, `Find <filename>` → `GET /v1/artifacts?q=`,
`Toggle compact density` → client state).
No new API is needed for the palette _shell_, and the `Find` entries now query the artifact list.

### 2.6 Client-only state (no API, keep in `localStorage`)

The design already persists these itself and the React port should too — none of them
belong on the daemon:

- `oa-pane-widths` → `{ workW: 396, workListW: 340, libListW: 326 }` (drag ranges
  `workW` 300–600 reversed, `workListW` 260–480, `libListW` 260–480).
- `view`, `secId`, `libKind`, `artTab`/`panelTab`, `dense` (compact density),
  `workOpen`, `panelArt`.
- Toast queue (2600 ms auto-dismiss).
- Keyboard map: `⌘K` palette, `Esc` (palette → picker → side panel → deny pending
  confirmation), `Enter` approves a pending confirmation when the composer is not
  focused.

---

## 3. GAP LIST

Ordered by how hard they block the design. Each entry states **what the UI needs**,
**why nothing fits** (with the file checked), and **a concrete proposal**.

**Outcomes.** Every heading below now carries one: the task that closed the gap, or
`OPEN`. The sketches underneath are kept as written — they are the argument the
work was done from, not a description of what shipped, and where the shipped
shape differs the heading's blockquote says so. Two gaps are still open, **GAP-17**
(no route adds a connector) and **GAP-20** (a template has no `enabled` flag);
those two, and only those two, are the entries in
`apps/openalpaca-gui/src/lib/unavailable.ts`, which is what the UI's "not yet
available" notes are generated from. If this section and that registry ever
disagree, the registry is right — it is compiled.

---

### GAP-01 — "Always allow" cannot be expressed _(trivial fix, high value)_ — **CLOSED (Phase 0, T7)**

Landed `88e8a3b` (Phase 0): `ConfirmationBody` gained `#[serde(default)] approval_scope:
Option<ApprovalScope>`, forwarded verbatim into `ConfirmationResponse` instead of the
hardcoded `None` below. The proposal below was implemented as written.

**UI needs:** the third confirmation button, `Always allow`, which adds
`shell_execute` to an allowlist so it stops asking.

**Why nothing fits:** the core type already supports it —
`crates/openalpaca_core/src/security/confirmation.rs`:

```rust
pub struct ConfirmationResponse { pub approved: bool, pub approval_scope: Option<ApprovalScope> }
pub enum ApprovalScope { TheseArgs, EntireTool }   // serde: "these_args" | "entire_tool"
```

and `ApprovalCache::record(tool_name, args_hash, scope)` honours both. But the HTTP
route throws it away — `apps/openalpacad/src/routes/chat.rs`:

```rust
#[derive(Deserialize)] pub struct ConfirmationBody { pub approved: bool }
// ...
broker.respond(&request_id, ConfirmationResponse { approved: body.approved, approval_scope: None })
```

**Proposal — extend the existing route (backward compatible):**

```
POST /v1/chat/confirmations/{request_id}
{ "approved": true, "approval_scope": "entire_tool" }   // optional; "these_args" | "entire_tool"
→ 200 {}
```

Change `ConfirmationBody` to `{ approved: bool, #[serde(default)] approval_scope: Option<ApprovalScope> }`
and pass it through. One-line-ish change.

---

### GAP-02 — No first-class steering endpoint — **CLOSED (Phase 5 item 1, T35)**

> **Closed in Phase 5.** `POST /v1/tasks/{id}/steer` ships — one more producer on the existing `SteeringInbox` rail (`push_steering`), not a second mechanism: no new inbox, no new cap, no new event. It is addressed at a _run_, and the lane it stamps comes from that row's `source_lane`, never from the request. `200 { task_id, accepted, inbox_depth, lane_key }`; `409 STEERING_INBOX_FULL` / `409 TASK_NOT_STEERABLE`, `503 STEERING_DISABLED`, `400 EMPTY_MESSAGE`, `404 NOT_FOUND` — `accepted` is not a promise the workflow _read_ the message (the rail drains at the next round boundary), so `inbox_depth` is the receipt. The route is owner-scoped, and the only task route that is, so every task row now carries `steerable: bool` (R40 — the local user's run, not terminal, live inbox) and the GUI disables `Steer` with a stated reason instead of letting the user discover the answer by sending. The `grep … router.rs` line below is the state at filing; `router.rs:70-75` matches it now.

**UI needs:** a `Steer` button on a running run (Work view, run card, and the chat
composer's `steering → connector audit` mode) that injects a message into _that_
workflow and gets a deterministic accepted/rejected answer.

**Why nothing fits:** steering is reachable **only** through the chat text channel.
`crates/openalpaca_core/src/orchestrator/handlers.rs:163` strips the literal
`"/steer "` prefix; `orchestrator/task_ops.rs:355` handles it, and it targets the
_lane's_ active workflow — it takes no explicit `task_id`. The alternative path is the
model's own `steer_workflow` tool (`tools/builtins/steer_workflow.rs`), i.e. a
non-deterministic LLM decision. `grep -rn "steer" apps/openalpacad/src/router.rs`
returns nothing. Also: `steering_enabled` (default on) and `steering_inbox_cap` are
config-only; the UI cannot discover whether steering is even available, or whether the
inbox is full.

**Proposal:**

```
POST /v1/tasks/{id}/steer
{ "message": "Also check the MCP resource stubs while you're in there." }
→ 200 { "task_id": "b41c8e02", "accepted": true, "queued_position": 1, "inbox_depth": 1 }
→ 409 { "error": { "code": "STEERING_INBOX_FULL", "message": "...", "cap": 8 } }
→ 409 { "error": { "code": "TASK_NOT_STEERABLE", "message": "task is 'completed'" } }
→ 503 { "error": { "code": "STEERING_DISABLED", "message": "steering_enabled = false" } }
```

Emits the existing `ServerEvent::WorkflowSteered`. This is a thin wrapper over the
`SteeringInbox` registered per running lead-agent task in `runner/steering.rs`.

---

### GAP-03 — No follow-up queue API (write or read) — **CLOSED (Phase 5 item 2, T36)**

> **Closed in Phase 5.** All three verbs ship on the existing `lane_followups` queue — no new table, no second mechanism: `GET /v1/lanes/{lane_key}/followups` (bare array of the lane's **queued** rows, oldest first, both kinds — terminal rows are history and live in the event log), `POST` (`{ content, kind?, source_task_id? }` → `201` with the stored row) and `DELETE /v1/lanes/{lane_key}/followups/{id}` (`200 { id, status:"cancelled" }`). The wire shape drops `principal_json` and `workspace_path`; the principal is stamped as the local user and the workspace comes from `x-workspace-path`, never from the body. Cancel is a CAS on `status = 'queued'` against the autostart's own `claim_next`, so a cancel that lost is `409 FOLLOWUP_NOT_QUEUED` rather than a lie — the turn it tried to stop is running. Other codes: `400 EMPTY_CONTENT` / `INVALID_KIND` (a client cannot mint `unprocessed_steering`; `claim_next` never claims one) / `INVALID_LANE_KEY`, `404 NOT_FOUND`. **`POST` and `DELETE` are owner-scoped** (R42) — a lane whose `user_id` is not the local user's is `404`, because a follow-up is later _run as a turn_ on the lane it names; `GET` is not, per the read/list line. The event is the specific `followup_cancelled` the requirement named, not the generic `followup_status_changed` floated below, and it is published only when the CAS won; `followup_queued` is reused unchanged. The `grep … routes/` line below is the state at filing — it matches `routes/followups.rs` and `router.rs:81,85,89` now.

**UI needs:** the `Queue follow-up` button on every run card, the composer's
`follow-up → connector audit` mode, and a visible pending-follow-ups list.

**Why nothing fits:** follow-ups exist in storage — migration `033_lane_followups.sql`,
`FollowupRepository { queue, ... }`, model
`FollowupRecord { id: i64, lane_key, kind: "followup"|"unprocessed_steering", content, principal_json, workspace_path?, source_task_id?, status: "queued"|"running"|"done"|"cancelled", created_at, updated_at }`
— and `ServerEvent::FollowupQueued` is emitted. But
`grep -rn "followup" apps/openalpacad/src/routes/ apps/openalpacad/src/router.rs`
returns **zero** matches. The only writer is the model's `queue_followup` tool.

**Proposal:**

```
POST /v1/lanes/{lane_key}/followups
{ "content": "...", "source_task_id": "b41c8e02" }        // source_task_id optional
→ 201 { "followup_id": 42, "lane_key": "...", "status": "queued" }

GET /v1/lanes/{lane_key}/followups?status=queued&limit=50
→ 200 { "followups": FollowupRecord[] }                   // principal_json redacted

DELETE /v1/lanes/{lane_key}/followups/{followup_id}
→ 200 { "followup_id": 42, "status": "cancelled" }
```

Also add a `followup_status_changed` ServerEvent
(`{ type, lane_key, followup_id, status, ts, instance_id }`) so the UI can retire a
pending chip when `followup_autostart` fires it.

---

### GAP-04 — No artifact/output API _(the Library view is entirely unbacked)_ — **CLOSED (Phase 3: routes T27, Library T31)**

> **Closed in Phase 3.** `GET /v1/artifacts` (list, `?task_id=&kind=&origin=&project_root=&pinned=&q=&include_missing=&limit=&offset=` → `{artifacts,total}`), `GET /v1/artifacts/{id}` and the `artifact_written` event all ship. The Library, the run's Output card, the chat inline card and the palette's `Find` rows read them.

**UI needs:** `GET`-able list of everything agents produced, filterable by kind, each
row carrying `{ name, kind, run/task_id, runName, agent, created_at, size/summary }`;
plus per-run `Files · N` sections and the inline artifact cards in chat.

**Why nothing fits:**

- `router.rs` has no `/v1/artifacts` or `/v1/files` **list** route — only
  `POST /v1/files/upload`, `GET /v1/files/{id}`, `GET /v1/files/{id}/content`,
  `POST /v1/files/{id}/open`.
- `FileAssetRepository` (`repository/file_asset/mod.rs`) offers only `get_by_id`,
  `list_orphaned(older_than_hours)`, `list_by_status(status, limit)` — nothing
  user-facing.
- `FileAsset` has **no `task_id` and no `agent_id`** (`models/file_asset.rs`), so
  even a list route could not attribute a file to a run.
- The only artifact-shaped data the daemon serves is
  `ParsedOutcomeFields.artifacts: Vec<serde_json::Value>` — free-form JSON blobs
  parsed out of `task.outcome_json`, whose observed shape in tests is
  `{ key, label, agent_id, step_order }`. That is a _reference_, not a retrievable
  file, and it is schema-less.

**Proposal (needs a storage change: add `task_id` + `agent_id` + `kind` columns to
`file_assets`, or a new `artifacts` table):**

```
GET /v1/artifacts?task_id=&kind=&pinned=&limit=&offset=
→ 200 { "artifacts": Artifact[], "total": 1234 }

GET /v1/artifacts/{id}          → 200 Artifact
GET /v1/artifacts/{id}/content  → bytes (see GAP-11 for auth)

Artifact {
  id: string, name: string,
  kind: "markdown"|"code"|"terminal"|"table"|"plan"|"image"|"html"|"binary",
  mime_type: string, size_bytes: i64,
  task_id: string|null, task_title: string|null,
  agent_id: string|null, agent_template_id: string|null,
  version: i32, version_count: i32,          // see GAP-05
  pinned: boolean,                            // see GAP-12
  summary: string|null,                       // "+41 −6" / "exit 0 · 1.4s" / "3 rows"
  metadata: object|null,                      // per-kind: {added,removed} | {exit_code,duration_ms} | {width,height} | {rows}
  created_at: string, updated_at: string
}
```

Plus a `ServerEvent::ArtifactWritten { artifact_id, task_id, agent_id, name, kind, version, path, ts, instance_id }`
so the chat transcript can render the card the moment it lands (the design's
`artifact` event-log tag and the `connector-audit-findings.md v2 written` line).

---

### GAP-05 — No artifact versioning, history, or diff — **CLOSED (Phase 3 item 3, T29)**

> **Closed in Phase 3.** `GET /v1/artifacts/{id}/versions` and `…/diff?from=&to=` ship, over migration 036's `artifact_versions` and a `similar` unified patch. The History and Diff tabs render them; a non-text or oversized pair is a 409 the tab names (`NOT_DIFFABLE`, `DIFF_TOO_LARGE`).

**UI needs:** `v2 of 2` stamps, the `History` tab (`versions[] = { v, note, by, when }`),
and the `Diff v1→v2` tab with `+9 −2` counts and a rendered unified diff.

**Why nothing fits:** nothing versioned exists anywhere. `file_assets` has a single
row per file with `created_at`/`updated_at`; there is no history table
(`ls crates/openalpaca_storage/src/migrations/` — 034 migrations, none about file or
artifact versions), and `FileAssetRepository` has no revision methods. `state_version`
on `Task` is an optimistic-lock counter, not content history.

**Proposal (extends GAP-04):**

```
GET /v1/artifacts/{id}/versions
→ 200 { "versions": [
    { "version": 2, "note": "Added MCP resource stub finding after steer",
      "author_agent_id": "review_agent", "created_at": "...",
      "size_bytes": 4120, "added_lines": 9, "removed_lines": 2 },
    { "version": 1, "note": "Initial two findings and suggested fix",
      "author_agent_id": "review_agent", "created_at": "...",
      "size_bytes": 3760, "added_lines": null, "removed_lines": null }
  ] }

GET /v1/artifacts/{id}/versions/{n}/content   → bytes
GET /v1/artifacts/{id}/diff?from=1&to=2
→ 200 { "from": 1, "to": 2, "added_lines": 9, "removed_lines": 2,
        "format": "unified", "patch": "@@ -3,4 +3,6 @@\n..." }
```

Storage: an `artifact_versions` table (`artifact_id, version, storage_path, note,
author_agent_id, size_bytes, added_lines, removed_lines, created_at`) with the
artifact row pointing at the head version.

---

### GAP-06 — Task actions are missing `rerun` and `start` — **CLOSED (Phase 5 item 3, T37)**

> **Closed in Phase 5.** Both verbs ship, and not in the single shape proposed
> below — the asymmetry the proposal noticed turned out to run deeper than the
> status code.
>
> - `POST /v1/tasks/{id}/rerun` → `201 { task_id, source_task_id, title, status }`.
>   The `task_id` is a run the caller has not seen; `source_task_id` is the id it
>   asked about, and it is a **stored column** (`task.source_task_id`, migration
>   037), so the link outlives the response. Refusals: `409 TASK_NOT_TERMINAL`
>   (steer or cancel a live run instead of re-running it), `422
TASK_NOT_RERUNNABLE` (the row has no description to re-dispatch),
>   `503 DISPATCH_FAILED` (no lead-agent template free), `404 NOT_FOUND`.
> - `POST /v1/tasks/{id}/action { "action": "start" }` →
>   `200 { task_id, status }`, with `task_id` the id that was sent (**D5**). The
>   route answers it before `apply_task_action`, which knows three _transitions_
>   and would call this an unknown action; that refusal now lists `start`.
>   Refusals: `409 TASK_NOT_STARTABLE`, `409 TASK_ALREADY_RUNNING`,
>   `422 TASK_NOT_DISPATCHABLE`, `503 DISPATCH_FAILED`, `404 NOT_FOUND`.
>   - `409 TASK_NOT_STARTABLE` is **R43**: the run has already finished, and
>     `start` re-launches the row _in place_, so it would clear the summary,
>     outcome and artifact count that run produced. `rerun` is the verb that
>     keeps them.
>   - `409 TASK_ALREADY_RUNNING` is a compare-and-set on the run slot, not a
>     look — two simultaneous starts must not put two lead agents on one id.
>   - `422 TASK_NOT_DISPATCHABLE` is the row carrying no description. **R44**
>     renamed it from `TASK_NOT_STARTABLE`, which now means the `409` above: one
>     code word cannot carry two conditions at two statuses.
>
> `rerun` gets its own route rather than a fourth action word because the two
> answer different things: a re-run is a _second_ run, and the finished one keeps
> its row and its result — which is what the user is re-running against.
>
> Both are **owner-scoped** (R40), like `POST /v1/tasks/{id}/steer`: a verb that
> puts work into the daemon as this user refuses a run this caller did not start,
> with the same `404` a missing one gets. Reading and cancelling a run stay
> unscoped.
>
> The prose below stays as the record of why the gap was filed. One sentence in
> it is now false: the `Unknown action` message lists four verbs, and
> `POST /v1/tasks/{id}/rerun` is a re-dispatch entry point on the HTTP surface.

**UI needs:** `Re-run` on every terminal (done/cancelled) run, and `Start now` on a
queued run.

**Why nothing fits:** `apply_task_action` (via `routes/tasks.rs`) accepts exactly three
verbs; the error path spells it out: `"Unknown action: '{}'. Valid: cancel, pause, resume"`.
`TaskActionError` has only `CannotCancel`/`CannotPause`/`CannotResume`/`NotFound`/
`UnknownAction`/`Db`. There is no re-dispatch entry point on the HTTP surface at all —
`POST /v1/tasks` only _creates a DB row + lane_; it never dispatches a workflow
(re-read `create_task_handler`: it persists, registers, creates a lane, publishes
`TaskCreated`, and returns — no dispatcher call).

**Proposal:**

```
POST /v1/tasks/{id}/action
{ "action": "rerun" }
→ 201 { "task_id": "<new uuid>", "status": "queued", "source_task_id": "b41c8e02" }
   // clones title/description/source_lane into a fresh task and dispatches it

{ "action": "start" }
→ 200 { "task_id": "3ac55f19", "status": "running" }   // promotes a queued task now
→ 409 { "error": "Can only start a queued task, current state: 'running'" }
```

Note the deliberate asymmetry: `rerun` returns a **new** `task_id` (201), which the UI
must follow; `start` mutates in place (200).

_As built:_ the asymmetry decided the routing too — `rerun` is its own route, and
`start` keeps `/action`. `start`'s `409` is `TASK_ALREADY_RUNNING` (the id has a
live run) rather than the proposal's "can only start a queued task": a queued row
is not the only one without a run behind it, and the run slot is what the daemon
can actually check.

---

### GAP-07 — `task_status` drops `title` and `agent_status` drops `name` — **CLOSED (Phase 0, T8)**

Landed `298bad3` (Phase 0): `title`/`name` are now filled at all ten producer sites
(`task_ops.rs`, `dispatcher/lead_agent.rs`, `dispatcher/outcome.rs`, `dispatcher/mod.rs`,
`runner/lead_agent/tools.rs`+`guard.rs`, `routes/agents.rs`) and threaded through
`event_bridge.rs` into the wire fields below — no new endpoint, exactly the bridge fix
the proposal called for.

**UI needs:** the Work list and the chat delegation card render `{{ r.title }}` on
every status change — including runs the client has never fetched (a scheduled skill
firing, a follow-up auto-starting).

**Why it is a gap:** `ServerEvent::TaskStatus` _has_ a `title: String` field, but
`apps/openalpacad/src/event_bridge.rs` passes `""` for every case except `TaskCreated`:

```rust
SystemEvent::TaskUpdated  { .. } => eb.task_status(&task_id, "", &status, ...)
SystemEvent::TaskCompleted{ .. } => eb.task_status(&task_id, "", "completed", ...)
SystemEvent::TaskFailed   { .. } => eb.task_status(&task_id, "", "failed", ...)
```

Same for agents: `SystemEvent::AgentStatusChanged` → `eb.agent_status(&agent_id, "", ...)`,
so `ServerEvent::AgentStatus.name` is always `""`.

**Impact:** the client must issue a `GET /v1/tasks/{id}` per unknown task id just to
learn a title it should have received. With several runs in flight that is an N+1
storm on every status tick.

**Proposal:** carry the title through. `SystemEvent::TaskUpdated`/`TaskCompleted`/
`TaskFailed` should include `title` (or the bridge should resolve it from the
`task_registry`, which already stores `(task_id, title)` — see
`task_registry.register(task_id, title)` in `create_task_handler`). Likewise resolve
the agent display name from the agent registry. **No new endpoint needed — a bridge
fix.**

---

### GAP-08 — Cost is not served anywhere the UI can use it — **CLOSED in two parts (Phase 0, T7; the rollup and the cap continued as GAP-08c, T50)**

**UI needs:** `$0.41` per run, `{{ spend }} today` in the chat footer and Work rail,
`$0.0184 of $5.00 cap` in Settings → Connection, and `41k tok today` per provider.

**Why nothing fits — three separate holes, two now closed:**

1. **Per-task cost — RESOLVED (`a827dcf`, Phase 0, GAP-08b).**
   `LlmUsageRepository::get_task_usage(task_id, limit)` **existed**
   (`repository/llm_usage/mod.rs:95`) and `llm_call_log` has a `task_id` column;
   `LlmUsageQuery` now takes `task_id` (checked first in `get_llm_usage`, ahead of
   `agent_id`/`key_id`), and `GET /v1/tasks` additionally carries a per-row `cost_usd:
f64` from one grouped `cost_for_tasks` query per page (`0.0` for a task with no
   logged calls, never omitted). The single-task detail route (`GET /v1/tasks/{id}`)
   does **not** carry it yet — that unification is a later phase; the GUI's `RunDetail`
   falls back to the list row's figure in the meantime (`components/work/run-model.ts`).
2. **Today's spend — RESOLVED (`7dbb988`, Phase 0, GAP-08a).**
   `GET /v1/orchestrator/config.daily_cost_usd` now sums today's UTC `llm_usage_daily`
   rows via `query_daily_usage`, replacing the hardcoded `0.0` below.
3. **The cap and a real summary rollup — RESOLVED (Phase 8 item 7, GAP-08c).**
   `GET /v1/usage/summary?window=today`. See below.

**Proposal (implemented for 1 and 2):**

```
# 1. one new query param on an existing route — IMPLEMENTED
GET /v1/llm/usage?task_id=b41c8e02&limit=200   → LlmCallLog[]

# 2. fix daily_cost_usd (no API change) — IMPLEMENTED
```

---

### GAP-08c — No usage-summary rollup or served cost cap — **CLOSED (Phase 8 item 7, T50)**

> **Closed in Phase 8 (item 7, T50).** `GET /v1/usage/summary?window=today` serves today's total, its per-provider breakdown and the two caps that actually bound spend. Per **N4** there is no daily budget and none is coming, so the total ships with no denominator and the design's progress bar stays undrawn _by decision_ — which is what the panel now says.

**UI needs:** `41k tok today` per provider (today, not lifetime), and a spend cap so
the design's progress bar has a denominator.

**What shipped:**

```
GET /v1/usage/summary?window=today
→ 200 {
  "date": "2026-09-08",              # authoritative UTC date — the client's local
                                     # `todayIsoDate()` can disagree by up to 12h
  "total_usd": 0.0184,
  "by_provider": [ { "provider": "anthropic", "usd": 0.0184, "calls": 71, "tokens": 41125 } ],
  "caps": { "workflow_max_cost_usd": 5.0, "agent_max_cost_usd": 1.0 }
}
400 UNKNOWN_WINDOW    # any window but `today`; absent means `today`
```

- `date` is the daemon's UTC day and every figure is that day's. The GUI's Today
  card filters its run count by this date too, so all three numbers describe one
  day — before T50 spend was UTC and runs were the browser's local day.
- `total_usd` is `sum(by_provider[].usd)` — one source, computed from the same
  `llm_call_log` rows grouped by `LlmUsageRepository::provider_usage_since`, so
  the breakdown adds up to the total by construction (fix round 1, R64). It is
  **not** the `llm_usage_daily` rollup (`cost_for_utc_date`) that
  `GET /v1/orchestrator/config.daily_cost_usd` reports — that rollup is a
  separate writer and this route does not serve it. `by_provider` is also
  **not** the lifetime `CostTracker::all_provider_usage()` the Models panel
  showed under a "today" heading.
- `tokens` rides on each row because the design's per-provider figure is a token
  count and the call log already holds the columns — cost and calls alone would
  have left that half of the gap open.
- `caps` is **N4 on the wire**: the per-workflow (`lead_agent_defaults.max_cost`,
  $5) and per-turn (`agent_defaults.max_cost`, $1) limits, named as such. There
  is no `daily_*` key and no daily budget — one would be a new enforcement point
  in the LLM router, not a label — and a route test serialises the response and
  fails on the substring, because what has to be prevented here is a _field_, not
  a line of prose.
- `UNKNOWN_WINDOW` is deliberately the same code word `GET /v1/agent-templates`
  answers (T48): one unknown-window refusal across the API.

The panel copy changed with it: "Spend is not capped daily by design — the caps
are $5.00 per workflow and $1.00 per agent turn", built from the served `caps`.

Plan reference: `tasks/api-fix-plan.md` §Phase 8 item 7.

---

### GAP-09 — No subagent timeline (the `Parallel work` swimlanes) — **CLOSED (Phase 4, T32)**

> **Closed in Phase 4.** `GET /v1/tasks/{id}/timeline` and the `subagent_span` event ship, over migration 037's `subagent_span` table. The swimlanes render the real per-lane start/end/state; `blocked`/`cancelled`(`"interrupted"`) are derived at read time, never stored.

**UI needs:** per-run lanes `{ label: "explore·1", start: 6, end: 50, state: "run"|"done"|"block", detail: "12 files read" }`,
rendered against a time axis (`14:22 · 14:27 · 14:33 now`). This is the single most
distinctive element of the Work view.

**Why nothing fits:**

- `AgentTaskHistory { id, agent_id, task_id, role, status, runtime_seconds?, completed_at }`
  (`models/subagent.rs`) has **no `started_at`** and no `label`/`detail`. A start time is
  only _derivable_ for finished runs (`completed_at − runtime_seconds`), and
  `runtime_seconds` is `Option` — for an in-flight or failed subagent there is nothing.
- `ServerEvent::AgentStatus` gives `{ agent_id, name, status, current_task_id,
agent_instance_id, template_id, ts }` — a point-in-time ping with no span. `name` is a
  real value now (GAP-07, resolved `298bad3`), but that does not give the timeline the
  start/end range it needs.
- `ServerEvent::DagNodeStatus` had the right _shape_ (`task_id, node_id, node_title,
agent_id, status, duration_ms, output_preview`) but the DAG executor was **deleted in
  Routing V2 Phase 5** (CLAUDE.md, "Execution Topology"), so nothing emitted it under the
  lead-agent topology — it kept firing only as a double-emission alongside the
  `subagent_span` events below, and was itself **deleted in Phase 8 (P9)** once the GUI
  switched to rendering `subagent_span` exclusively. It no longer exists; do not build on
  it.
- `ServerEvent::WorkflowProgress { task_id, lane_key, message }` is a free-text
  narration line, not a structured lane.

**Proposal — a real subagent-span resource plus live events:**

```
GET /v1/tasks/{id}/timeline
→ 200 {
  "task_id": "b41c8e02",
  "started_at": "2026-08-31T14:22:41Z",
  "now": "2026-08-31T14:33:07Z",
  "completed_at": null,
  "lanes": [
    { "lane_id": "lead",       "label": "lead",      "template_id": "lead_agent",
      "agent_instance_id": "lead_agent::9f21",
      "started_at": "...14:22:41Z", "ended_at": null,
      "state": "running", "detail": "orchestrating",
      "steps_current": 5, "steps_total": 8 },
    { "lane_id": "review_agent::7c11", "label": "review·3", "template_id": "review_agent",
      "agent_instance_id": "review_agent::7c11",
      "started_at": "...14:27:44Z", "ended_at": null,
      "state": "blocked", "detail": "awaiting you",
      "blocked_on": { "kind": "tool_confirmation", "request_id": "..." } }
  ]
}
```

`state ∈ running | done | failed | blocked | cancelled`. The UI converts
`started_at`/`ended_at` to the `start`/`end` percentages itself against
`started_at..now`.

Live counterpart (new ServerEvent variant):

```rust
SubagentSpan {
    task_id: String,
    lane_id: String,
    label: String,
    template_id: String,
    agent_instance_id: String,
    /// "started" | "progress" | "blocked" | "unblocked" | "finished"
    phase: String,
    state: String,
    detail: Option<String>,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    ts: DateTime<Utc>,
    instance_id: String,
}
```

Minimum storage change: add `started_at` (and optionally `detail`) to
`agent_task_history`, and write a row at _spawn_ time rather than only at completion.

---

### GAP-10 — No per-run event log — **CLOSED (Phase 4, T33)**

> **Closed in Phase 4.** Migration 037’s `event_log.task_id` is filled by every persistence arm that knows its run, the five security/tool `SystemEvent`/`ServerEvent` variants carry `task_id`, and `GET /v1/events/history?task_id=&agent_id=&event_type=&before=&limit=` answers `{ events, next_before }` — **always the envelope** (P20; the bare array is gone, and the one CLI consumer moved with it). Paging walks the autoincrement `id`, not the dual-format `timestamp`.

**UI needs:** the run detail's `Event log` and the Settings `Event log` section, with
rows `{ run, tag: "tool"|"steer"|"artifact"|"spawn"|"run", text, at }` — every row is
attributed to a run.

**Why nothing fits:**

- `GET /v1/events/history` (`routes/events_history.rs`) accepts only
  `{ limit, agent_id }` and returns `EventLog { id, timestamp, agent_id?, event_type,
detail?, result? }` (`models/core.rs:20`). **There is no `task_id` column** — so a
  run-scoped log is impossible even with a new query param.
- The live WS events that _would_ form the log are mostly un-attributed to a task:
  `ToolExecuted { agent_id, tool_name, success, duration_ms }`,
  `LlmCallCompleted { agent_id, model, input_tokens, output_tokens, cost_usd }`,
  `SecurityViolation { agent_id, tool_name, reason }` — all carry `agent_id`, none
  carries `task_id`. A client can only join them through `AgentStatus.current_task_id`,
  which is racy and breaks after the agent moves on.
- The WS stream is also much richer than what is persisted: `event_bridge.rs` forwards
  ~40 `SystemEvent` variants, but `EventLogRepository::log` is called from far fewer
  sites — so a reconnecting client cannot backfill what it missed.

**Proposal:**

1. Add `task_id: Option<String>` to `EventLog` (migration 035) **and** to the
   `ToolExecuted`, `LlmCallCompleted`, `SecurityViolation`, `CircuitBreakerTripped`,
   `SkillInvocationStarted`/`SkillCompleted`/`SkillFailed` and
   `ToolConfirmationRequested` ServerEvent variants.
2. Expose the scoped read:

```
GET /v1/events/history?task_id=b41c8e02&limit=200&before=<id>&event_type=tool_executed
→ 200 { "events": EventLog[], "next_before": 8812 }
```

Note the response shape change: the route used to return a **bare array**. As shipped
it is the `{ events, next_before }` envelope on _every_ path, filtered or not — the
dual shape the proposal floated was never built (P20), because a client that has to
sniff the response to know how to read it is worse than one call site updated.

Two departures from the proposal above, both narrower than it: the migration is 037,
not 035, and the three `Skill*` variants did **not** gain `task_id` — a skill
invocation is not a workflow, so there is no run to attribute one to.

---

### GAP-11 — Artifact content cannot be rendered by the browser — **CLOSED (Phase 3, T27)**

> **Closed in Phase 3.** The three content routes moved out of the auth middleware and check `?token=` inline, exactly as this section proposed. Image previews are `<img src={artifactContentUrl(id)}>`; `tauri.conf.json`'s `img-src` gained `blob:` and the loopback origins. **HTML previews stay deferred** — rendering agent markup is a security review, so HTML and SVG are shown as source.

**UI needs:** the Library `Preview` tab renders images (`screenshot-settings-drawer.png`,
`1440 × 900`) and HTML (`weekly-report.html`) inline.

**Why it is awkward:** `GET /v1/files/{id}/content` sits behind
`auth_middleware`, so `<img src={url}>` and `<iframe src={url}>` both fail — the
browser cannot attach `Authorization`. The client is forced into
`fetch → blob → URL.createObjectURL`, which means every preview is fully buffered in
memory and object URLs must be revoked by hand. Content-Type/Content-Disposition are
set correctly, so the only obstacle is auth.

**Proposal (mirror what chat SSE and the WS endpoint already do):** accept the token as
a query param on the content route, merged outside the auth layer with an inline check:

```
GET /v1/files/{id}/content?token=<bearer>
GET /v1/artifacts/{id}/content?token=<bearer>        # and .../versions/{n}/content
```

Given the daemon binds loopback and the token is a 32-byte secret, this is the same
threat model as `/v1/chat/stream/{id}?token=` and `/v1/events?token=`. Alternatively
issue short-lived per-file tickets (`POST /v1/files/{id}/ticket → { url, expires_at }`)
if query-string tokens are unwanted.

---

### GAP-12 — No pin state for artifacts — **CLOSED (Phase 3, T30)**

> **Closed in Phase 3.** The server won: migration 036 added the `pinned` column and `PUT /v1/artifacts/{id}/pin` is the writer, so a pin survives a reinstall and is visible to every client. `oa-pins` remains only as the optimistic cache, overwritten by every row the server returns.

**UI needs:** `★ Pin` / `☆ Pin` in the Library, the side panel, and the picker;
pinned items sort first and show a `★` in the list.

**Why nothing fits:** `FileAsset` has no `pinned` column; `grep -rni "pinned" crates/openalpaca_storage/src`
returns nothing. There is a generic `preference` repository
(`repository/preference/`) but no route exposes it.

**Proposal:** if pins are per-machine-per-user (they are), the simplest correct answer
is `localStorage` keyed by artifact id — **no API needed**. Only promote to the server
if pins must survive a DB reset or sync across devices, in which case:
`PUT /v1/artifacts/{id}/pin { "pinned": true } → 200 { id, pinned }` plus a `pinned`
field on `Artifact` and a `?pinned=true` filter.

---

### GAP-13 — Per-chat model override is a global mutation — **CLOSED (Phase 8 item 8, T51)**

> **Closed in Phase 8 (item 8, T51).** `POST /v1/chat` takes an optional `model`. It runs **that one turn** and is persisted nowhere, so the composer's picker is conversation-scoped and no longer rewrites `llm.toml` for every other client. The id is refused before dispatch, and the response echoes `model_used`.

**UI needs:** the composer's model picker (`claude-sonnet-4-6 ▴`) sets "the chat
model", scoped to the conversation, with an immediate visual confirmation.

**What was awkward:** the only writable model setting was
`PUT /v1/orchestrator/config { model, fallback_models }`, which **rewrites
`llm.toml` on disk** and broadcasts `OrchestratorConfigChanged` to every client and
connector (Telegram, iMessage, Discord all shift model). `POST /v1/chat` took only
`{ content, attachments }`. So the design's lightweight "pick a model for this
chat" gesture was a global, persistent config write, and the picker carried a
footer note saying so.

**What shipped:**

```
POST /v1/chat
{ "content": "...", "attachments": [], "model": "claude-opus-4-6" }
→ 200 { stream_id, lane_key, model_used: "claude-opus-4-6" }
→ 400 { "error": { "code": "UNKNOWN_MODEL", "message": "Unknown model '…': it is not in the model registry. …" } }
→ 503 { "error": { "code": "LLM_NOT_CONFIGURED", "message": "…" } }   # naming a model on a daemon with no router
```

- **Request-scoped, by design.** The id travels `GatewayRequest.model_override` →
  `HandleRequest` → `LoopOverrides::MainLoop` → `LoopConfig.model`, and the
  orchestrator's stored `loop_config` is never written — the next turn on the lane
  is back on the daemon default. The lane-scoped `preference` KV
  (`lane_model:{lane_key}`) sketched below is a **separate, later decision**, not
  an omission: remembering a pick across turns is a different promise from
  honouring one.
- **Validated before dispatch, because a bogus id is not inert.**
  `handle_simple_query` sizes the turn's trimming budget from
  `model_registry().get_model_info(model)` and falls back to a 200k window when the
  lookup misses, so an unrecognised name would quietly change how much context the
  turn keeps. `UNKNOWN_MODEL` names the id it refused. A model whose **provider is
  disabled** is refused by the same lookup and with the same code: R58b takes a
  disabled provider's rows out of the registry, so "not registered" is the one true
  answer for both, and the message says which knob puts them back. `GET /v1/models`
  reads that same registry, so neither kind of refused id is in the picker.
- `model_used` is the named model, or the router's current default — so a client
  never has to guess whether its override took. The SSE `done` frame's own `model`
  still reports what actually answered.
- **The answering model is now stored** on the assistant row.
  `conversation_messages.model` has always existed and the transcript has always
  rendered it, but nothing wrote it, so a reload dropped it — which this gap makes
  visible, since a turn can now run on a model the picker is not showing.

`PUT /v1/orchestrator/config` keeps its job: Settings → Models & keys' real "make
this the default" action, now labelled as exactly that.

Plan reference: `tasks/api-fix-plan.md` §Phase 8 item 8.

---

### GAP-14 — Connection panel: no uptime, schema version, or log path — **CLOSED (Phase 8 item 1, T44)**

> **Closed in Phase 8.** `GET /v1/status` — already the protected route that names
> the store roots (§4.7 item 4) — carries the rest:
>
> ```http
> GET /v1/status  → 200 {
>   home_root, state_dir, db_path, project_root,      # unchanged
>   started_at, uptime_secs, schema_version, log_path,
>   upload_bytes, produced_bytes,
>   sessions: { last_sweep: {…} | null, dropped_records }
> }
> ```
>
> Not in the shape proposed below, and deliberately so. **No `status`, `version`,
> `pid`, `instance_id`, `listen`** — `/v1/health` already serves those to an
> unauthenticated caller and duplicating them here would give the client two
> sources for one fact. **No `data_dir`** — the roots are three named paths
> (`home_root`/`state_dir`/`db_path`), which is what D1's layout actually has;
> one blurred "data dir" was the pre-D1 idea. **No `connectors_running` or
> `active_tasks`** — `/v1/connectors` and `/v1/tasks` are those questions, and a
> count here would go stale against them.
>
> **`schema_version` is read from the open database** (`Database::schema_version()`),
> not counted from the migration files: a binary that failed to migrate must not
> report the version it wished for. **`started_at` is stamped at the top of the
> daemon's own `async_main`**, which is why `ConnectionInfo` did _not_ need a
> `startedAt`: the route is the daemon's clock, and a daemon too slow to answer it
> is not one whose uptime is interesting. **`log_path` is `null` unless the file
> exists** (N2, resolved: serve it) — only `openalpaca daemon start` writes
> `state/logs/daemon.log`, so a `cargo run` or sidecar daemon gets no path rather
> than a path to nothing, and the Copy button is inert there with the reason shown.
> The same change **bounds that log**: 16 MB, three generations, rotated before the
> file is opened — serving a path to an unbounded file would have been an invitation.
>
> Two fields the proposal never asked for come from §4.8 and §5.4: `upload_bytes`
> and `produced_bytes` (two numbers, never one — agent output must not read as
> upload traffic against the cap) and the boot session-log sweep, whose
> `over_cap_after` is the one storage state an owner can act on.
>
> **Phase B stays open**: a real in-daemon log appender, and un-discarding the GUI
> sidecar's stdout (`src-tauri/src/lib.rs`). Until then a GUI-launched daemon has
> no log file, which is exactly what `log_path: null` reports.

**UI needs:** `uptime 4d 02h`, `Schema v33`, and `Copy log path`.

**Why nothing fits:** `GET /v1/health` (`router.rs`, `health_handler`) returns exactly
`{ status, version, pid, instance_id }`. Uptime is _derivable_ client-side from
`discovery.json.started_at`, but that field is **not** on `ConnectionInfo`
(`{ base_url, token, instance_id }` only — `discovery/mod.rs:264`), so the React client
cannot see it without a new Tauri command. The migration count is compile-time
(`crates/openalpaca_storage/src/migrations/`, highest `034_drop_context_compaction_log.sql`)
and never surfaced. Nothing anywhere returns a log path
(`grep -rn "log_path" apps/openalpacad/src/routes/` → nothing).

**Proposal:**

```
GET /v1/status            # authenticated sibling of the public /v1/health
→ 200 {
  "status": "ok",
  "version": "0.4.1",
  "protocol": 1,
  "pid": 51823,
  "instance_id": "7f3a91c4-...",
  "started_at": "2026-08-27T12:10:04Z",
  "uptime_secs": 353582,
  "schema_version": 34,
  "data_dir": "/Users/…/.openalpaca",
  "log_path": "/Users/…/.openalpaca/state/logs/daemon.log",
  "db_path": "/Users/…/openalpaca.db",
  "listen": { "host": "127.0.0.1", "port": 51823 },
  "connectors_running": 2,
  "active_tasks": 3
}
```

Keep `/v1/health` unauthenticated and minimal as-is (it is the liveness probe).
Additionally extend the Tauri `ConnectionInfo` with `startedAt` so uptime survives a
daemon that is up but slow to answer.

---

### GAP-15 — No provider enable/disable — **CLOSED (Phase 8 item 4, T47)**

> **Closed in Phase 8.** The bit was already on the wire — `ProviderInfo.enabled`
> in `GET /v1/settings/llm`, and the boot builder has always skipped a provider
> with `enabled = false`. Only the write was missing:
>
> ```http
> PUT /v1/settings/llm/providers/{provider}/enabled
> { "enabled": false }
> → 200 { "id": "ollama", "enabled": false, "loaded": false, "warning": null }
> ```
>
> `id`, not the `provider` the original proposal named: every other row this
> API serves keys itself `id`, and the path segment already says which provider
> was addressed.
>
> **`loaded` and `warning`** say what the daemon did with the bit, which is not
> always what the bit says. Turning on a provider with no usable key (or one
> this build was not compiled with) writes the file and leaves the router with
> nothing: that is still a `200` — the write happened, and a restart reaches
> the same state — but it answers `loaded: false` and carries the daemon's own
> reason. A disable is `loaded: false` with no warning; that is what it asked
> for. The GUI prints the warning under the provider row, because a switch
> reading _on_ beside a provider that cannot answer is exactly the silent
> degradation the settled rules reject.
>
> **Write-first.** `llm.toml` is the disposition, so it lands before the router
> is touched and a refused write leaves the loaded providers exactly as they
> were. The write goes through `config_io::atomic_write_with_backup` (plan §1.4,
> P-11) — tmp → fsync → rotate → rename, five versions under `state/backups/` —
> which every `/v1/settings/llm*` write now shares, not just this one.
>
> **Then the hot path.** A disable calls `deregister_provider`: in-flight calls
> finish, new ones fall through to the fallback chain or fail as unconfigured,
> and the provider's models leave the registry with it. An enable re-registers,
> puts back the catalogue the disable stripped (compiled defaults plus the
> config's own `[models]` rows) and refreshes from the provider's API —
> without that restore, a disable/enable cycle left the model picker empty
> until the daemon restarted.
>
> **`409 PROVIDER_IS_DEFAULT`** for the provider that serves the default model:
> turning it off would leave every request with nowhere to go, so it is refused
> rather than done and reported. Only the _default model's_ provider — a
> provider that merely appears in a fallback chain is not protected, because a
> chain is already a list of things that may be unavailable. A name this build
> cannot serve is **`404 PROVIDER_NOT_FOUND`**.
>
> **The guard fails closed.** `[orchestrator] model` is placed by three rungs in
> order: the live model registry, the config's own `[models]` table, then what
> the model id itself says (`claude-…` → Anthropic, `gpt-…`/`o3-…` → OpenAI, or
> an explicit `provider/model` prefix). When all three come up empty the disable
> is refused too — `409 **DEFAULT_MODEL_UNRESOLVED**`, its own code word, with
> the message naming the unresolved default — because no provider can then be
> shown _not_ to be the one that was going to answer. The word differs from
> `PROVIDER_IS_DEFAULT` because the fact does: that one says this provider
> serves the default model, which is exactly what this arm could not establish,
> so a client rendering per code would say a false thing about the row the owner
> just touched. This is not exotic-model-id territory: the compiled registry
> holds no Ollama entries and Ollama discovery needs a non-empty key pool, so a
> local-first owner whose default is an Ollama model not declared in `[models]`
> previously got no protection at all. Fixing the default model id clears it.
>
> **Not added: `enabled` on `ProviderUsageSummary`.** The proposal asked for it;
> the settings payload already carries the flag, and a second copy on a usage
> row is a second thing to keep in sync. The model picker's `off` group badge is
> not drawn either — that view is built from `GET /v1/models`, which lists
> models rather than providers, so a disabled provider has no row there to
> badge.

---

### GAP-16 — The default lane key is not discoverable — **CLOSED (Phase 0, T7)**

Landed `26b3eaf` (Phase 0): `GET /v1/me` now serves `{ user_id, default_lane_key,
sources }` — `sources` is the distinct `session.source` values for the owner,
deduped and sorted in-process over `list_conversations_for_owner` (the repository
method kept its name when migration 039 renamed the table it reads). The chat view itself
still omits `lane_key` on `GET /v1/chat/history` and reads the echoed value back (one
round trip, unaffected); `/v1/me` is for a caller that needs the default lane before it
has any chat history to ask.

**UI needs:** to load the transcript **before** sending anything
(`GET /v1/chat/history` on mount).

**Why it is awkward:** `state.default_lane_key` is used as the server-side fallback
when `lane_key` is omitted, and `ChatHistoryResponse` echoes it back — so the client
_can_ omit it and read `response.lane_key`. But there is no route that simply says
"here is your identity", and the conversation list returns rows without telling you
which one is the GUI default. Also `local_user_id` — the prefix that `is_lane_owned_by`
enforces — is never returned by any endpoint, so the client cannot construct a lane key
for a task lane or a follow-up route on its own.

**Proposal:** fold it into GAP-14's `/v1/status`, or a dedicated:

```
GET /v1/me
→ 200 { "user_id": "local", "default_lane_key": "local:gui", "sources": ["gui","telegram","imessage"] }
```

---

### GAP-17 — Connectors: no `Connect service` add flow — **OPEN** (narrowed by Phase 8 item 6, T49; one of the two entries left in `lib/unavailable.ts`)

**Narrowed by Phase 8 item 6 (T49).** The detail half is served and the `unwired`
badge was never a daemon gap; what is left is adding a connector.

**Closed (T49):** `GET /v1/connectors` rows now carry

```
[{ "id": "telegram", "name": "Telegram", "status": "active", "configured": true,
   "source": "telegram", "registered": true, "messages_7d": 184 }]
```

still a bare array (§7's list rule — unbounded ⇒ bare array; the proposal below
wanted an envelope, which would have broken both clients for nothing).

- `name` comes from `ConnectorFactory::display_name` (`crates/openalpaca_connectors/src/lib.rs`),
  a required trait method. The old route-side `match` on `"telegram" | "imessage"`
  is gone — it was written before Discord existed, which is why Discord rendered
  as its raw id.
- `source` is the attribution token the count was grouped by — the connector id,
  which is what the gateway stamps on `conversation_messages.source`.
- `registered` is whether `ConnectorManager` holds a **spawned handle**. It is
  neither "enabled" nor "alive": a handle whose task exited is still registered
  and reports `status: "error"`, so the panel can say "started, then exited"
  rather than "never started" — the one question `status` cannot answer.
- `messages_7d` is `ConversationRepository::message_counts_by_source_since`:
  one grouped query over `conversation_messages` for the whole list, filtered on
  the table's own UTC `created_at` text. **Messages, not tool calls** — the
  daemon counts no per-connector "calls" anywhere, and a `connector_call_log`
  table would have been a new writer on every inbound message for a number the
  panel shows once. A row with no `source` counts for nobody.
- The `unwired` badge is a **client-side join** (`findUnwiredConnectors`): an
  extension row's `connector` that appears in no `GET /v1/connectors` row. It
  needed nothing server-side and was never a gap of its own.

**Still missing:** `Connect service`. Connectors are compiled into the daemon
(`get_supported_connectors()` is a feature-gated `vec!`), and
`POST /v1/connectors/{id}/config` only accepts `{ token }` — so there is no route
that _adds_ a connector, only one that sets a token on one that already exists,
and any connector needing more than a bearer token has no configuration path
either.

**Proposal:**

```
POST /v1/connectors { "kind": "slack", "credentials": { … } }
→ 201 { "id": "slack", … }
```

Which presupposes a connector registry that is not a compile-time `vec!`.
GAP-24 needed the same shape for extensions and has since shipped it, but only
because a plugin and an MCP server are _already_ declared on disk — a directory
and a `[servers.<name>]` block. A connector has no such declaration to write, so
the two are no longer one piece of work.

---

### GAP-18 — No skill catalog endpoint — **CLOSED** (tool half with design C6/C7, skill half Phase 8 item 2, T45)

> **Closed in Phase 8**, in two halves. The tool half was `GET /v1/tools` (below).
> The skill half is `GET /v1/skills` — a bare array, sorted by id:
>
> ```http
> GET /v1/skills → 200 [ {
>   "id": "daily-digest", "name": "Daily Digest", "description": "…",
>   "source": "file" | "plugin",
>   "origin": { "kind": "plugin", "id": "notion", "enabled": true,
>               "state": "enabled" } | null,
>   "requires_capabilities": ["notion::create_page"],
>   "triggers": { "slash": "digest", "keywords": ["digest"] },
>   "schedule": "0 9 * * *" | null,
>   "invocations_today": 2, "version": "3.1.0" | null, "author": "plugin:notion"
> } ]
> ```
>
> Not the shape proposed below, and deliberately so. **No `scope`, and no
> `command`** — the scope is folded into `author` (`file:project` / `file:user`,
> matching `/v1/tools`' `mcp:<server>` / `plugin:<id>` convention), and the slash
> command is one of two `triggers` beside the routing keywords, because a client
> that shows how a skill fires needs both. **No embedded `health`** — the two
> routes stay separate and the client joins them on the id: `/v1/skills/health` is
> keyed by `skill_id` and outlives the catalog (a deleted directory, a disabled
> plugin's withdrawn skill), so embedding it would have made a health row
> unreachable the moment its catalog entry went away. Settings → Tools joins them
> and falls back to the id.
>
> **`origin` is `/v1/tools`' rule one axis over**: the extension row read from the
> ledger at render time for a plugin skill, and `null` for a file skill, which is
> on no ENABLE axis at all and carries no enable field of any kind. There is no
> per-skill toggle, and no route that would accept one (S1). **Nothing is
> filtered**: T2 already removes a disabled plugin's skills from the catalog
> (ADR-030 §10 case 5(a)), so a filter here would be a second enforcement point
> rather than a rendering.
>
> **`invocations_today` is the tool catalog's count one table over** — one grouped
> query over `skill_execution_log` from today's local midnight _converted to UTC_
> (the column is `datetime('now')`, i.e. UTC text), served by `idx_sel_skill_ts`.
> **`version` is `null`** when the frontmatter omits it, rather than an invented
> number.

**Closed half:** the tool catalog. `GET /v1/tools` ships with ADR-030 §8 —
`{ name, description, source, origin, provides_capabilities, requires_confirmation,
invocations_today, version, author }` — and backs the Settings → Tools rows, including
the `asks` badge and `N today`. Two fields of the shape proposed here did **not** ship,
deliberately: `denied` and `provider`. There is no per-tool enable state anywhere in the
system; availability is _derived_ — (the agent's capabilities) ∩ (its extension being
enabled) — never asserted per tool (S1), so provenance is `origin: {kind, id, enabled,
state} | null` and a builtin row carries no enable field at all.

**The skill listing, as it was missing** (`GET /v1/skills` now serves it):

- `SkillCatalog` (`orchestrator/skill/catalog/mod.rs`) has `list_names()`,
  `catalog_summary() -> Vec<(String, String, Option<String>)>`, `entries_snapshot()`,
  `count()`, `validate_dependencies()` — in-process only. The **only** skill route is
  `GET /v1/skills/health`, which returned `SkillHealthMetrics` keyed by `skill_id` with no
  name, description or trigger, which is why the Skill health rows read as ids.
  `entries_snapshot()` is what the route renders.

**Proposal (superseded by the shape above):**

```
GET /v1/skills
→ 200 [ { "id": "daily-digest", "name": "Daily digest", "description": "…",
          "source": "file" | "plugin", "scope": "user" | "project",
          "command": "/digest", "requires_capabilities": ["web_fetch"],
          "cron": "0 9 * * *",
          "health": SkillHealthMetrics | null } ]
```

A bare array, like `/v1/tools` and `/v1/extensions`. Embedding (or linking) the existing
`/v1/skills/health` payload keeps the Settings panel to one call.

---

### GAP-20 — Agent templates have no enabled state — **OPEN** (the counts closed: T34's lifetime interim, then T48's windowed, completed-only figure; the toggle is not served — the other entry left in `lib/unavailable.ts`)

**UI needs:** `12 runs 7d` per template row (**served**) and the per-template on/off
toggle in Settings → Agents (**still missing**, P-31).

**Counts — closed (T48).** `TemplateResponse` carries `run_count`, `last_run_at` and
`window`, filled from one grouped query over the table the run timeline already writes:

```
SELECT template_id, COUNT(*), MAX(started_at) FROM subagent_span
WHERE state != 'running' [AND started_at >= ?] GROUP BY template_id
```

One query per list, never one per template; a template with no matching span is absent
from the result and reports `0`. `?window=7d|30d|all` on `GET /v1/agent-templates`
(default `7d`; an unrecognised value is `400 UNKNOWN_WINDOW`) picks the cutoff — `all`
omits the `started_at` clause entirely. The count is _completed_ runs only
(`state != 'running'`): a span still open is a run in progress, not a run yet, so the
row reads `12 runs · 7d` — the daemon's own number over the window it says it used. The
pre-P8 proposal counted `agent_task_history`, which only ever knew about runs that had
already returned and carried no start time; the P8 interim counted every span
lifetime-and-in-flight, which this closes.

`GET /v1/agent-templates/{id}` (the single-template read) takes no `?window=` of its
own — it is not a listing, so a picker over it was never proposed — but returns the same
`TemplateResponse` shape, computed over the list route's `7d` default, so its `run_count`
and `window` mean the same thing wherever the row is read from.

**Enabled — still missing (P-31).** `TemplateResponse` has no `enabled` field, and
adding one is not a serialization change: nothing would enforce it where subagents are
spawned. `POST /v1/agents/{id}/action` takes `pause|resume` for a **running instance**,
which is a different concept from disabling a template. The plan's shape for when this
is picked up is a deny-class rule on the spawn capability (`spawn_subagent` refuses
template X with an attributed error; `resolve_agent_tools` and the lead's template
listing omit it) — not a third toggle axis with its own persistence, the same way
Claude Code's `permissions.deny: ["Agent(name)"]` works.

**Proposal (the toggle; the window above is already shipped):**

```
PUT /v1/agent-templates/{id}/enabled
{ "enabled": false } → 200 { "id": "writing_agent", "enabled": false }
```

---

### GAP-21 — Conversations cannot be renamed or deleted — **CLOSED (Phase 7a: daemon T39, sidebar T40)**

> **Closed in Phase 7a.** Migration 039 turned `conversations` into `session` — a lane
> now holds many conversations with exactly one `active`, enforced by a partial unique
> index — and the surface that replaced `/v1/conversations` (P19) carries the two verbs
> this gap was filed for, plus the three the rewrite made necessary:
>
> ```http
> POST   /v1/sessions {source?, workspace_path?, title?}   201 SessionView  ("New chat")
> POST   /v1/sessions/{id}/activate                        200 SessionView  (resume)
> POST   /v1/sessions/{id}/archive                         200 SessionView
> PATCH  /v1/sessions/{id} {title?, workspace_path?}       200 SessionView  (rename)
> DELETE /v1/sessions/{id}                                 204 | 409
> ```
>
> Not in the shape proposed below. **`PATCH` answers the row, not a `Conversation`** —
> every write re-reads and returns the `SessionView` the list serves, so a client never
> guesses what its own change produced. **`DELETE` answers `204` with no body**, not
> `200 { id, deleted_messages }`: the count would be a number nobody acts on, and the
> route's real refusal is `409 SESSION_HAS_ACTIVE_WORKFLOWS` — a run **this** conversation
> started is still going to write its completion report into the transcript. Another
> conversation on the same lane being busy does not block it. Every write is owner-scoped
> and answers `404` on someone else's row rather than `403` (R40); a `PATCH` carrying
> `workspace_path` for a conversation that already has a project is
> `409 SESSION_WORKSPACE_BOUND` (R48) — one project per conversation, and changing
> project is what "New chat" is for.
>
> The client half is the **chat view's conversation sidebar**, not the Settings list:
> the live conversation first, then archived by `updated_at`, each with its project, and
> the verbs beside the transcript they act on. Settings → Conversations stays a
> read-only inventory across every lane. `DELETE /v1/chat/history?lane_key=` is
> unchanged and still means what it always did — it _empties_ a conversation, it does
> not remove one.

**UI needs:** the Settings → Conversations rows carry a toggle, and lane management is
implied ("Stored lanes. Memory compaction runs weekly.").

**Why nothing fits:** `DELETE /v1/chat/history?lane_key=` deletes _messages_ and clears
the summary (`conv_repo.clear_summary(lane_key)`) but leaves the `Conversation` row
(and its `title`, `message_count`) behind. There is no
`DELETE /v1/conversations/{id}` and no rename route (`router.rs` had exactly two
conversation routes, both `GET` — the sentence the block above falsifies).

**Proposal:**

```
PATCH  /v1/conversations/{id}  { "title": "Rust workspace" } → 200 Conversation
DELETE /v1/conversations/{id}                                → 200 { "id", "deleted_messages": 142 }
```

---

### GAP-22 — Plugin lifecycle events carried no `ts` or `instance_id` — **CLOSED (design C7, T19)**

Six `ServerEvent` variants — `PluginLoaded`, `PluginUnloaded`, `PluginCrashed`,
`PluginDisabled`, `PluginPendingApproval`, `PluginNeedsConfig` — carried neither
`ts: DateTime<Utc>` nor `instance_id: String`, so plugin rows in the event log could not
be ordered. C7 deleted all six with `/v1/plugins*`, together with their producers and the
`PluginEventSink` that fed them. The extension family that replaced them
(`extension_state_changed`, `extension_capability_withheld`,
`extension_capability_withdrawn` — ADR-030 §7.3) carries both fields on every frame, so
the defect has no way back.

---

### GAP-23 — Chat messages are not linked to the runs or artifacts they produced — **CLOSED (Phase 6, T38)**

> **Closed in Phase 6.** Migration 038 adds nullable `conversation_messages.task_id`
> (plus `idx_conv_msg_task`), and **two** messages write it, because a turn cannot know
> its own artifacts: the **delegating turn** takes the id off the same
> `result.delegation` that goes on the SSE `done` frame, and the **completion report**
> takes it from the run it closed — and, by then, `file_assets` has a row per file the
> run produced, so the report also gets one
> `conversation_message_attachments` row per artifact with `role='artifact'`
> (`028:7`, the column that made a link table unnecessary; `origin != 'upload'` keeps a
> file the user attached _during_ the run out of it). No link table was added.
> Both history routes answer the stored row flattened plus
> `artifacts: [{ id, name, kind }]` — always present, `[]` on most rows — resolved for a
> whole page in one query, so the client still makes one round trip. `task_id` is
> deliberately **not** a foreign key: a pruned run leaves a dangling id the transcript
> renders as plain text rather than taking the message with it.
>
> What the GUI rebuilds from history is the **run pill** on both of those messages and
> the **artifact chips** on the report. The recap _card_ stays session-local — its
> status, duration and summary are on the `task_status` frame and on no stored message
> — so that is a rendering choice now, not a missing route. The `artifact_ids` spelling
> proposed below became `artifacts`, carrying the name and kind a chip needs so the
> client does not fetch each file just to draw a badge.

**UI needs:** the transcript's `Run finished · 13:41 / 4d81c0a2 · 6m 12s · $0.22`
recap card, and the inline `MD triage-summary.md` chip inside an assistant message.
On a fresh page load this must be reconstructible from history alone.

**Why nothing fits:** `ConversationMessage { id, lane_key, role, content, source,
model, tokens_in, tokens_out, duration_ms, created_at, content_json, display_text }`
has **no `task_id`** and no artifact references. The `delegation { task_id, title }`
link exists _only_ on the live SSE `done` frame — it is not persisted into the message
row (`GatewayPersistence` writes the message; the delegation lives on
`GatewayResponse`). So after a reload the client cannot tell which assistant turn
started which run.

**Proposal:** add nullable `task_id: Option<String>` and
`artifact_ids: Vec<String>` to `ConversationMessage` (migration + `GatewayPersistence`
write), and surface both in `GET /v1/chat/history` and the per-conversation messages
route (`GET /v1/sessions/{id}/messages` since P19). `content_json` could carry them without a
schema change, but a typed field is far easier for the client to rely on.

---

### GAP-24 — No extension install / uninstall route — **CLOSED (Phase 8 item 9, T52)**

_(Was GAP-19, "no plugin install route", widened in ADR-030 §9.1 to both extension kinds:
the same mechanism was missing for an MCP server, which had no gap id at all.)_

**What shipped**, in the shape proposed — the `path` variant only, `{kind}` singular the
way the rest of the family speaks, and the uninstall as a **flag on the DELETE that
already existed** rather than a second path:

```
POST /v1/extensions/plugin  { "source": "path", "path": "/Users/…/my-plugin" }
→ 201 { "extension": <the §8 row>, "manifest": { name, version, description, entry,
        capabilities, virtual_capabilities, types, required_config_keys,
        sensitive_config_keys }, "added_capabilities": [], "consent_reset": false }

POST /v1/extensions/plugin/validate  { "path": "…" }
→ 200 { "manifest": { … }, "installed": false }          // the dry run; copies nothing

PUT  /v1/extensions/plugin/{id}  { "source": "path", "path": "…" }
→ 200 the same envelope, with `added_capabilities` / `consent_reset` filled in

POST /v1/extensions/mcp  { "name": "github", "transport": "stdio", "command": "npx",
                           "args": [], "enabled": true }
→ 201 { "extension": <the §8 row>, "manifest": null }

DELETE /v1/extensions/{kind}/{id}?uninstall=true[&keep_data=false]
→ 200 { "removed": "…", "trashed": "…/plugins/.trash/notion-2026…", "kept_data": true,
        "data_trashed": null }
```

Four things the routes make true rather than promise.

**An install grants nothing.** The directory lands with its toggle at the serde default
(on) and _no consent decision_, so the row reads `unapproved`/`never_seen` and the
existing `approve` verb is the single action that starts it — which is why the manifest
summary travels beside the row: it is the approval preview, and the row alone cannot
answer "what would approving grant?". The `.permissions.toml` entry records
`installed_from`/`installed_at`.

**An uninstall deletes nothing.** T0–T5, then the permissions entry through the same
atomic writer, then the directory _moved_ to `plugins/.trash/<id>-<ts>/` — a directory
the owner dropped in is never `rm -rf`'d — and `keep_data` (default true) decides
`plugins/.data/<id>/`, which is moved rather than removed when it is not kept. It is
also the only path that **expires the C5 tombstones**: with no uninstall, `/slash` and
`spawn_subagent` kept answering for a plugin that no longer existed, and a re-install
under the same name inherited them.

**Nothing is ever half-copied.** A copy lands in `plugins/.staging/` and becomes
`plugins/<name>` with one rename; an abandoned staging copy sweeps itself. An update
stages _before_ it tears the running plugin down, so a source that cannot be copied has
not cost the owner anything — and because the child runs with `current_dir(plugin_dir)`,
the incumbent is trashed and the staged tree renamed into the name it vacated rather
than replaced in place.

**The bare DELETE still means what it meant.** Without `?uninstall=true` it is C6's
orphan-row removal, which never touches a directory. The destructive half has to be
asked for by name.

Refusals: `400` `invalid_path` / `escaping_symlink` / `unsupported_source` /
`invalid_declaration`, `404` `source_not_found`, `422` `invalid_manifest`, `409`
`already_installed` / `already_declared` / `not_disabled` / `busy`, `500` `copy_failed`
— each with the word a client branches on and a sentence a person reads.
`extension_state_changed` narrates the result over WS.

**Still declined:** `source: "url"`. Fetching and unpacking an archive from the network
is its own security review, and it is refused by name rather than by serde error.

CLI: `openalpaca ext install|update|uninstall`, `ext mcp add|remove`. GUI: Settings →
Extensions grows the `Add extension` panel (with the dry run behind `Check`), and each
row's overflow menu grows `Update…` and `Uninstall…`.

---

### Summary table

Every gap is accounted for below: the task that closed it, or `OPEN`. The last
column is the estimate this table carried **where it carried one** — the rows
that shipped in Phase 0, and the ones the table never listed, have no recorded
estimate and say so with `—`; nothing here was estimated after the fact.
**Two rows are open**, and they are the two entries in `src/lib/unavailable.ts`.

| #   | Gap                                    | Blocks                          | Outcome                                                           | Est. was                         |
| --- | -------------------------------------- | ------------------------------- | ----------------------------------------------------------------- | -------------------------------- |
| 01  | no "Always allow" scope on the wire    | third confirmation button       | closed T7 (Phase 0, `88e8a3b`)                                    | —                                |
| 02  | no steer endpoint                      | Steer button                    | closed T35 — `POST /v1/tasks/{id}/steer`, owner-scoped            | **S–M**                          |
| 03  | no follow-up API                       | Queue follow-up                 | closed T36 — `/v1/lanes/{lane_key}/followups` ×3                  | **M**                            |
| 04  | no artifact list / attribution         | **entire Library view**         | closed T27 (routes) + T31 (Library), migration 036                | **L** (migration + routes)       |
| 05  | no artifact versions / diff            | History + Diff tabs             | closed T29 — `…/versions`, `…/diff`                               | **L** (migration + routes)       |
| 06  | no `rerun` / `start` action            | Re-run, Start now               | closed T37 — `rerun` is its own route, `201` with a new id        | **S**                            |
| 07  | `task_status`/`agent_status` drop text | run cards, agent rows           | closed T8 (Phase 0, `298bad3`)                                    | —                                |
| 08  | cost served nowhere usable             | per-run cost, today's spend     | closed T7 for the per-run and daily halves; the rest became 08c   | —                                |
| 08c | no usage rollup, no served cap         | Connection spend panel          | closed T50 — `GET /v1/usage/summary?window=today`                 | —                                |
| 09  | no subagent timeline                   | **Parallel work swimlanes**     | closed T32 — `…/timeline` + `subagent_span`, migration 037        | **L** (migration + events)       |
| 10  | event log has no `task_id`             | per-run event log               | closed T33 — `event_log.task_id` + the history envelope           | **M** (migration)                |
| 11  | content route is header-auth only      | image/html preview              | closed T27 — `?token=` checked inline on the three content routes | **S** (query token)              |
| 12  | no pin state                           | ★ Pin                           | closed T30 — `PUT /v1/artifacts/{id}/pin`, server state not local | **XS** — do it in `localStorage` |
| 13  | per-chat model override is global      | model picker                    | closed T51 — `POST /v1/chat { model }`, request-scoped            | **M**                            |
| 14  | uptime / schema / log path             | Connection panel                | closed T44 — `GET /v1/status`                                     | **S**                            |
| 15  | no provider enable/disable             | Models section switches         | closed T47 — `PUT …/providers/{provider}/enabled`                 | —                                |
| 16  | default lane key undiscoverable        | every lane-scoped call          | closed T7 (Phase 0, `26b3eaf`) — `GET /v1/me`                     | —                                |
| 17  | no `Connect service` add flow          | Connectors section's Add button | **OPEN** — narrowed by T49 (the detail half is served)            | **M**                            |
| 18  | no tool / skill catalog                | Tools and Skills rows           | closed — `GET /v1/tools` (C6/C7) and `GET /v1/skills` (T45)       | —                                |
| 19  | no plugin install route                | Add extension                   | became GAP-24 (widened to both kinds, ADR-030 §9.1)               | —                                |
| 20  | no template run counts / enabled       | Agents section                  | **OPEN** for the toggle; counts closed T34 → T48                  | **M**                            |
| 21  | no conversation rename/delete          | Conversations rows              | closed T39 (daemon) + T40 (sidebar), migration 039                | **S**                            |
| 22  | `plugin_*` events lacked `ts`/id       | Event log dedupe                | closed T19 (design C7) — the variants were deleted                | —                                |
| 23  | messages not linked to runs/artifacts  | transcript recap cards          | closed T38 — migration 038                                        | **M** (migration)                |
| 24  | no extension install / uninstall       | Add extension                   | closed T52 — install, validate, update, `?uninstall=true`         | **M**                            |

**Recommended order** (kept as written, and followed): the XS/S column first
(11, 14, 06, 02, 21), then 04 + 05 + 09 as one "run observability + artifacts"
milestone, since they share the same storage work and are what the Work and
Library views are built around. That is the order Phases 0–8 ran in; the UI
shipped with the unserved surfaces empty-stated rather than feature-flagged,
through the `unavailable.ts` registry.

---

## 4. Streaming contract

### 4.1 SSE — chat

**Endpoint.** `GET {baseUrl}/v1/chat/stream/{stream_id}?token={token}`
Auth is inline in `chat_stream_handler` (query param), not the middleware.
Keep-alive comments every `server.sse_keep_alive_secs` (default **15 s**).

**Lifecycle (from `chat/service.rs::send_message` and `routes/chat.rs`):**

1. Client `POST /v1/chat` → `{ stream_id, lane_key }`. The handler synchronously
   creates the broadcast channel (`ChatStreamManager::create_stream`, capacity **128**)
   **before** returning, then spawns the work task and publishes
   `SystemEvent::ChatStreamStarted` → WS `chat_stream_started`.
2. The spawned task **sleeps 100 ms** before the first event, explicitly to give the
   client time to subscribe. **The client must open the EventSource immediately on
   receiving `stream_id` — do not await anything else first.**
3. `thinking` — sent once, after the sleep. Data is literally `{}`.
4. `delta` — `{ "content": "<chunk>" }`, one per `stream_chunk_words` words
   (default **3**), spaced `stream_chunk_delay_ms` (default **30 ms**). Chunks preserve
   exact bytes (whitespace/newlines/indentation) — concatenating all `delta.content`
   reproduces the final text. Note: this is **simulated** streaming — the daemon has the
   full response before the first delta.
5. `confirmation_requested` — `{ request_id, tool_name, tool_arguments }`. May arrive
   **at any point before `done`**, including before any `delta`. It does **not**
   terminate the stream; the same stream continues after
   `POST /v1/chat/confirmations/{request_id}` resolves. The identical information also
   arrives on the WS as `tool_confirmation_requested` (with `agent_id`, `stream_id`,
   `lane_key`) — dedupe by `request_id`.
6. **Terminal:** exactly one of
   - `done` — `{ content, model, tokens_in, tokens_out, duration_ms, attachments_used?, delegation? }`.
     `content` is the **full** text (not the tail) — prefer it over the accumulated
     deltas as the source of truth. `attachments_used` and `delegation` are omitted
     entirely when absent (`skip_serializing_if = "Option::is_none"`), and the SSE
     payload strips the serde `"event"` tag (`done_event_data`).
   - `error` — `{ message }`.
7. `SystemEvent::ChatStreamEnded { stream_id, lane_key, status: "completed"|"error" }`
   is published → WS `chat_stream_ended`.
8. **The stream is removed 5 s after the terminal event**
   (`tokio::time::sleep(Duration::from_secs(5)); stream_manager.remove(&sid)`).
   A `GET` after that window returns `404 { error: { code: "STREAM_NOT_FOUND" } }`.
   _Practical rule: a reconnect is only viable inside that 5 s._ Because the transport
   is a `tokio::sync::broadcast`, a late subscriber gets **only events sent after it
   subscribed** — there is no replay. A reconnect mid-stream therefore loses the deltas
   it missed; recover by falling back to `GET /v1/chat/history` for the final message.

**Independent GC:** `spawn_chat_cleanup` (`apps/openalpacad/src/background.rs`) runs
every `cleanup_interval_secs` (default **60 s**) and drops streams idle longer than
`stale_timeout_secs` (default **30 s**), measured from `last_active` (refreshed on
every send), not `created_at`.

**Errors the client must handle:**

| Condition               | Response                                                                                                                                                                                |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| chat service absent     | `503 { error: { code: "CHAT_NOT_CONFIGURED" } }` on POST and on stream GET                                                                                                              |
| bad/expired token       | `401 "Invalid token"` (plain text) on the stream GET                                                                                                                                    |
| stream gone / >5 s late | `404 { error: { code: "STREAM_NOT_FOUND", message: "Stream not found or expired" } }`                                                                                                   |
| unknown attachment      | `404 ATTACHMENT_NOT_FOUND` / `403 ATTACHMENT_ACCESS_DENIED` on POST                                                                                                                     |
| too many attachments    | `400 TOO_MANY_ATTACHMENTS`                                                                                                                                                              |
| lane not owned          | `403 FORBIDDEN` (history/delete)                                                                                                                                                        |
| broadcast lag           | the SSE bridge silently drops lagged frames (`Err(_) => None` in `make_sse_stream`) — **a very slow client can lose deltas without any error**; always reconcile against `done.content` |

**Reference client shape:**

```ts
const { stream_id, lane_key } = await post("/v1/chat", {
  content,
  attachments,
});
const es = new EventSource(sseUrl(stream_id)); // open IMMEDIATELY
let buf = "";
es.addEventListener("thinking", () => setPhase("thinking"));
es.addEventListener("delta", (e) => {
  buf += JSON.parse(e.data).content;
  render(buf);
});
es.addEventListener("confirmation_requested", (e) =>
  openConfirmCard(JSON.parse(e.data)),
);
es.addEventListener("done", (e) => {
  const d = JSON.parse(e.data);
  commit(d);
  es.close();
});
es.addEventListener("error", (e) => {
  /* distinguish: */
});
// NOTE: EventSource fires a *transport* `error` (e.data === undefined) on network
// failure AND the server sends a *named* `error` event with a JSON body. Branch on
// whether `e.data` is present, or you will swallow real server errors.
es.onerror = () => {
  /* transport: after `done` this always fires — close() first */
};
```

Because `EventSource` auto-reconnects on transport failure and the server has no
`Last-Event-ID` replay, **call `es.close()` inside the `done`/`error` handlers** or the
browser will reopen a stream that is about to 404.

If `EventSource` is too limiting (no headers, no abort reason), `fetch` +
`ReadableStream` + a small SSE frame parser works identically; the token still goes in
the query string.

### 4.2 WebSocket — system events

**Endpoint.** `GET {wsBaseUrl}/v1/events?token={token}` (upgrade).

**Semantics (from `routes/events.rs`):**

- Server → client only, one JSON `ServerEvent` per text frame,
  `#[serde(tag = "type", rename_all = "snake_case")]`.
- The server answers client `Ping` with `Pong`; it does not send app-level pings.
  A daemon-wide `heartbeat` event `{ type:"heartbeat", ts, instance_id }` is broadcast
  centrally (not per connection) — use it as the liveness signal.
- **Lag handling:** on `RecvError::Lagged(n)` the handler logs and `continue`s — the
  client silently misses `n` events with no notification. Treat the WS as _best-effort
  live hints_, never as the source of truth; always have a REST refetch path for events
  a lagged client dropped entirely. (`task_status.title`/`agent_status.name` used to add
  a second reason — an event that _arrived_ but carried an empty field — but GAP-07's
  fix, `298bad3`, closed that one; a dropped frame is still possible.)
- `RecvError::Closed` breaks the loop and closes the socket.
- No subscription/filtering protocol: every client gets every event.
- The client's own union of the frames it handles is
  `apps/openalpaca-gui/src/lib/events.ts`, and the one place a frame becomes a
  refetch is `lib/query-client.ts` — a frame is **never** rendered from its
  payload, which is why a late, dropped or reordered one cannot show a state the
  daemon is not in. `dag_node_status` was deleted in Phase 8 (T53); the daemon
  has no `plugin_*` frame either (C7).

**Client lifecycle to reproduce (matches `daemon.ts`):**

1. `ensure_daemon_running` → open WS.
2. `onopen` → state `connected`, reset backoff to 1000 ms.
3. `onmessage` → parse, tag with a monotonic local `_id`, prepend to a bounded ring
   (existing client keeps **100**; the design's Event log wants more — 500–1000 is fine).
4. `onclose` → state `disconnected`; if reconnect is enabled, schedule with
   `min(backoff × [0.8,1.2), 30000)` then `backoff = min(backoff × 2, 30000)`.
5. On each reconnect attempt: `get_connection_info`; **if `instanceId` differs from the
   cached one, do a full re-bootstrap** (the daemon restarted — every `task_id`,
   `stream_id`, and `request_id` you hold is dead) rather than just reopening the socket.
6. Null all four handlers **before** `close()` when tearing down, or `onclose` will
   re-trigger `scheduleReconnect` (the existing `teardownWebSocket` does this
   deliberately).

**Which WS events drive which surface:**

| Surface                 | Events                                                                                                                                                                                                                                                                                                                                   |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Work list / run cards   | `task_status`, `workflow_started`, `workflow_progress`, `workflow_steered`, `followup_queued`                                                                                                                                                                                                                                            |
| Swimlanes               | `subagent_span` (**GAP-09, closed**) invalidates the run's timeline query; `agent_status` is weak (see the closed GAP-09 section). `dag_node_status` used to fire alongside from the same spawn sites but was never rendered — it was **deleted in Phase 8 (P9)**, so `subagent_span` is now the only event describing a lane transition |
| Chat confirmation card  | `tool_confirmation_requested` (dedupe with the SSE frame by `request_id`)                                                                                                                                                                                                                                                                |
| Library                 | `artifact_written` (**GAP-04, closed**) invalidates the artifact keys — list, versions and diff — and the producing run's own log. Carries `path`                                                                                                                                                                                        |
| Conversation sidebar    | `session_changed` (**GAP-21, closed**) invalidates the session list and the transcript; a `status: "interrupted"` frame names a run, so it invalidates the run keys too                                                                                                                                                                  |
| Pending follow-ups      | `followup_queued`, `followup_cancelled` (**GAP-03, closed**)                                                                                                                                                                                                                                                                             |
| Event log               | `tool_executed`, `llm_call_completed`, `security_violation`, `circuit_breaker_tripped`, `skill_invocation_started`/`skill_completed`/`skill_failed`, `command_received`, `wake`                                                                                                                                                          |
| Settings → Connectors   | `connector_status`                                                                                                                                                                                                                                                                                                                       |
| Settings → Extensions   | `extension_state_changed` (invalidate and re-read the row — never render the payload), `extension_capability_withheld`, `extension_capability_withdrawn`. The six `plugin_*` variants this row used to name were **deleted in C7** with `/v1/plugins*`                                                                                   |
| Settings → Models       | `key_status_changed`, `orchestrator_config_changed`, `daemon_config_changed`                                                                                                                                                                                                                                                             |
| Settings → Skills       | `skill_catalog_updated`                                                                                                                                                                                                                                                                                                                  |
| Settings → Agents       | `agent_config_changed`, `agent_status`                                                                                                                                                                                                                                                                                                   |
| Connection chip         | `heartbeat`, plus the socket's own open/close                                                                                                                                                                                                                                                                                            |
| Chat stream bookkeeping | `chat_stream_started`, `chat_stream_ended`                                                                                                                                                                                                                                                                                               |

---

## 5. Notes for the implementer

- **Two response envelope styles exist and they are inconsistent.** Chat/settings use
  `{ error: { code, message } }` (settings adds `status`); tasks/agents/extensions/connectors
  use `{ error: "<string>" }` — for `/v1/extensions` the string is a **word**
  (`not_loaded`, `store_unreadable`, `unsupported_for_kind`, `not_orphaned`, `orphaned`)
  the client maps to copy, ADR-030 §8. Write one `parseError(response)` that handles both.
- **Two list styles exist too.** `GET /v1/tasks`, `GET /v1/connectors`,
  `GET /v1/extensions`, `GET /v1/tools`, `GET /v1/models` and `GET /v1/skills/health`
  return **bare arrays**; `GET /v1/sessions`, `/v1/chat/history`, `/v1/orchestrator/latency`,
  `/v1/orchestrator/decisions` and `/v1/events/history` (always, P20) return **envelopes**. Any new endpoint proposed above
  uses an envelope — do not "fix" the old ones without versioning.
- **`GET /v1/tasks` and `GET /v1/tasks/{id}` still disagree.** The list row is a
  flattened `Task` carrying `outcome`, `cost_usd` and `subagent_count`; the detail
  route nests it as `{ task, outcome }` and has neither of the last two. The agent-run half of the disagreement is
  settled: **P8** deleted the list's `assigned_agents` summary array and the detail
  route's `assignments` key (`AgentTaskHistory[]` under `#[serde(rename)]`) — both are
  served by `GET /v1/tasks/{id}/timeline` now, which also has lanes for subagents that
  have not finished. Model the two remaining shapes explicitly in TypeScript; do not
  assume the list row and the detail row are the same type.
- **`Task.status` is an enum in Rust but a bare string on the wire.** Values:
  `queued | running | paused | completed | failed | cancelled`. The design's `runs[]`
  uses `running/queued/paused/done/cancelled` — map `completed`→`done` and give
  `failed` a state the design does not currently draw (it only has the green DONE and
  the red CANCELLED terminal cards; `failed` should reuse the red card with different
  copy).
- **`/v1/tasks?status=active`** is a special mode (`repo.list_active`), not a
  `TaskStatus` value — anything else goes through `str::parse::<TaskStatus>()` and
  `400`s on failure.
- **Never trust `progress_current`/`progress_total` to be present** — both are
  `Option<i32>` and the design's `5/8 steps` needs a fallback.
- The design's `LANES`, `ARTS`, `LOG`, `AXIS`, `SECTIONS` and `state.runs` constants are
  **mock fixtures**, not a contract. Where this document says a field is missing, the
  fixture is the _requirement_; the proposed API shape above is the contract.
- `apps/openalpaca-gui/src/lib/api/*.ts` already has working, correctly-typed callers
  for tasks, agents, templates, instances, settings, usage, plugins, files, skills,
  latency, decisions, sessions and feedback. Port those call signatures verbatim
  into the React data layer — only the store/reactivity wrapper needs rewriting.
  _(Done. That directory is now the React data layer itself, and the `plugins`
  module named above is `extensions.ts` — the `/v1/plugins*` routes it wrapped were
  deleted in C7. `artifacts.ts`, `followups.ts`, `run-events.ts`, `sessions.ts`,
  `status.ts`, `tools.ts`, `usage.ts` and `workspaces.ts` are the modules the closed
  gaps above added.)_
