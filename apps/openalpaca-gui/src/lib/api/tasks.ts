/** `/v1/tasks*`. */

import { apiFetch, ApiError } from "../http";
import type {
  CreateTaskRequest,
  CreateTaskResponse,
  Task,
  TaskAction,
  TaskActionResponse,
  TaskDetailResponse,
} from "./types";

export interface ListTasksQuery {
  /** `active` is a special list mode, not a `TaskStatus` value. */
  status?: "active" | Task["status"];
  createdBy?: string;
  limit?: number;
}

/** `GET /v1/tasks` — a bare array of `Task` rows carrying `outcome` and `cost_usd`. */
export async function listTasks(
  query: ListTasksQuery = {},
  signal?: AbortSignal,
): Promise<Task[]> {
  return await apiFetch<Task[]>("/v1/tasks", {
    query: {
      status: query.status,
      created_by: query.createdBy,
      limit: query.limit,
    },
    signal,
  });
}

/** `GET /v1/tasks/{id}` — a different shape from a list row. */
export async function getTask(
  id: string,
  signal?: AbortSignal,
): Promise<TaskDetailResponse> {
  return await apiFetch<TaskDetailResponse>(
    `/v1/tasks/${encodeURIComponent(id)}`,
    { signal },
  );
}

/** `POST /v1/tasks` — persists a row and a lane; it does **not** dispatch a workflow. */
export async function createTask(
  req: CreateTaskRequest,
): Promise<CreateTaskResponse> {
  return await apiFetch<CreateTaskResponse>("/v1/tasks", {
    method: "POST",
    body: req,
  });
}

// ── The run timeline (`GET /v1/tasks/{id}/timeline`) ────────────────────────

/**
 * A lane's state as the daemon *reports* it, which is not always the state it
 * stored: a span left running on a finished run comes back `cancelled` with
 * `detail: "interrupted"`, and a lane whose agent is waiting on a tool
 * confirmation comes back `blocked` for exactly as long as that is true.
 */
export type TimelineLaneState =
  "running" | "done" | "failed" | "blocked" | "cancelled";

export interface TimelineLane {
  /** The span id — the spawn's `node_id`. */
  lane_id: string;
  /** `review·3`; unique within the run, and the key the swimlanes use. */
  label: string;
  template_id: string;
  agent_instance_id: string;
  started_at: string;
  ended_at: string | null;
  state: TimelineLaneState;
  detail: string | null;
  /** Absent today: nothing counts steps inside a subagent's loop. */
  steps_current?: number;
  steps_total?: number;
}

export interface TaskTimeline {
  task_id: string;
  started_at: string;
  /** The daemon's clock at the read — the axis's right edge for a live run. */
  now: string;
  completed_at: string | null;
  lanes: TimelineLane[];
}

/** `GET /v1/tasks/{id}/timeline` — the Parallel work swimlanes. */
export async function getTaskTimeline(
  id: string,
  signal?: AbortSignal,
): Promise<TaskTimeline> {
  return await apiFetch<TaskTimeline>(
    `/v1/tasks/${encodeURIComponent(id)}/timeline`,
    { signal },
  );
}

/** `POST /v1/tasks/{id}/action` — 409 on an illegal transition. `rerun`/`start` are GAP-06. */
export async function performTaskAction(
  id: string,
  action: TaskAction,
): Promise<TaskActionResponse> {
  return await apiFetch<TaskActionResponse>(
    `/v1/tasks/${encodeURIComponent(id)}/action`,
    {
      method: "POST",
      body: { action },
    },
  );
}

// ── Steering (`POST /v1/tasks/{id}/steer`) ──────────────────────────────────

/**
 * What the steering queue accepted.
 *
 * `accepted` is not a promise the workflow *read* the message: the rail drains
 * at the run's next round boundary, so `inbox_depth` — the queue depth after
 * this push — is the only acknowledgement the daemon can honestly give.
 */
export interface SteerResult {
  task_id: string;
  accepted: boolean;
  inbox_depth: number;
  lane_key: string;
}

/**
 * `POST /v1/tasks/{id}/steer` — inject a message into one *running* run.
 *
 * Addressed at the run, not at a lane: the daemon reads the lane from the
 * run's own `source_lane`. `workspacePath` is optional — omitted, the daemon
 * uses the run's own project, so a message the workflow never drained re-enters
 * as a follow-up scoped where the run was.
 *
 * Throws `ApiError` with the daemon's code: `STEERING_INBOX_FULL` /
 * `TASK_NOT_STEERABLE` (409), `STEERING_DISABLED` (503), `EMPTY_MESSAGE` (400),
 * `NOT_FOUND` (404). Render them through {@link steerErrorMessage}.
 */
export async function steerTask(
  id: string,
  message: string,
  workspacePath?: string,
): Promise<SteerResult> {
  return await apiFetch<SteerResult>(
    `/v1/tasks/${encodeURIComponent(id)}/steer`,
    {
      method: "POST",
      body: {
        message,
        ...(workspacePath === undefined
          ? {}
          : { workspace_path: workspacePath }),
      },
    },
  );
}

/**
 * The sentence a failed steer shows the user.
 *
 * Every code the route can answer with gets its own line — a steer that did
 * not land must never look like one that did, and "Request failed with status
 * 409" tells nobody what to do next. An unrecognised code falls back to the
 * daemon's own message rather than to a shrug.
 *
 * `runOnScreen` disambiguates the one code the daemon cannot: `NOT_FOUND`
 * covers both a run that never existed and one started from another channel
 * (the route is owner-scoped, and answers the two identically on purpose, so
 * it cannot leak which ids exist). The client is the side that knows whether
 * it is holding the row — pass `true` when the run is in the list on screen,
 * and "gone" is not claimed about a run whose progress is still updating.
 * With the `steerable` hint (R40) the control is disabled before it comes to
 * this, so this is the belt to that braces.
 */
export function steerErrorMessage(error: unknown, runOnScreen = false): string {
  if (error instanceof ApiError) {
    switch (error.code) {
      case "STEERING_INBOX_FULL":
        return "The run's steering queue is full — it has not caught up with earlier messages yet.";
      case "TASK_NOT_STEERABLE":
        return "That run is no longer running — nothing was queued.";
      case "STEERING_DISABLED":
        return "Steering is disabled on this daemon — nothing was queued.";
      case "EMPTY_MESSAGE":
        return "A steering message cannot be empty.";
      case "NOT_FOUND":
        return runOnScreen
          ? "This run can't be steered from here — it was started somewhere else."
          : "That run no longer exists — nothing was queued.";
      default:
        break;
    }
    if (error.isTransport)
      return "Could not reach the daemon — nothing was queued.";
    return error.message;
  }
  if (error instanceof Error) return error.message;
  return "Could not steer that run.";
}
