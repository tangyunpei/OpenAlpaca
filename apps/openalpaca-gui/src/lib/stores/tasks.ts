/**
 * Reactive task store — combines REST fetches with real-time WebSocket updates.
 */

import { writable, derived, type Readable } from "svelte/store";
import { events, type ServerEvent } from "../daemon";
import { getTasks, getTask } from "../api/tasks";
import type { Task, TaskDetailResponse } from "../types";

/** Internal Map store for O(1) lookup by task_id */
const taskMap = writable<Map<string, Task>>(new Map());

/** Sorted task list (most recent first) */
export const taskList: Readable<Task[]> = derived(taskMap, ($map) =>
  Array.from($map.values()).sort(
    (a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime(),
  ),
);

/** Active (non-terminal) tasks */
export const activeTasks: Readable<Task[]> = derived(taskList, ($list) =>
  $list.filter((t) => t.status === "queued" || t.status === "running" || t.status === "paused"),
);

/** Completed/terminal tasks */
export const completedTasks: Readable<Task[]> = derived(taskList, ($list) =>
  $list.filter((t) => t.status === "completed" || t.status === "failed" || t.status === "cancelled"),
);

/** Currently selected task detail (fetched on demand) */
export const selectedTaskDetail = writable<TaskDetailResponse | null>(null);

/** Loading state */
export const tasksLoading = writable(false);

/** Fetch all tasks from REST and populate the map */
export async function loadTasks(): Promise<void> {
  tasksLoading.set(true);
  try {
    const tasks = await getTasks();
    taskMap.set(new Map(tasks.map((t) => [t.id, t])));
  } catch (e) {
    console.error("[tasks-store] Failed to load tasks:", e);
  } finally {
    tasksLoading.set(false);
  }
}

/** Fetch full task detail + assignments */
export async function loadTaskDetail(id: string): Promise<void> {
  try {
    const detail = await getTask(id);
    selectedTaskDetail.set(detail);
    // Also update the task in the map
    taskMap.update((map) => {
      map.set(detail.task.id, detail.task);
      return new Map(map);
    });
  } catch (e) {
    console.error(`[tasks-store] Failed to load task detail ${id}:`, e);
  }
}

/** Subscribe to WebSocket events and merge task_status into the map.
 *  Returns an unsubscribe function. */
export function subscribeToTaskEvents(): () => void {
  return events.subscribe(($events) => {
    if ($events.length === 0) return;
    const latest = $events[0] as ServerEvent;
    if (latest.type !== "task_status") return;

    taskMap.update((map) => {
      const existing = map.get(latest.task_id);
      if (existing) {
        // Merge — skip overwriting title with empty string
        map.set(latest.task_id, {
          ...existing,
          status: latest.status as Task["status"],
          progress_current: latest.progress_current ?? existing.progress_current,
          progress_total: latest.progress_total ?? existing.progress_total,
          result_summary: latest.result_summary ?? existing.result_summary,
          ...(latest.title ? { title: latest.title } : {}),
        });
      } else {
        // Create placeholder for unknown task
        map.set(latest.task_id, {
          id: latest.task_id,
          title: latest.title || "Unknown Task",
          description: null,
          status: latest.status as Task["status"],
          priority: 0,
          progress_current: latest.progress_current,
          progress_total: latest.progress_total,
          result_summary: latest.result_summary,
          created_by: "",
          source_lane: "",
          created_at: latest.ts,
          updated_at: latest.ts,
          completed_at: null,
        });
      }
      return new Map(map);
    });
  });
}
