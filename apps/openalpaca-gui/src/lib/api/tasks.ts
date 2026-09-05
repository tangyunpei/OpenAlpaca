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

/** `GET /v1/tasks` — a bare array, with `assigned_agents` and `outcome` injected. */
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
