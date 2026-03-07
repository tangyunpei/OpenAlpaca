/**
 * REST API client for task endpoints.
 */

import { ensureConnection } from "./connection";
import type {
  Task,
  TaskDetailResponse,
  CreateTaskRequest,
  CreateTaskResponse,
  TaskActionResponse,
} from "../types";

export interface ListTasksQuery {
  created_by?: string;
  status?: string;
  limit?: number;
}

/** GET /v1/tasks */
export async function getTasks(query?: ListTasksQuery): Promise<Task[]> {
  const conn = await ensureConnection();
  const params = new URLSearchParams();
  if (query?.created_by) params.set("created_by", query.created_by);
  if (query?.status) params.set("status", query.status);
  if (query?.limit) params.set("limit", String(query.limit));
  const qs = params.toString();

  const response = await fetch(
    `${conn.baseUrl}/v1/tasks${qs ? `?${qs}` : ""}`,
    { headers: { Authorization: `Bearer ${conn.token}` } },
  );

  if (!response.ok) {
    throw new Error(`Failed to fetch tasks: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/tasks/{id} — returns task + assignments */
export async function getTask(id: string): Promise<TaskDetailResponse> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/tasks/${id}`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    throw new Error(`Failed to fetch task ${id}: ${response.statusText}`);
  }
  return await response.json();
}

/** POST /v1/tasks */
export async function createTask(req: CreateTaskRequest): Promise<CreateTaskResponse> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/tasks`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify(req),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error || `Failed to create task: ${response.statusText}`);
  }
  return await response.json();
}

/** POST /v1/tasks/{id}/action — action: "cancel" | "pause" | "resume" */
export async function performTaskAction(
  id: string,
  action: "cancel" | "pause" | "resume",
): Promise<TaskActionResponse> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/tasks/${id}/action`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify({ action }),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error || `Task action ${action} failed: ${response.statusText}`);
  }
  return await response.json();
}
