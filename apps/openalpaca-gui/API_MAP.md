# OpenAlpaca GUI — API Map (UI ↔ Daemon)

Maps every data need of the new React UI (design canvas `OpenAlpaca.dc.html`, 1440×900,
four views: **Chat**, **Work**, **Library**, **Settings** + command palette) onto the
daemon's real HTTP/SSE/WS surface.

**Sources verified (all field names below are read from these files, not inferred):**

| What                          | File                                                                                         |
| ----------------------------- | -------------------------------------------------------------------------------------------- |
| All routes                    | `apps/openalpacad/src/router.rs`                                                             |
| Chat req/resp                 | `apps/openalpacad/src/routes/chat.rs`, `chat_types.rs`                                       |
| Tasks                         | `apps/openalpacad/src/routes/tasks.rs`, `tasks_types.rs`                                     |
| Settings/LLM/usage/models     | `apps/openalpacad/src/routes/settings.rs`, `settings_types.rs`                               |
| Agents/templates/instances    | `apps/openalpacad/src/routes/agents.rs`, `agents_types.rs`                                   |
| Files                         | `apps/openalpacad/src/routes/files.rs`, `files_types.rs`                                     |
| Plugins / connectors / skills | `routes/plugins.rs`, `routes/connectors.rs`, `routes/skills.rs`                              |
| Telemetry                     | `routes/events_history.rs`, `routes/orchestrator_latency.rs`, `routes/dispatch_decisions.rs` |
| WS event union                | `crates/openalpaca_api/src/events/mod.rs` (`ServerEvent`)                                    |
| Internal→WS bridge            | `apps/openalpacad/src/event_bridge.rs`                                                       |
| SSE event union               | `crates/openalpaca_core/src/chat/stream_manager/mod.rs` (`ChatStreamEvent`)                  |
| SSE lifecycle                 | `crates/openalpaca_core/src/chat/service.rs`, `apps/openalpacad/src/background.rs`           |
| Storage models                | `crates/openalpaca_storage/src/models/*.rs`, `src/repository/*`                              |
| Existing client               | `apps/openalpaca-gui/src/lib/daemon.ts`, `src/lib/api/*.ts`                                  |
| Tauri host                    | `apps/openalpaca-gui/src-tauri/src/lib.rs`, `crates/openalpaca_storage/src/discovery/mod.rs` |

> **Security note.** The design file was read as _data only_. It contains no text
> directed at the agent; all strings in it are UI copy and mock fixtures. Nothing in it
> was treated as an instruction.

---

## 1. Connection & auth

### 1.1 How it works today

The daemon writes `~/Library/Application Support/OpenAlpaca/discovery.json`
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

| UI element (design)                                        | Endpoint / event                                                                                                                    | Request                                                                                                                           | Response (real field names)                                                                                                                      |
| ---------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| Send composer                                              | ✅ `POST /v1/chat`                                                                                                                  | `{ content: string, attachments?: AttachmentRef[] }` — `AttachmentRef { file_id, caption? }`. Optional header `x-workspace-path`. | `{ stream_id: string, lane_key: string }`                                                                                                        |
| Streaming reply                                            | ✅ `GET /v1/chat/stream/{stream_id}?token=` (SSE)                                                                                   | —                                                                                                                                 | events `thinking` / `delta` / `done` / `error` / `confirmation_requested` — see §4                                                               |
| Assistant meta line `sonnet-4-6 · 3.8s · 1284/612 tok`     | ✅ SSE `done`                                                                                                                       | —                                                                                                                                 | `{ content, model, tokens_in, tokens_out, duration_ms, attachments_used?: string[], delegation?: { task_id, title } }`                           |
| "Started a background workflow" card                       | ✅ SSE `done.delegation` + WS `workflow_started`                                                                                    | —                                                                                                                                 | `delegation { task_id, title }`; WS `{ type:"workflow_started", task_id, lane_key, title, ts, instance_id }`                                     |
| Transcript on load / scrollback                            | ✅ `GET /v1/chat/history?limit&offset&lane_key`                                                                                     | —                                                                                                                                 | `{ messages: ConversationMessage[], total: i64, lane_key: string }`                                                                              |
| `ConversationMessage` fields                               | —                                                                                                                                   | —                                                                                                                                 | `{ id: i64, lane_key, role, content, source?, model?, tokens_in?, tokens_out?, duration_ms?, created_at: string, content_json?, display_text? }` |
| Clear lane                                                 | ✅ `DELETE /v1/chat/history?lane_key=`                                                                                              | —                                                                                                                                 | `{ deleted: u64 }` (also clears the conversation summary)                                                                                        |
| Confirmation card `Confirmation required · shell_execute`  | ✅ SSE `confirmation_requested` **and** WS `tool_confirmation_requested`                                                            | —                                                                                                                                 | SSE `{ request_id, tool_name, tool_arguments }`; WS adds `{ agent_id, stream_id?, lane_key? }`                                                   |
| `Approve ↵` / `Deny esc`                                   | ✅ `POST /v1/chat/confirmations/{request_id}`                                                                                       | `{ approved: boolean }`                                                                                                           | `200 OK` (empty) / `404 { error: { code:"NOT_FOUND", message } }`                                                                                |
| `Always allow` (allowlist)                                 | ✅ same route, `approval_scope: "entire_tool"` — GAP-01, resolved `88e8a3b`                                                         |                                                                                                                                   |                                                                                                                                                  |
| 👍/👎 on a message                                         | ✅ `PUT\|GET\|DELETE /v1/chat/messages/{message_id}/feedback`                                                                       | PUT `{ feedback: "positive"\|"negative", comment?: string }`                                                                      | `{ message_id, feedback, comment? }` / `{ deleted: bool }`                                                                                       |
| Model picker (`claude-sonnet-4-6 ▴`)                       | ⚠️ `GET /v1/models` + `PUT /v1/orchestrator/config` — see **GAP-13**                                                                |                                                                                                                                   | `/v1/models` → `available_models()` (`DiscoveredModel[]`); orchestrator PUT `{ model, fallback_models }`                                         |
| `{{ spend }} today` footer                                 | ✅ `GET /v1/orchestrator/config.daily_cost_usd` — GAP-08a, resolved `7dbb988` (sums today's UTC `llm_usage_daily` rows server-side) |                                                                                                                                   | `OrchestratorConfigResponse.daily_cost_usd: f64`                                                                                                 |
| `steer → connector audit` user bubble                      | ⚠️ no HTTP route — send `POST /v1/chat` with body `"/steer <text>"` — see **GAP-02**                                                |                                                                                                                                   |                                                                                                                                                  |
| `Queue follow-up` composer mode                            | ❌ no route — see **GAP-03**                                                                                                        |                                                                                                                                   |                                                                                                                                                  |
| Attachments (drag-drop)                                    | ✅ `POST /v1/files/upload` (multipart, 100 MB `DefaultBodyLimit`)                                                                   | multipart                                                                                                                         | `{ id, filename, mime_type, size_bytes, status }`                                                                                                |
| Attachment count cap                                       | ✅ enforced server-side                                                                                                             | —                                                                                                                                 | `400 TOO_MANY_ATTACHMENTS` when `> upload.max_files_per_message`                                                                                 |
| Inline artifact card (`MD connector-audit-findings.md v2`) | ❌ no artifact API — see **GAP-04 / GAP-05**                                                                                        |                                                                                                                                   |                                                                                                                                                  |

**Lane keys.** Format is `"{user_id}:{source_name}"`. `is_lane_owned_by` (chat_types.rs)
rejects any lane not prefixed `"{local_user_id}:"` with `403 FORBIDDEN`. The GUI's
default lane is `state.default_lane_key`; the client learns it from the `lane_key` in
the `POST /v1/chat` response. `GET /v1/me` (GAP-16, resolved `26b3eaf`) now also serves
`default_lane_key` directly for a caller that wants it before any chat history exists.

### 2.2 Work view (runs)

| UI element                                                                             | Endpoint / event                                                                                                                                                          | Request                                       | Response                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Run list, `{{ activeCount }} active · {{ doneCount }} done`                            | ✅ `GET /v1/tasks?status=active\|<status>&created_by=&limit=`                                                                                                             | —                                             | **array** of serialized `Task` + injected `assigned_agents[]` + injected `outcome` + injected `cost_usd: f64` (GAP-08b, resolved `a827dcf`)                                                                                                                                                                                                                                                                                                     |
| `Task` fields                                                                          | —                                                                                                                                                                         | —                                             | `{ id, title, description?, status, priority, progress_current?, progress_total?, result_summary?, created_by, source_lane, created_at, updated_at, completed_at?, state_version, outcome_kind?, artifact_count }` (`state_json`/`outcome_json` are `#[serde(skip_serializing)]`) — list route only, adds `cost_usd: f64` (`0.0` for a task with no logged calls, never omitted); the detail route's `Task` (row below) still has no such field |
| `TaskStatus` values                                                                    | —                                                                                                                                                                         | —                                             | `queued` / `running` / `paused` / `completed` / `failed` / `cancelled` (parsed by `str::parse::<TaskStatus>()`; `status=active` is a special list mode → `repo.list_active`)                                                                                                                                                                                                                                                                    |
| `assigned_agents[]` (list route only)                                                  | —                                                                                                                                                                         | —                                             | `[{ agent_id, role, status, runtime_seconds?, completed_at }]` from `agent_task_history`                                                                                                                                                                                                                                                                                                                                                        |
| Run detail header (`{{ sel.title }}`, `started {{ sel.started }}`)                     | ✅ `GET /v1/tasks/{id}`                                                                                                                                                   | —                                             | `{ task: Task, assignments?: AgentTaskHistory[], outcome?: ParsedOutcomeFields }`                                                                                                                                                                                                                                                                                                                                                               |
| `ParsedOutcomeFields`                                                                  | —                                                                                                                                                                         | —                                             | `{ outcome_summary?, outcome_kind: string, artifact_count: i32, artifacts: Value[], no_artifact_reason? }`                                                                                                                                                                                                                                                                                                                                      |
| `AgentTaskHistory`                                                                     | —                                                                                                                                                                         | —                                             | `{ id, agent_id, task_id, role, status, runtime_seconds?, completed_at }`                                                                                                                                                                                                                                                                                                                                                                       |
| `Pause` / `Resume` / `Cancel run`                                                      | ✅ `POST /v1/tasks/{id}/action`                                                                                                                                           | `{ action: "cancel" \| "pause" \| "resume" }` | `{ task_id, status }`; `409` on illegal transition with a human message; `400 Unknown action` otherwise                                                                                                                                                                                                                                                                                                                                         |
| `Start now` (queued run)                                                               | ❌ not a valid action — see **GAP-06**                                                                                                                                    |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `Re-run`                                                                               | ❌ not a valid action — see **GAP-06**                                                                                                                                    |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Live status ticks                                                                      | ✅ WS `task_status`                                                                                                                                                       | —                                             | `{ type:"task_status", task_id, title, status, progress_current, progress_total, result_summary, outcome_kind?, artifact_count?, outcome_summary?, ts, instance_id }` — `title` is filled at every producer site (GAP-07, resolved `298bad3`), no longer `""` on updates                                                                                                                                                                        |
| `{{ r.meta }}` = `11m 04s · 5/8 steps · $0.41`                                         | ✅ duration derivable (`created_at`→`updated_at`/`completed_at`), steps = `progress_current`/`progress_total`, cost = list row's `cost_usd` (GAP-08b, resolved `a827dcf`) |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `Parallel work` swimlanes (`lead`, `explore·1`, `code·2`, `review·3` with start/end %) | ❌ no per-subagent start/end timeline — see **GAP-09**                                                                                                                    |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Lane detail strings (`12 files read`, `awaiting you`)                                  | ❌ — see **GAP-09**                                                                                                                                                       |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `Files · {{ r.fileCount }}` per run                                                    | ❌ files are not linked to tasks — see **GAP-04**                                                                                                                         |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Per-run `Event log` (`{{ e.tag }} {{ e.text }} {{ e.at }}`)                            | ❌ `GET /v1/events/history` filters only by `agent_id` — see **GAP-10**                                                                                                   |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `Steer` button                                                                         | ⚠️ chat-mediated only — **GAP-02**                                                                                                                                        |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `Queue follow-up` button                                                               | ❌ — **GAP-03**                                                                                                                                                           |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Steering acknowledgement                                                               | ✅ WS `workflow_steered` `{ task_id, lane_key, ts, instance_id }`                                                                                                         |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Progress narration                                                                     | ✅ WS `workflow_progress` `{ task_id, lane_key, message, ts, instance_id }`                                                                                               |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Follow-up queued badge                                                                 | ✅ WS `followup_queued` `{ lane_key, followup_id: i64, kind: "followup"\|"unprocessed_steering", ts, instance_id }` — ⚠️ read-back list is missing, see **GAP-03**        |                                               |                                                                                                                                                                                                                                                                                                                                                                                                                                                 |

### 2.3 Library view (artifacts)

The whole view has **no backing API**. The design's `ARTS[]` fixture shape is:

```ts
{ id, name, kind: "md"|"code"|"term"|"table"|"plan"|"image"|"html",
  badge, run, runName, agent, version: "v2 of 2", stamp, time, sub,
  versions: [{ v, note, by, when }] }
```

| UI element                                                            | Status                                                                                                                                                                                                                                                  |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Library · {{ artCount }} files` list                                 | ❌ **GAP-04** — no list endpoint. `FileAssetRepository` has only `get_by_id`, `list_orphaned`, `list_by_status`.                                                                                                                                        |
| Kind filter chips (`All/Docs/Code/Output/Data/Media/Plans`)           | ❌ **GAP-04**                                                                                                                                                                                                                                           |
| `run` / `runName` / `agent` attribution                               | ❌ **GAP-04** — `FileAsset` has `{ id, owner_id, filename, mime_type, size_bytes, storage_path, status, extracted_text?, extract_error?, metadata_json?, created_at, updated_at }`; no `task_id`, no `agent_id`.                                        |
| Preview tab                                                           | ⚠️ `GET /v1/files/{id}/content` streams bytes with `Content-Type` + `Content-Disposition: inline`, but it is **Bearer-protected**, so `<img src>` / `<iframe src>` cannot load it — see **GAP-11**                                                      |
| `Diff v1→v2` tab                                                      | ❌ **GAP-05** — no versioning anywhere in storage                                                                                                                                                                                                       |
| `History` tab (`versions[]`)                                          | ❌ **GAP-05**                                                                                                                                                                                                                                           |
| `★ Pin`                                                               | ❌ **GAP-12**                                                                                                                                                                                                                                           |
| `Export`                                                              | ⚠️ client can fetch content and save via Tauri fs plugin (already a dependency) — no server route needed                                                                                                                                                |
| `Reveal` (in Finder)                                                  | ⚠️ `POST /v1/files/{id}/open` exists but _opens with the default app_ (`opener::open` on a staged copy in `$TMPDIR/openalpaca-open/{id}-{safe_name}`), it does not reveal — response `{ id, status: "opened" }`. Reveal should be a Tauri-side command. |
| `+41 −6` diff stats, `exit 0 · 1.4s`, `1440 × 900`, `3 rows` sublines | ❌ **GAP-05** — these are per-kind metadata the daemon never records (`metadata_json` is free-form and unpopulated for agent output)                                                                                                                    |

### 2.4 Settings view

| Section                                                                       | Endpoint(s)                                                                                                                                                                                                                                                                                                        | Notes                                                                                                                                                                                                                                            |
| ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Connection** — `Daemon connected`                                           | ✅ `GET /v1/health` → `{ status, version, pid, instance_id }`                                                                                                                                                                                                                                                      |                                                                                                                                                                                                                                                  |
| `uptime 4d 02h`                                                               | ⚠️ derivable client-side from `discovery.json.started_at` via the Tauri command; **not** on `/v1/health` — **GAP-14**                                                                                                                                                                                              |
| `Instance 7f3a91c4`                                                           | ✅ `health.instance_id` / `ConnectionInfo.instanceId`                                                                                                                                                                                                                                                              |                                                                                                                                                                                                                                                  |
| `Endpoint 127.0.0.1:51823`                                                    | ✅ `ConnectionInfo.baseUrl`                                                                                                                                                                                                                                                                                        |                                                                                                                                                                                                                                                  |
| `Schema v33`                                                                  | ❌ **GAP-14** — migration count is compile-time only (`crates/openalpaca_storage/src/migrations/`, currently 034)                                                                                                                                                                                                  |
| `Reconnect`                                                                   | ✅ client-side (re-invoke `ensure_daemon_running`, reopen WS)                                                                                                                                                                                                                                                      |                                                                                                                                                                                                                                                  |
| `Copy log path`                                                               | ❌ **GAP-14** — no route or Tauri command returns the daemon log path                                                                                                                                                                                                                                              |
| `Today: {{ spend }} of $5.00 cap · 15 runs · 41k tokens`                      | ⚠️ spend from `GET /v1/orchestrator/config.daily_cost_usd` (GAP-08a, resolved `7dbb988`); tokens from `GET /v1/llm/usage/daily?date=` summed client-side (no other source); run count from `GET /v1/tasks?limit=…` filtered client-side; **the cap is not served, by design — no daily budget (N4)** — **GAP-08c** |
| **Models & keys** — provider rows                                             | ✅ `GET /v1/settings/llm` (full `LlmConfig`), `GET /v1/settings/llm/status` (`key_health()`), `GET /v1/settings/llm/providers/usage` → `ProviderUsageSummary[] { provider, total_cost_usd, total_tokens, total_requests, health, external_usage? }`                                                                | ⚠️ `health` is hardcoded `"healthy"` in `get_provider_usage`                                                                                                                                                                                     |
| model chips per provider                                                      | ✅ `GET /v1/models`, `POST /v1/models/refresh`                                                                                                                                                                                                                                                                     |                                                                                                                                                                                                                                                  |
| `key added 12 Jul`                                                            | ⚠️ present inside the `GET /v1/settings/llm` config payload; verify the field survives redaction before relying on it                                                                                                                                                                                              |
| `41k tok today` per provider                                                  | ⚠️ `ProviderUsageSummary.total_tokens` is **lifetime**, not today; per-day-per-provider requires client math over `/v1/llm/usage?limit=` (`LlmCallLog { timestamp, agent_id, task_id, provider, model, key_id, input_tokens, output_tokens, cost_usd, status, latency_ms, error_message }`) — **GAP-08c**          |
| `Add provider` / key CRUD                                                     | ✅ `PUT /v1/settings/llm` (upsert), `DELETE /v1/settings/llm/keys/{provider}/{key_id}`, `PUT .../keys/reorder`, `PUT .../keys/priority`, `POST .../validate`, `GET .../credentials`, `POST .../credentials/rescan`, `GET .../cli-backends`                                                                         |                                                                                                                                                                                                                                                  |
| provider on/off toggle                                                        | ❌ **GAP-15** — no enable/disable route; only add/remove keys                                                                                                                                                                                                                                                      |
| **Connectors** rows                                                           | ⚠️ `GET /v1/connectors` → `[{ id, name, status, configured }]`                                                                                                                                                                                                                                                     | Name mapping is **hardcoded** for `telegram`/`imessage` only; MCP servers and plugin-declared connectors do not appear                                                                                                                           |
| toggle / delete                                                               | ✅ `POST /v1/connectors/{id}/action` `{ action: "enable"\|"disable"\|"delete" }`                                                                                                                                                                                                                                   |                                                                                                                                                                                                                                                  |
| config / settings                                                             | ✅ `POST /v1/connectors/{id}/config` `{ token }`; `GET\|PUT /v1/connectors/{id}/settings` (`{ settings: Record<string,string> }`, keys must be `"{id}."`-prefixed and present in `config_schema::CONFIG_KEYS`)                                                                                                     |                                                                                                                                                                                                                                                  |
| `184 calls 7d`, `unwired` tag, `Connect service`                              | ❌ **GAP-17**                                                                                                                                                                                                                                                                                                      |                                                                                                                                                                                                                                                  |
| live status                                                                   | ✅ WS `connector_status` `{ id, status, ts, instance_id }`                                                                                                                                                                                                                                                         |                                                                                                                                                                                                                                                  |
| **Skills** rows (`shell_execute`, `file_edit`, `web_fetch`, `memory_search`…) | ❌ **GAP-18** — the design's "skills" are actually **tools**; neither the tool registry nor the skill catalog has an HTTP listing                                                                                                                                                                                  |
| skill health                                                                  | ✅ `GET /v1/skills/health` → `SkillHealthMetrics[] { skill_id, total_invocations, clean_success_rate, clean_success_rate_7d, repair_rate, repair_effectiveness, degraded_rate, avg_duration_ms, avg_cost_usd, avg_rounds, last_invoked_at?, user_satisfaction_rate?, feedback_count, feedback_coverage }`          | health only — no name/description/`asks`/enabled state                                                                                                                                                                                           |
| `asks` (requires confirmation) badge                                          | ❌ **GAP-18**                                                                                                                                                                                                                                                                                                      |                                                                                                                                                                                                                                                  |
| `9 uses today`                                                                | ⚠️ `total_invocations` is lifetime; no daily breakdown                                                                                                                                                                                                                                                             |                                                                                                                                                                                                                                                  |
| skill lifecycle events                                                        | ✅ WS `skill_catalog_updated`, `skill_invocation_started` `{ request_id, skill_id, query_preview }`, `skill_completed` `{ request_id, skill_id, duration_ms, output_preview }`, `skill_failed` `{ request_id, skill_id, error }`                                                                                   |                                                                                                                                                                                                                                                  |
| **Plugins** rows                                                              | ✅ `GET /v1/plugins` → `[{ name, version, status, tools, connector, provider, models }]`                                                                                                                                                                                                                           |                                                                                                                                                                                                                                                  |
| approve/deny/enable/disable/config                                            | ✅ `POST /v1/plugins/{name}/{approve\|deny\|enable\|disable}`; `POST /v1/plugins/{name}/config` `{ key, value }`                                                                                                                                                                                                   |                                                                                                                                                                                                                                                  |
| `Install plugin`                                                              | ❌ **GAP-19**                                                                                                                                                                                                                                                                                                      |                                                                                                                                                                                                                                                  |
| `Declares 2 connectors, 0 registered` warn tag                                | ⚠️ derivable: `plugins[].connector` non-null vs. absent from `GET /v1/connectors` — client-side join                                                                                                                                                                                                               |
| plugin lifecycle                                                              | ✅ WS `plugin_loaded` `{ plugin_id, tools[] }`, `plugin_unloaded`, `plugin_crashed { error, restart_in_secs }`, `plugin_disabled { reason }`, `plugin_pending_approval { capabilities[] }`, `plugin_needs_config { missing_keys[] }` — ⚠️ these six carry **no `ts`/`instance_id`** (unlike every other variant)   |                                                                                                                                                                                                                                                  |
| **Agents** rows (templates)                                                   | ✅ `GET /v1/agent-templates` → `TemplateResponse[] { id, name, description, icon?, singleton, capabilities[], denied_capabilities[], temperature, verbosity, model?, fallback_models[], max_tool_calls?, timeout_seconds?, max_cost_per_task?, require_confirmation_for[], persona, body }`                        | plus `GET\|PUT\|DELETE /v1/agent-templates/{id}`, `POST /v1/agent-templates`                                                                                                                                                                     |
| running instances                                                             | ✅ `GET /v1/agent-instances` → `InstanceResponse[] { id, template_id, name, status, current_task? }`                                                                                                                                                                                                               |                                                                                                                                                                                                                                                  |
| agent CRUD/config/action                                                      | ✅ `GET\|POST /v1/agents`, `POST /v1/agents/from-toml`, `GET\|DELETE /v1/agents/{id}`, `GET\|PUT /v1/agents/{id}/config` (`{ config: AgentConfigFile, config_version: u64 }` — optimistic lock), `POST /v1/agents/{id}/action` `{ action: "pause"\|"resume" }`                                                     |                                                                                                                                                                                                                                                  |
| `12 runs 7d` per template                                                     | ❌ **GAP-20** — `AgentMetrics { agent_id, tasks_completed, tasks_failed, total_runtime_seconds, average_runtime_seconds, success_rate, updated_at }` is lifetime and keyed by _instance_, not template; and it is only returned on `GET /v1/agents/{id}`                                                           |
| template on/off toggle                                                        | ❌ **GAP-20**                                                                                                                                                                                                                                                                                                      |                                                                                                                                                                                                                                                  |
| **Conversations** rows                                                        | ✅ `GET /v1/conversations?source&limit&offset` → `{ conversations: Conversation[] }`                                                                                                                                                                                                                               | `Conversation { id, lane_key, source, title, message_count: i64, last_message_at?, created_at, updated_at, summary, summary_version: i64, last_summarized_message_id: i64, summary_updated_at? }` — covers `142 messages`, `29 Aug`, `compacted` |
| messages of one conversation                                                  | ✅ `GET /v1/conversations/{id}/messages?limit&offset` → `{ messages: ConversationMessage[], total: i64 }`                                                                                                                                                                                                          |                                                                                                                                                                                                                                                  |
| delete / rename a conversation                                                | ❌ **GAP-21** — `DELETE /v1/chat/history?lane_key=` clears _messages_ but the conversation row and its title are untouched, and there is no rename                                                                                                                                                                 |
| **Event log** section                                                         | ⚠️ `GET /v1/events/history?limit&agent_id` → `EventLog[] { id, timestamp, agent_id?, event_type, detail?: Value, result?: Value }`                                                                                                                                                                                 | ⚠️ the WS stream carries far more (and richer) events than the persisted log — see **GAP-10**                                                                                                                                                    |
| **Orchestrator observability** (not in the design; available if wanted)       | ✅ `GET /v1/orchestrator/latency?mode&from&to&limit` → `{ records: [...] }`; `GET /v1/orchestrator/latency/aggregate?from&to` → `{ aggregates: [...] }`; `GET /v1/orchestrator/decisions?mode&from&to&limit` → `{ records: [...] }`                                                                                | `mode` values are historical strings (`two_phase_*`, `planner_*`, `fast_path`, `no_llm`) plus current Routing V2 modes                                                                                                                           |

### 2.5 Command palette (⌘K)

Every palette command maps onto an existing action already covered above
(`New background job` → composer, `Steer …` → GAP-02, `Approve pending shell_execute`
→ `POST /v1/chat/confirmations/{id}`, `Go …` → client routing,
`Find <filename>` → GAP-04, `Toggle compact density` → client state).
No new API is needed for the palette _shell_; only GAP-04 blocks the `Find` entries.

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

---

### GAP-01 — "Always allow" cannot be expressed _(trivial fix, high value)_ — **RESOLVED**

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

### GAP-02 — No first-class steering endpoint

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

### GAP-03 — No follow-up queue API (write or read)

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

### GAP-04 — No artifact/output API _(the Library view is entirely unbacked)_

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

Plus a `ServerEvent::ArtifactWritten { artifact_id, task_id, agent_id, name, kind, version, ts, instance_id }`
so the chat transcript can render the card the moment it lands (the design's
`artifact` event-log tag and the `connector-audit-findings.md v2 written` line).

---

### GAP-05 — No artifact versioning, history, or diff

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

### GAP-06 — Task actions are missing `rerun` and `start`

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

---

### GAP-07 — `task_status` drops `title` and `agent_status` drops `name` — **RESOLVED**

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

### GAP-08 — Cost is not served anywhere the UI can use it

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
3. **The cap and a real summary rollup — still open, renamed GAP-08c.** See below.

**Proposal (implemented for 1 and 2):**

```
# 1. one new query param on an existing route — IMPLEMENTED
GET /v1/llm/usage?task_id=b41c8e02&limit=200   → LlmCallLog[]

# 2. fix daily_cost_usd (no API change) — IMPLEMENTED
```

---

### GAP-08c — No usage-summary rollup or served cost cap

**UI needs:** `41k tok today` per provider (today, not lifetime), and a served spend
cap so the design's progress bar has a denominator. Per **N4** (settled — no daily
budget, only per-workflow/per-turn caps), there is **no** `daily_*` cap to serve; the
summary's `caps` object carries the per-workflow and per-turn `max_cost` values instead,
and today's total ships as an unbounded figure.

**Why nothing fits:** `ProviderUsageSummary.total_tokens` is `all_provider_usage()`,
lifetime since boot, not a per-day figure — there is no per-day-per-provider rollup, and
`execution.max_cost` / `orchestrator.costs.*` live in config with no route (the only
daemon-config route, `/v1/daemon/config/providers`, returns web-search settings only).

**Proposal:**

```
GET /v1/usage/summary?window=today
→ 200 {
  "date": "2026-08-31",              # authoritative UTC date — client's local
                                      # `todayIsoDate()` can disagree by up to 12h
  "total_cost_usd": 0.0184,
  "by_provider": [ { "provider": "anthropic", "cost_usd": 0.0184, "tokens": 41125, "requests": 71 } ],
  "caps": { "workflow_max_cost_usd": 5.0, "agent_max_cost_usd": 1.0 }
}
```

`by_provider` is computed from today's `llm_call_log` rows, not the lifetime
`all_provider_usage()`. Plan reference: `tasks/api-fix-plan.md` §Phase 8 item 7.

---

### GAP-09 — No subagent timeline (the `Parallel work` swimlanes)

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
- `ServerEvent::DagNodeStatus` has the right _shape_ (`task_id, node_id, node_title,
agent_id, status, duration_ms, output_preview`) but the DAG executor was **deleted in
  Routing V2 Phase 5** (CLAUDE.md, "Execution Topology"), so nothing emits it under the
  lead-agent topology. Do not build on it.
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

### GAP-10 — No per-run event log

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

Note the response shape change: today the route returns a **bare array**; a
`{ events, next_before }` envelope is what the UI's infinite log needs. Keep the bare
array on the un-filtered path if backward compatibility matters, or version it.

---

### GAP-11 — Artifact content cannot be rendered by the browser

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

### GAP-12 — No pin state for artifacts

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

### GAP-13 — Per-chat model override is a global mutation

**UI needs:** the composer's model picker (`claude-sonnet-4-6 ▴`) sets "the chat
model", scoped to the conversation, with an immediate visual confirmation.

**Why it is awkward:** the only writable model setting is
`PUT /v1/orchestrator/config { model, fallback_models }`, which **rewrites
`llm.toml` on disk** and broadcasts `OrchestratorConfigChanged` to every client and
connector (Telegram, iMessage, Discord all shift model). `POST /v1/chat` takes only
`{ content, attachments }` — no per-request model. So the design's lightweight
"pick a model for this chat" gesture is actually a global, persistent config write.

**Proposal:** accept an optional per-request override:

```
POST /v1/chat
{ "content": "...", "attachments": [], "model": "claude-haiku-4-5" }
→ 200 { stream_id, lane_key, model: "claude-haiku-4-5" }
→ 400 { "error": { "code": "UNKNOWN_MODEL", "message": "..." } }
```

and/or a lane-scoped preference:
`PUT /v1/lanes/{lane_key}/preferences { "model": "..." } → 200 { lane_key, model }`.
Keep `PUT /v1/orchestrator/config` for the Settings → Models panel's real "make this
the default" action, and label the composer picker accordingly.

---

### GAP-14 — Connection panel: no uptime, schema version, or log path

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
  "data_dir": "/Users/…/Library/Application Support/OpenAlpaca",
  "log_path": "/Users/…/Library/Application Support/OpenAlpaca/openalpacad.log",
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

### GAP-15 — No provider enable/disable

**UI needs:** the per-provider toggle in Settings → Models & keys (`Local (Ollama)`
shown `off`), and the model picker's `off` group badge.

**Why nothing fits:** the key routes are add/remove/reorder/priority/validate only
(`router.rs` `/v1/settings/llm/*`). `grep -rn "enabled" apps/openalpacad/src/routes/settings.rs`
finds no provider-enable path. Removing every key is the only way to "turn a provider
off", which is destructive and not what the toggle implies.

**Proposal:**

```
PUT /v1/settings/llm/providers/{provider}/enabled
{ "enabled": false }
→ 200 { "provider": "ollama", "enabled": false }
```

and add `enabled: bool` to each provider entry in the `GET /v1/settings/llm` payload
and to `ProviderUsageSummary`.

---

### GAP-16 — The default lane key is not discoverable — **RESOLVED**

Landed `26b3eaf` (Phase 0): `GET /v1/me` now serves `{ user_id, default_lane_key,
sources }` — `sources` is the distinct `conversations.source` values for the owner,
deduped and sorted in-process over `list_conversations_for_owner`. The chat view itself
still omits `lane_key` on `GET /v1/chat/history` and reads the echoed value back (one
round trip, unaffected); `/v1/me` is for a caller that needs the default lane before it
has any chat history to ask.

**UI needs:** to load the transcript **before** sending anything
(`GET /v1/chat/history` on mount).

**Why it is awkward:** `state.default_lane_key` is used as the server-side fallback
when `lane_key` is omitted, and `ChatHistoryResponse` echoes it back — so the client
_can_ omit it and read `response.lane_key`. But there is no route that simply says
"here is your identity", and `/v1/conversations` returns rows without telling you which
one is the GUI default. Also `local_user_id` — the prefix that `is_lane_owned_by`
enforces — is never returned by any endpoint, so the client cannot construct a lane key
for a task lane or a follow-up route on its own.

**Proposal:** fold it into GAP-14's `/v1/status`, or a dedicated:

```
GET /v1/me
→ 200 { "user_id": "local", "default_lane_key": "local:gui", "sources": ["gui","telegram","imessage"] }
```

---

### GAP-17 — Connectors: no call counts, no "unwired" signal, no add flow

**UI needs:** `184 calls 7d` / `0 calls` per connector, the `unwired` badge
("Declared by plugin manifest, never registered"), and a `Connect service` action.

**Why nothing fits:** `GET /v1/connectors` returns `{ id, name, status, configured }`
and nothing else (`routes/connectors.rs`). The display name is a hardcoded `match` on
`"telegram" | "imessage"` — anything else falls back to the raw id, and MCP servers
(`config/mcp.toml`) and plugin-declared connectors never appear in this list at all.
There is no usage counter for connectors anywhere in storage. `POST /v1/connectors/{id}/config`
only accepts `{ token }`, so any connector needing more than a bearer token has no
add flow.

**Proposal:**

```
GET /v1/connectors
→ 200 { "connectors": [
    { "id": "telegram", "name": "Telegram", "kind": "connector",
      "status": "running", "configured": true, "enabled": true,
      "source": "builtin",                       // builtin | plugin | mcp
      "declared_by": null,
      "registered": true,                        // false ⇒ the "unwired" badge
      "calls_7d": 184, "last_call_at": "…" },
    { "id": "notion", "name": "Notion", "kind": "connector",
      "status": "unwired", "configured": false, "enabled": false,
      "source": "plugin", "declared_by": "openalpaca-notion",
      "registered": false, "calls_7d": 0, "last_call_at": null }
  ] }
```

(Note this changes the response from a bare array to an envelope — version it or add
`/v2`.) `calls_7d` requires a per-connector counter that does not exist yet; the
cheapest source is a `connector_call_log` table or reusing `event_log` once it gains
structured `detail`.

---

### GAP-18 — No tool/skill catalog endpoint

**UI needs:** the Settings → Skills rows: `{ name, description, asks (requires
confirmation), uses today, enabled }` for `shell_execute`, `file_edit`, `web_fetch`,
`memory_search`, `calendar_read`, `email_send`.

**Why nothing fits:** these are **tools**, not skills, and neither registry is exposed.

- Tools: `ToolRegistry` (`crates/openalpaca_core/src/tools/registry/mod.rs`) has
  `registered_tool_names()`, `iter_registered_tools()`, `tools_for_capabilities()`,
  `extension_tool_defs(deny)` — all in-process. `grep -rn "v1/tools" apps/openalpacad`
  → nothing.
- Skills: `SkillCatalog` (`orchestrator/skill/catalog/mod.rs`) has `list_names()`,
  `catalog_summary() -> Vec<(String, String, Option<String>)>`, `entries_snapshot()`,
  `count()`, `validate_dependencies()` — again in-process only. The **only** skill route
  is `GET /v1/skills/health`, which returns `SkillHealthMetrics` keyed by `skill_id`
  with no name, description, trigger, or enabled flag.
- "Asks first": `AgentTemplate.require_confirmation_for: Vec<String>` is served per
  template, and `execution.skill_defaults.global_tool_deny` lives in daemon config —
  neither gives a per-tool "this will ask you" answer.

**Proposal — two new listings:**

```
GET /v1/tools
→ 200 { "tools": [
    { "name": "shell_execute", "description": "Runs a command in the project directory",
      "source": "builtin",                   // builtin | mcp | plugin
      "provider": null,                      // "<server>" for mcp, "<plugin>" for plugin
      "capability": "shell_execute",
      "requires_confirmation": true,         // ⇒ the "asks" badge
      "denied": false,                       // in execution.skill_defaults.global_tool_deny
      "invocations_today": 9, "invocations_7d": 61,
      "parameters_schema": { … } }
  ] }

GET /v1/skills
→ 200 { "skills": [
    { "id": "daily-digest", "name": "Daily digest", "description": "…",
      "source": "file" | "plugin", "scope": "user" | "project",
      "command": "/digest", "requires_capabilities": ["web_fetch"],
      "cron": "0 9 * * *", "enabled": true,
      "health": SkillHealthMetrics | null } ] }
```

`GET /v1/skills` should embed (or link) the existing `/v1/skills/health` payload so the
Settings panel needs one call. `invocations_today` depends on GAP-10's structured
event log or a dedicated counter.

---

### GAP-19 — No plugin install route

**UI needs:** `Install plugin` in Settings → Plugins.

**Why nothing fits:** `router.rs` exposes list / approve / deny / enable / disable /
config only. Installation today means dropping a directory into
`~/Library/Application Support/OpenAlpaca/plugins/` by hand and restarting the daemon.

**Proposal:**

```
POST /v1/plugins/install
{ "source": "path", "path": "/Users/…/my-plugin" }        // or { "source": "url", "url": "…" }
→ 202 { "name": "openalpaca-notion", "version": "0.3.1", "status": "pending_approval",
        "capabilities": ["connectors","tools"] }
```

Then the existing `POST /v1/plugins/{name}/approve` completes the flow, and
`plugin_pending_approval` / `plugin_loaded` narrate it over WS. If URL installs are out
of scope for the local-first threat model, ship the `path` variant only.

---

### GAP-20 — Agent templates have no run counts and no enabled state

**UI needs:** `12 runs 7d` per template row and the per-template on/off toggle in
Settings → Agents.

**Why nothing fits:** `TemplateResponse` (agents_types.rs) is pure static config — no
counters, no `enabled`. `AgentMetrics { agent_id, tasks_completed, tasks_failed,
total_runtime_seconds, average_runtime_seconds, success_rate, updated_at }` is
lifetime-scoped, keyed by _agent instance_ not template, and only returned on
`GET /v1/agents/{id}` (embedded in `AgentResponse.metrics`). `POST /v1/agents/{id}/action`
takes `pause|resume` for a **running instance**, which is a different concept from
disabling a template.

**Proposal:**

```
GET /v1/agent-templates?window=7d
→ TemplateResponse[] each gaining:
   { "enabled": true, "runs_7d": 12, "runs_total": 143,
     "success_rate": 0.94, "last_run_at": "…" }

PUT /v1/agent-templates/{id}/enabled
{ "enabled": false } → 200 { "id": "writing_agent", "enabled": false }
```

`runs_7d` is a `SELECT count(*) FROM agent_task_history WHERE agent_id LIKE '<template>::%' AND completed_at > …`
— cheap, but note it only counts _completed_ runs until GAP-09 adds `started_at`.

---

### GAP-21 — Conversations cannot be renamed or deleted

**UI needs:** the Settings → Conversations rows carry a toggle, and lane management is
implied ("Stored lanes. Memory compaction runs weekly.").

**Why nothing fits:** `DELETE /v1/chat/history?lane_key=` deletes _messages_ and clears
the summary (`conv_repo.clear_summary(lane_key)`) but leaves the `Conversation` row
(and its `title`, `message_count`) behind. There is no
`DELETE /v1/conversations/{id}` and no rename route (`router.rs` has exactly two
conversation routes, both `GET`).

**Proposal:**

```
PATCH  /v1/conversations/{id}  { "title": "Rust workspace" } → 200 Conversation
DELETE /v1/conversations/{id}                                → 200 { "id", "deleted_messages": 142 }
```

---

### GAP-22 — Six plugin ServerEvent variants omit `ts` and `instance_id`

**Why it matters:** every other `ServerEvent` variant carries
`ts: DateTime<Utc>` and `instance_id: String`; the client keys its event log and its
stale-instance guard on those. `PluginLoaded`, `PluginUnloaded`, `PluginCrashed`,
`PluginDisabled`, `PluginPendingApproval`, `PluginNeedsConfig`
(`crates/openalpaca_api/src/events/mod.rs`) have neither — confirmed in the existing
client's union type too (`daemon.ts`, the six plugin arms have only `_id`).

**Proposal:** add `ts` and `instance_id` to all six for uniformity. Non-breaking for
readers that ignore extra fields.

---

### GAP-23 — Chat messages are not linked to the runs or artifacts they produced

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
write), and surface both in `GET /v1/chat/history` and
`GET /v1/conversations/{id}/messages`. `content_json` could carry them without a
schema change, but a typed field is far easier for the client to rely on.

---

### Summary table

Gaps 01, 07, 08.1, 08.2 and 16 shipped in Phase 0 (`88e8a3b`, `298bad3`, `a827dcf`,
`7dbb988`, `26b3eaf`) and are removed from this table; see their now-**RESOLVED**
sections above. GAP-08's remaining piece (the cap and a real usage-summary rollup)
continues below as GAP-08c.

| #   | Gap                                         | Blocks                                 | Fix size                         |
| --- | ------------------------------------------- | -------------------------------------- | -------------------------------- |
| 08c | no usage-summary rollup; no served cost cap | spend cap; per-provider "today" tokens | **S**                            |
| 11  | content route is header-auth only           | image/html preview                     | **S** (query token)              |
| 14  | uptime / schema / log path                  | Connection panel                       | **S**                            |
| 06  | no `rerun` / `start` action                 | Re-run, Start now                      | **S**                            |
| 02  | no steer endpoint                           | Steer button                           | **S–M**                          |
| 03  | no follow-up API                            | Queue follow-up                        | **M**                            |
| 15  | no provider enable/disable                  | Models toggles                         | **S**                            |
| 21  | no conversation rename/delete               | Conversations rows                     | **S**                            |
| 22  | plugin events lack `ts`/`instance_id`       | event log ordering                     | **XS**                           |
| 13  | per-chat model override is global           | model picker                           | **M**                            |
| 18  | no tool/skill catalog                       | Skills section                         | **M**                            |
| 20  | no template run counts / enabled            | Agents section                         | **M**                            |
| 17  | connector call counts / unwired / add       | Connectors section                     | **M**                            |
| 19  | no plugin install                           | Install plugin                         | **M**                            |
| 10  | event log has no `task_id`                  | per-run event log                      | **M** (migration)                |
| 23  | messages not linked to runs/artifacts       | transcript recap cards                 | **M** (migration)                |
| 09  | no subagent timeline                        | **Parallel work swimlanes**            | **L** (migration + events)       |
| 04  | no artifact list / attribution              | **entire Library view**                | **L** (migration + routes)       |
| 05  | no artifact versions / diff                 | History + Diff tabs                    | **L** (migration + routes)       |
| 12  | no pin state                                | ★ Pin                                  | **XS** — do it in `localStorage` |

**Recommended order:** the XS/S column first (08c, 11, 14, 06, 02, 15, 21, 22) unblocks roughly two-thirds of the design for a handful of one-file changes. Then
04 + 05 + 09 as one "run observability + artifacts" milestone, since they share the
same storage work and are what the Work and Library views are actually built around.
Ship the UI with those three surfaces feature-flagged/empty-stated until then.

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

| Surface                 | Events                                                                                                                                                                          |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Work list / run cards   | `task_status`, `workflow_started`, `workflow_progress`, `workflow_steered`, `followup_queued`                                                                                   |
| Swimlanes               | `agent_status` (weak — see GAP-09); `dag_node_status` is dead under the lead-agent topology                                                                                     |
| Chat confirmation card  | `tool_confirmation_requested` (dedupe with the SSE frame by `request_id`)                                                                                                       |
| Event log               | `tool_executed`, `llm_call_completed`, `security_violation`, `circuit_breaker_tripped`, `skill_invocation_started`/`skill_completed`/`skill_failed`, `command_received`, `wake` |
| Settings → Connectors   | `connector_status`                                                                                                                                                              |
| Settings → Plugins      | `plugin_loaded`/`plugin_unloaded`/`plugin_crashed`/`plugin_disabled`/`plugin_pending_approval`/`plugin_needs_config`                                                            |
| Settings → Models       | `key_status_changed`, `orchestrator_config_changed`, `daemon_config_changed`                                                                                                    |
| Settings → Skills       | `skill_catalog_updated`                                                                                                                                                         |
| Settings → Agents       | `agent_config_changed`, `agent_status`                                                                                                                                          |
| Connection chip         | `heartbeat`, plus the socket's own open/close                                                                                                                                   |
| Chat stream bookkeeping | `chat_stream_started`, `chat_stream_ended`                                                                                                                                      |

---

## 5. Notes for the implementer

- **Two response envelope styles exist and they are inconsistent.** Chat/settings use
  `{ error: { code, message } }` (settings adds `status`); tasks/agents/plugins/connectors
  use `{ error: "<string>" }`. Write one `parseError(response)` that handles both.
- **Two list styles exist too.** `GET /v1/tasks`, `GET /v1/connectors`, `GET /v1/plugins`,
  `GET /v1/models`, `GET /v1/skills/health`, `GET /v1/events/history` return **bare
  arrays**; `GET /v1/conversations`, `/v1/chat/history`, `/v1/orchestrator/latency`,
  `/v1/orchestrator/decisions` return **envelopes**. Any new endpoint proposed above
  uses an envelope — do not "fix" the old ones without versioning.
- **`GET /v1/tasks` and `GET /v1/tasks/{id}` disagree.** The list injects
  `assigned_agents` (a hand-built summary array) and `outcome`; the detail route returns
  `{ task, assignments, outcome }` where `assignments` is the **full**
  `AgentTaskHistory[]` under a legacy key (`#[serde(rename = "assignments")]`, kept
  deliberately for CLI/GUI compatibility). Model both shapes explicitly in TypeScript;
  do not assume the list row and the detail row are the same type.
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
  latency, decisions, conversations and feedback. Port those call signatures verbatim
  into the React data layer — only the store/reactivity wrapper needs rewriting.
