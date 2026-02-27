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

/** Debounced + in-flight-guarded backfill.
 *  - Debounce: coalesces rapid terminal WS events into one fetch.
 *  - In-flight guard: if a fetch is running, new requests are deferred
 *    and a single trailing fetch runs after the current one finishes. */
let backfillTimer: ReturnType<typeof setTimeout> | null = null;
let backfillInFlight = false;
let backfillNeeded = false;

function scheduleBackfill(): void {
  if (backfillInFlight) {
    backfillNeeded = true;
    return;
  }
  if (backfillTimer !== null) return;
  backfillTimer = setTimeout(() => {
    backfillTimer = null;
    runBackfill();
  }, 200);
}

async function runBackfill(): Promise<void> {
  backfillInFlight = true;
  backfillNeeded = false;
  try {
    await loadTasks();
  } finally {
    backfillInFlight = false;
    if (backfillNeeded) {
      backfillNeeded = false;
      scheduleBackfill();
    }
  }
}

/** Fetch all tasks from REST and merge into the map (preserves WebSocket-derived data) */
export async function loadTasks(): Promise<void> {
  tasksLoading.set(true);
  try {
    const tasks = await getTasks();
    taskMap.update((existing) => {
      // Start with REST data as the source of truth
      const merged = new Map(tasks.map((t) => [t.id, t] as [string, Task]));
      // Merge in any WebSocket-derived tasks not present in REST response
      for (const [id, task] of existing) {
        if (!merged.has(id)) {
          merged.set(id, task);
        }
      }
      return merged;
    });
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
          // Merge outcome fields from WS event
          outcome_kind: latest.outcome_kind ?? existing.outcome_kind,
          artifact_count: latest.artifact_count ?? existing.artifact_count,
          // Build a minimal ParsedOutcome from WS fields when available
          outcome: latest.outcome_kind
            ? {
                ...(existing.outcome ?? {}),
                outcome_kind: latest.outcome_kind,
                outcome_summary: latest.outcome_summary ?? existing.outcome?.outcome_summary ?? null,
                artifact_count: latest.artifact_count ?? existing.outcome?.artifact_count ?? 0,
                artifacts: existing.outcome?.artifacts ?? [],
                no_artifact_reason: existing.outcome?.no_artifact_reason,
              }
            : existing.outcome,
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
          outcome_kind: latest.outcome_kind,
          artifact_count: latest.artifact_count,
          outcome: latest.outcome_kind
            ? {
                outcome_summary: latest.outcome_summary ?? null,
                outcome_kind: latest.outcome_kind,
                artifact_count: latest.artifact_count ?? 0,
                artifacts: [],
              }
            : undefined,
          created_by: "",
          source_lane: "",
          created_at: latest.ts,
          updated_at: latest.ts,
          completed_at: null,
        });
        // Terminal placeholder — backfill from REST to resolve "Unknown Task"
        if (latest.status === "completed" || latest.status === "failed") {
          scheduleBackfill();
        }
      }
      return new Map(map);
    });
  });
}
