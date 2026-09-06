/** `/v1/tasks*`. */

import { apiFetch } from "../http";
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
