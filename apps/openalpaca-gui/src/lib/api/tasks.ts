/** `/v1/tasks*`. */

import { apiFetch, ApiError } from "../http";
import { workspaceHeader } from "../workspace-header";
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

/**
 * `POST /v1/tasks/{id}/action` — 409 on an illegal transition.
 *
 * Also carries `start` (D5), which is not a transition: it dispatches a stored
 * row under its own id, so the response's `task_id` is the one you sent. Its
 * refusals are {@link launchErrorMessage}'s, not a transition's.
 */
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

// ── Re-run and start (`POST /v1/tasks/{id}/rerun`, action `start`) ───────────

/**
 * What a re-run produced. `task_id` is a run you have not seen: a re-run is a
 * *second* run of the same goal, and the finished one keeps its row and its
 * result — which is the thing you are re-running against. `source_task_id` is
 * the id you asked about.
 */
export interface RerunResult {
  /** The new run. */
  task_id: string;
  /** The run it was copied from — the id in the request. */
  source_task_id: string;
  title: string;
  /** `queued`, or `running` if the daemon's background half got there first. */
  status: string;
}

/**
 * `POST /v1/tasks/{id}/rerun` — dispatch a new run from a finished one's goal.
 *
 * Throws `ApiError` with the daemon's code: `TASK_NOT_TERMINAL` (409),
 * `TASK_NOT_RERUNNABLE` (422), `DISPATCH_FAILED` (503), `NOT_FOUND` (404).
 * Render them through {@link launchErrorMessage}.
 */
export async function rerunTask(id: string): Promise<RerunResult> {
  return await apiFetch<RerunResult>(
    `/v1/tasks/${encodeURIComponent(id)}/rerun`,
    { method: "POST" },
  );
}

/**
 * `POST /v1/tasks/{id}/action {"action":"start"}` — run a queued row now.
 *
 * D5: the id does not change, so the response is the same `{task_id, status}`
 * the other actions answer with and every reference you hold stays valid.
 *
 * Only a run that has not finished can be started — it is re-launched in
 * place, so a finished one would lose its result (`409 TASK_NOT_STARTABLE`;
 * {@link rerunTask} is the verb for that case).
 */
export async function startTaskNow(id: string): Promise<TaskActionResponse> {
  return await performTaskAction(id, "start");
}

/**
 * The sentence a failed re-run or start shows the user.
 *
 * One line per code the two verbs can answer with, because "Request failed
 * with status 409" tells nobody what to do next, and a run that did *not*
 * start must never read like one that did. Two codes differ only by verb —
 * `TASK_NOT_RERUNNABLE` and `TASK_NOT_DISPATCHABLE` — so the daemon says which
 * without the client having to remember what it asked. `TASK_NOT_STARTABLE` is
 * a different refusal at a different status (409, R43): the run is over, and
 * `Re-run` is the way to run it again. §5.6c's three resume refusals are here
 * for the same reason — `Resume` on an interrupted run is a launch verb, not a
 * transition, and `Re-run` is what every one of them points back at.
 *
 * An unrecognised code falls back to the daemon's own message rather than to a
 * shrug.
 */
export function launchErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    switch (error.code) {
      case "TASK_NOT_TERMINAL":
        return "That run hasn't finished — steer or cancel it instead of re-running it.";
      case "TASK_ALREADY_RUNNING":
        return "That run is already running.";
      case "TASK_NOT_RERUNNABLE":
        return "That run has no description to re-dispatch — there is nothing to re-run.";
      case "TASK_NOT_DISPATCHABLE":
        return "That task has no description to dispatch — there is nothing to start.";
      case "TASK_NOT_STARTABLE":
        return "That run has already finished — use Re-run, which keeps its result.";
      case "DISPATCH_FAILED":
        return "No agent is free to lead a run right now — try again shortly.";
      case "NOT_FOUND":
        return "That run no longer exists — nothing was started.";
      // §5.6c's replay resume. It is experimental and off by default, so the
      // first of these has to name the switch: "resume is disabled" with no
      // key is a dead end for whoever reads the toast.
      case "RESUME_DISABLED":
        return "Replay resume is experimental and off — set resume_enabled under [orchestrator.routing] in daemon.toml, or use Re-run.";
      case "TASK_NOT_RESUMABLE":
        return "Only an interrupted run can be resumed — use Re-run for this one.";
      case "RESUME_LOG_MISSING":
        return "That run's transcript is gone, so there is nothing to resume from — use Re-run.";
      default:
        break;
    }
    if (error.isTransport)
      return "Could not reach the daemon — nothing was started.";
    return error.message;
  }
  if (error instanceof Error) return error.message;
  return "Could not start that run.";
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
 * The project rides in `x-workspace-path`, never in the body: one resolver for
 * every route that takes a project from a client (R22), and the daemon now
 * refuses a body `workspace_path` with `400 WORKSPACE_PATH_IN_BODY` rather than
 * storing an unresolved path.
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
      body: { message },
      ...(workspacePath === undefined
        ? {}
        : { headers: workspaceHeader(workspacePath) }),
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
