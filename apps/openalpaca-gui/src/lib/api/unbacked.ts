/**
 * Adapters for the design surfaces the daemon cannot serve (API_MAP §3).
 *
 * Each function returns an `Availability<T>` whose `T` is the shape the
 * *proposed* endpoint would return, so views can be written against the real
 * contract today and lose nothing but the `unavailable` branch when the route
 * lands. Nothing here fabricates rows.
 *
 * Not every adapter has a `hooks/useUnbacked` wrapper: where a surface has a
 * working alternative rather than an empty state, the view handles the gap at
 * the point of use — `components/work/run-actions` disables `Start now`,
 * `Re-run` and `Queue follow-up` and names the missing route. The adapters
 * below stay as the shape those routes take.
 */

import { unavailable, type Availability } from "../unavailable";

// The artifact resource left this file when Phase 3 landed `/v1/artifacts*`:
// the `Artifact`, `ArtifactVersion` and `ArtifactDiff` types are now wire types
// in `api/artifacts.ts`, fetched by real functions there. What was GAP-04 and
// GAP-05 is served — list, row, content, versions, diff and pin.

// The subagent timeline left this file when Phase 4 landed
// `GET /v1/tasks/{id}/timeline`: `TimelineLane`, `TimelineLaneState` and
// `TaskTimeline` are now wire types in `api/tasks.ts`, fetched by a real
// function there. What was GAP-09 is served — `subagent_span` records each
// lane from its spawn, so an *in-flight* lane is visible, which is what
// `agent_task_history` (written only at completion, with no start time) could
// never do.

// The per-run event log left this file when Phase 4 filled `event_log.task_id`
// and made `GET /v1/events/history` a task-filtered envelope: `RunEvent`,
// `RunEventTag` and `RunEventPage` are now wire-derived types in
// `api/run-events.ts`, fetched by `getRunEventLog` in `api/telemetry.ts`. What
// was GAP-10 is served — including the tool rows the socket could never
// attribute to a run, and surviving a restart, which a live ring cannot.

// ── Task actions the daemon rejects (GAP-06) ────────────────────────────────

export interface RerunResult {
  task_id: string;
  status: string;
  source_task_id: string;
}

/** GAP-06 — `apply_task_action` accepts only cancel/pause/resume. */
export function rerunTask(_taskId: string): Availability<RerunResult> {
  void _taskId;
  return unavailable("GAP-06");
}

/** GAP-06 — no way to promote a queued task; `POST /v1/tasks` never dispatches. */
export function startTaskNow(
  _taskId: string,
): Availability<{ task_id: string; status: string }> {
  void _taskId;
  return unavailable("GAP-06");
}

// Steering left this file when Phase 5 landed `POST /v1/tasks/{id}/steer`:
// `SteerResult` is a wire type in `api/tasks.ts`, pushed by a real `steerTask`
// there. What was GAP-02 is served — and *addressed*, which the `/steer ` chat
// prefix never could: it aims at the lane's sole running workflow, so a client
// holding a run id had no way to name it. The route answers `accepted` and
// `inbox_depth` synchronously instead of leaving the client to infer the
// outcome from a later `workflow_steered` frame.

// ── Follow-ups (GAP-03) ─────────────────────────────────────────────────────

export interface FollowupRecord {
  id: number;
  lane_key: string;
  kind: "followup" | "unprocessed_steering";
  content: string;
  source_task_id: string | null;
  status: "queued" | "running" | "done" | "cancelled";
  created_at: string;
  updated_at: string;
}

/** GAP-03 — storage and the `followup_queued` event exist; no routes do. */
export function listFollowups(
  _laneKey: string,
): Availability<FollowupRecord[]> {
  void _laneKey;
  return unavailable("GAP-03");
}

/** GAP-03 — the only writer is the model's own `queue_followup` tool. */
export function queueFollowup(
  _laneKey: string,
  _content: string,
  _sourceTaskId?: string,
): Availability<FollowupRecord> {
  void _laneKey;
  void _content;
  void _sourceTaskId;
  return unavailable("GAP-03");
}

/** GAP-03 — no cancel route either. */
export function cancelFollowup(
  _laneKey: string,
  _followupId: number,
): Availability<FollowupRecord> {
  void _laneKey;
  void _followupId;
  return unavailable("GAP-03");
}

// ── Daemon status detail (GAP-14) ───────────────────────────────────────────

export interface DaemonStatusDetail {
  started_at: string;
  uptime_secs: number;
  schema_version: number;
  data_dir: string;
  log_path: string;
  db_path: string;
}

/**
 * GAP-14 — `/v1/health` is four fields, `ConnectionInfo` has no `startedAt`,
 * and the migration count is compile-time only. Uptime, `Schema vNN` and
 * `Copy log path` all wait on this.
 */
export function getDaemonStatusDetail(): Availability<DaemonStatusDetail> {
  return unavailable("GAP-14");
}

// The tool catalog left this file in C7: `GET /v1/tools` is real, so
// `ToolCatalogEntry` is a wire type (`api/types.ts`) fetched by `api/tools.ts`.
// Its ADR-029 shape carried `denied: boolean` and `provider: string | null`;
// both are gone — availability is derived from the extension's `origin`, and
// there is no per-tool enable state anywhere in the system (S1, ADR-030 §8).
