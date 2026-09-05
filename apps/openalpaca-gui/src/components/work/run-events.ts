/**
 * The run detail's Event log, assembled from what actually exists (GAP-10).
 *
 * There is no per-run event history: `event_log` has no `task_id` column and
 * `GET /v1/events/history` filters by `agent_id` only. What *does* carry a
 * `task_id` is the live socket — five of the `ServerEvent` variants — so the
 * card shows those, scoped to this run, and says plainly that it is showing
 * this session only.
 *
 * The two variants the design's `tool` tag would come from (`tool_executed`,
 * `tool_confirmation_requested`) carry an `agent_id` and no `task_id`, so they
 * cannot be attributed to a run. Guessing which run a tool call belonged to
 * would be fabrication; they are dropped, and that omission is exactly what
 * GAP-10 asks the daemon to fix.
 */

import type { RunEvent } from "@/lib/api/unbacked";
import type { ServerEvent } from "@/lib/events";

/** One socket event, or `null` when it does not belong to this run. */
function toRunEvent(event: ServerEvent, taskId: string): RunEvent | null {
  switch (event.type) {
    case "workflow_started":
      if (event.task_id !== taskId) return null;
      return {
        id: event._id,
        task_id: taskId,
        tag: "run",
        text:
          event.title === "" ? "workflow started" : `started · ${event.title}`,
        at: event.ts,
      };
    case "workflow_progress":
      if (event.task_id !== taskId) return null;
      return {
        id: event._id,
        task_id: taskId,
        tag: "run",
        text: event.message,
        at: event.ts,
      };
    case "workflow_steered":
      if (event.task_id !== taskId) return null;
      return {
        id: event._id,
        task_id: taskId,
        tag: "steer",
        text: "steering message delivered",
        at: event.ts,
      };
    case "task_status": {
      if (event.task_id !== taskId) return null;
      const steps =
        event.progress_total !== null && event.progress_total > 0
          ? ` · ${event.progress_current ?? 0}/${event.progress_total}`
          : "";
      return {
        id: event._id,
        task_id: taskId,
        tag: "run",
        text: `status ${event.status}${steps}`,
        at: event.ts,
      };
    }
    case "dag_node_status":
      if (event.task_id !== taskId) return null;
      return {
        id: event._id,
        task_id: taskId,
        tag: "spawn",
        text: `${event.node_title} · ${event.status}`,
        at: event.ts,
      };
    default:
      return null;
  }
}

/** Newest first, capped — the design shows up to six rows (§5.2). */
export function runEventsFromRing(
  events: readonly ServerEvent[],
  taskId: string,
  limit = 6,
): RunEvent[] {
  const rows: RunEvent[] = [];
  for (const event of events) {
    const row = toRunEvent(event, taskId);
    if (row !== null) rows.push(row);
  }
  rows.sort((a, b) => b.id - a.id);
  return rows.slice(0, limit);
}
