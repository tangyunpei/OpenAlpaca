/**
 * Adapters for the design surfaces the daemon cannot serve (API_MAP §3).
 *
 * Each function returns an `Availability<T>` whose `T` is the shape the
 * *proposed* endpoint would return, so views can be written against the real
 * contract today and lose nothing but the `unavailable` branch when the route
 * lands. Nothing here fabricates rows.
 *
 * Two exceptions do real work because a genuine workaround exists:
 *   `steerWorkflow` — posts `/steer …` down the chat channel (GAP-02)
 *   `queueFollowupViaChat` — has no workaround, so it stays unavailable
 *
 * Not every adapter has a `hooks/useUnbacked` wrapper: where a surface has a
 * working alternative rather than an empty state, the view handles the gap at
 * the point of use — `components/work/run-actions` disables `Start now`,
 * `Re-run` and `Queue follow-up` and names the missing route, `useChatSession`
 * does the `/steer …` send itself, and `views/work/EventLogSection` shows the
 * live socket instead of a per-run history. The adapters below stay as the
 * shape those routes take.
 */

import { sendChatMessage } from "../chat-stream";
import { unavailable, type Availability } from "../unavailable";
import type { ChatSendResponse } from "./types";

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

// ── Per-run event log (GAP-10) ──────────────────────────────────────────────

export type RunEventTag = "tool" | "steer" | "artifact" | "spawn" | "run";

export interface RunEvent {
  id: number;
  task_id: string;
  tag: RunEventTag;
  text: string;
  at: string;
}

export interface RunEventPage {
  events: RunEvent[];
  next_before: number | null;
}

/** GAP-10 — `event_log` has no `task_id` column, so a run-scoped log is impossible. */
export function getRunEventLog(
  _taskId: string,
  _limit = 200,
): Availability<RunEventPage> {
  void _taskId;
  void _limit;
  return unavailable("GAP-10");
}

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

// ── Steering (GAP-02) ───────────────────────────────────────────────────────

export interface SteerResult {
  /** The chat stream the steer went down; there is no deterministic ack. */
  response: ChatSendResponse;
}

/**
 * GAP-02 — the only steering channel is the chat text stream: the orchestrator
 * strips a literal `"/steer "` prefix and targets the *lane's* active workflow.
 * It takes no `task_id`, so this cannot address a specific run, and there is no
 * accepted/rejected answer — the acknowledgement arrives later as a
 * `workflow_steered` WS event, if at all.
 */
export async function steerWorkflow(message: string): Promise<SteerResult> {
  const response = await sendChatMessage({ content: `/steer ${message}` });
  return { response };
}

/** The note to render beside the Steer control. */
export const STEERING_GAP = unavailable("GAP-02");

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
