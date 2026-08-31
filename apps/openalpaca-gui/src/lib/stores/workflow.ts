/**
 * Workflow activity store — collects Routing V2 workflow lifecycle events
 * (workflow_started / workflow_steered / workflow_progress / followup_queued)
 * from the WebSocket event stream for rendering alongside tasks.
 */

import { writable } from "svelte/store";
import { events, type ServerEvent } from "../daemon";

/** A single workflow activity entry (per-task or lane-level) */
export interface WorkflowActivityEntry {
  kind: "started" | "steered" | "progress" | "followup_queued";
  /** Task this entry belongs to (absent for lane-level followup_queued) */
  task_id: string | null;
  lane_key: string;
  /** Human-readable line: title, progress message, or a short label */
  message: string;
  ts: string;
  _id: number;
}

const MAX_ENTRIES_PER_TASK = 20;
const MAX_FEED_ENTRIES = 50;

/** Per-task activity (newest first) — keyed by task_id */
export const workflowActivity = writable<Map<string, WorkflowActivityEntry[]>>(new Map());

/** Flat feed of all workflow events (newest first), including lane-level follow-ups */
export const workflowFeed = writable<WorkflowActivityEntry[]>([]);

function toEntry(event: ServerEvent): WorkflowActivityEntry | null {
  switch (event.type) {
    case "workflow_started":
      return {
        kind: "started",
        task_id: event.task_id,
        lane_key: event.lane_key,
        message: event.title,
        ts: event.ts,
        _id: event._id,
      };
    case "workflow_steered":
      return {
        kind: "steered",
        task_id: event.task_id,
        lane_key: event.lane_key,
        message: "Steering message accepted",
        ts: event.ts,
        _id: event._id,
      };
    case "workflow_progress":
      return {
        kind: "progress",
        task_id: event.task_id,
        lane_key: event.lane_key,
        message: event.message,
        ts: event.ts,
        _id: event._id,
      };
    case "followup_queued":
      return {
        kind: "followup_queued",
        task_id: null,
        lane_key: event.lane_key,
        message:
          event.kind === "unprocessed_steering"
            ? "Unprocessed steering queued as follow-up"
            : "Follow-up queued",
        ts: event.ts,
        _id: event._id,
      };
    default:
      return null;
  }
}

/** Subscribe to WebSocket events and collect workflow activity.
 *  Returns an unsubscribe function. */
export function subscribeToWorkflowEvents(): () => void {
  return events.subscribe(($events) => {
    if ($events.length === 0) return;
    const entry = toEntry($events[0] as ServerEvent);
    if (!entry) return;

    workflowFeed.update((feed) => {
      // Guard against duplicate delivery of the same event
      if (feed.length > 0 && feed[0]._id === entry._id) return feed;
      return [entry, ...feed].slice(0, MAX_FEED_ENTRIES);
    });

    if (entry.task_id) {
      workflowActivity.update((map) => {
        const existing = map.get(entry.task_id!) ?? [];
        if (existing.length > 0 && existing[0]._id === entry._id) return map;
        map.set(entry.task_id!, [entry, ...existing].slice(0, MAX_ENTRIES_PER_TASK));
        return new Map(map);
      });
    }
  });
}

/** Clear all collected workflow activity */
export function clearWorkflowActivity(): void {
  workflowActivity.set(new Map());
  workflowFeed.set([]);
}
