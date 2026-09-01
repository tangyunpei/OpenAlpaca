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
 * does the `/steer …` send itself, `views/work/EventLogSection` shows the live
 * socket instead of a per-run history, and the lane key is learned from the
 * first chat reply. The adapters below stay as the shape those routes take.
 */

import { sendChatMessage } from "../chat-stream";
import { unavailable, type Availability } from "../unavailable";
import type { ChatSendResponse } from "./types";

// ── Artifacts (GAP-04 / GAP-05) ─────────────────────────────────────────────

export type ArtifactKind =
  | "markdown"
  | "code"
  | "terminal"
  | "table"
  | "plan"
  | "image"
  | "html"
  | "binary";

/** The proposed `Artifact` resource. */
export interface Artifact {
  id: string;
  name: string;
  kind: ArtifactKind;
  mime_type: string;
  size_bytes: number;
  task_id: string | null;
  task_title: string | null;
  agent_id: string | null;
  agent_template_id: string | null;
  version: number;
  version_count: number;
  summary: string | null;
  metadata: Record<string, unknown> | null;
  created_at: string;
  updated_at: string;
}

export interface ArtifactListPage {
  artifacts: Artifact[];
  total: number;
}

export interface ArtifactVersion {
  version: number;
  note: string;
  author_agent_id: string | null;
  created_at: string;
  size_bytes: number;
  added_lines: number | null;
  removed_lines: number | null;
}

export interface ArtifactDiff {
  from: number;
  to: number;
  added_lines: number;
  removed_lines: number;
  format: "unified";
  patch: string;
}

export interface ListArtifactsQuery {
  taskId?: string;
  kind?: ArtifactKind;
  limit?: number;
  offset?: number;
}

/** GAP-04 — there is no `/v1/artifacts` list route and `FileAsset` has no run attribution. */
export function listArtifacts(
  _query: ListArtifactsQuery = {},
): Availability<ArtifactListPage> {
  void _query;
  return unavailable("GAP-04");
}

/** GAP-05 — nothing versioned exists in storage. */
export function listArtifactVersions(
  _artifactId: string,
): Availability<ArtifactVersion[]> {
  void _artifactId;
  return unavailable("GAP-05");
}

/** GAP-05 — no diff endpoint, and no prior content to diff against. */
export function getArtifactDiff(
  _artifactId: string,
  _from: number,
  _to: number,
): Availability<ArtifactDiff> {
  void _artifactId;
  void _from;
  void _to;
  return unavailable("GAP-05");
}

// ── Subagent timeline (GAP-09) ──────────────────────────────────────────────

export type TimelineLaneState =
  "running" | "done" | "failed" | "blocked" | "cancelled";

export interface TimelineLane {
  lane_id: string;
  label: string;
  template_id: string;
  agent_instance_id: string;
  started_at: string;
  ended_at: string | null;
  state: TimelineLaneState;
  detail: string | null;
  steps_current?: number;
  steps_total?: number;
}

export interface TaskTimeline {
  task_id: string;
  started_at: string;
  now: string;
  completed_at: string | null;
  lanes: TimelineLane[];
}

/**
 * GAP-09 — the Parallel work swimlanes. `agent_task_history` has no
 * `started_at`, so even finished spans cannot be placed on an axis, and
 * `dag_node_status` is dead under the lead-agent topology.
 */
export function getTaskTimeline(_taskId: string): Availability<TaskTimeline> {
  void _taskId;
  return unavailable("GAP-09");
}

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

// ── Tool / skill catalog (GAP-18) ───────────────────────────────────────────

export interface ToolCatalogEntry {
  name: string;
  description: string;
  source: "builtin" | "mcp" | "plugin";
  provider: string | null;
  requires_confirmation: boolean;
  denied: boolean;
  invocations_today: number;
}

/** GAP-18 — the Settings → Skills rows are really tools, and nothing lists them. */
export function listToolCatalog(): Availability<ToolCatalogEntry[]> {
  return unavailable("GAP-18");
}

// ── Identity (GAP-16) ───────────────────────────────────────────────────────

export interface Identity {
  user_id: string;
  default_lane_key: string;
  sources: string[];
}

/**
 * GAP-16 — no `/v1/me`. The lane key is instead learned from the `lane_key`
 * echoed by `GET /v1/chat/history` or `POST /v1/chat`, which is why
 * `useChatHistory` omits the param on first load.
 */
export function getIdentity(): Availability<Identity> {
  return unavailable("GAP-16");
}
