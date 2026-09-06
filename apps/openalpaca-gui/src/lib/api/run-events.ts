/**
 * The run detail's Event log, read from the daemon (GAP-10, closed).
 *
 * `event_log.task_id` and `GET /v1/events/history?task_id=` are real, so this
 * card no longer scrapes the live socket for the handful of frames that
 * happened to carry a run: the server answers with *this run's* rows, tool
 * calls included, and they survive a restart. What is left here is the
 * projection — one persisted row onto the design's `LogTag` palette (§3.28).
 *
 * Two rules hold the projection honest:
 *
 * - **Nothing is invented.** A row whose `detail` is missing the field a
 *   sentence would name falls back to the raw `event_type` rather than to a
 *   plausible-looking phrase.
 * - **Nothing is silently dropped, except a known duplicate.**
 *   `dag_node_status` fires for the same moment as `subagent_span` (P9 /
 *   Phase 8 deletes the emitter), so rendering both would fill the card with
 *   pairs of the same transition; `subagent_span` is the single source of
 *   `spawn` rows.
 */

import type { EventLogRecord } from "./types";

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
  /** Feed back as `?before=` for the older page; `null` at the end. */
  next_before: number | null;
}

/**
 * `event_type` values a live `ServerEvent` can carry a `task_id` for and
 * that are *not* already covered by the `["tasks"]` prefix group (i.e. not
 * `task_status` / `workflow_started` / `workflow_progress` /
 * `workflow_steered`) or dropped as a duplicate (`dag_node_status`).
 * `tool_auto_approved` is excluded too: it renders on this card but has no
 * `ServerEvent` variant, so nothing live ever needs to invalidate for it.
 *
 * `lib/query-client.ts`'s `invalidationKeysFor` must refresh
 * `qk.tasks.eventLog(task_id)` for every one of these when the frame names a
 * run — `query-client.test.ts` iterates this exact list, so a type added
 * here without its own invalidation arm fails that test instead of leaving
 * the run-detail card stale until an unrelated frame happens to land.
 */
export const RUN_LOG_EVENT_TYPES = [
  "tool_executed",
  "security_violation",
  "circuit_breaker_tripped",
  "llm_call_completed",
  "subagent_span",
  "artifact_written",
  "tool_confirmation_requested",
] as const;

/** `event_type` → the design's five tones. */
function tagFor(eventType: string): RunEventTag {
  switch (eventType) {
    case "tool_executed":
    case "tool_confirmation_requested":
    case "tool_auto_approved":
    case "security_violation":
    case "circuit_breaker_tripped":
      return "tool";
    case "workflow_steered":
      return "steer";
    case "artifact_written":
      return "artifact";
    case "subagent_span":
      return "spawn";
    default:
      return "run";
  }
}

function str(detail: Record<string, unknown>, key: string): string | null {
  const value = detail[key];
  return typeof value === "string" && value !== "" ? value : null;
}

function num(detail: Record<string, unknown>, key: string): number | null {
  const value = detail[key];
  return typeof value === "number" ? value : null;
}

/** The row's own sentence, or the bare `event_type` when the blob cannot fill one. */
function textFor(record: EventLogRecord): string {
  const detail =
    typeof record.detail === "object" && record.detail !== null
      ? (record.detail as Record<string, unknown>)
      : {};

  switch (record.event_type) {
    case "task_status": {
      const status = str(detail, "status");
      if (status === null) return record.event_type;
      const total = num(detail, "progress_total");
      const steps =
        total !== null && total > 0
          ? ` · ${num(detail, "progress_current") ?? 0}/${total}`
          : "";
      return `status ${status}${steps}`;
    }
    case "workflow_started": {
      const title = str(detail, "title");
      return title === null ? "workflow started" : `started · ${title}`;
    }
    case "workflow_progress":
      return str(detail, "message") ?? record.event_type;
    case "workflow_steered":
      return "steering message delivered";
    case "artifact_written": {
      const name = str(detail, "name");
      const version = num(detail, "version");
      if (name === null) return record.event_type;
      return version === null ? name : `${name} · v${version}`;
    }
    case "subagent_span": {
      const label = str(detail, "label");
      const state = str(detail, "state");
      if (label === null || state === null) return record.event_type;
      return `${label} · ${state}`;
    }
    case "tool_executed": {
      const tool = str(detail, "tool_name");
      if (tool === null) return record.event_type;
      return detail["success"] === false ? `${tool} · failed` : tool;
    }
    case "tool_confirmation_requested": {
      const tool = str(detail, "tool_name");
      return tool === null ? record.event_type : `${tool} · awaiting approval`;
    }
    case "tool_auto_approved": {
      const tool = str(detail, "tool_name");
      return tool === null ? record.event_type : `${tool} · auto-approved`;
    }
    case "security_violation": {
      const tool = str(detail, "tool_name");
      const reason = str(detail, "reason");
      if (tool === null) return record.event_type;
      return reason === null ? `${tool} · denied` : `${tool} · ${reason}`;
    }
    case "circuit_breaker_tripped": {
      const tool = str(detail, "tool_name");
      const failures = num(detail, "consecutive_failures");
      if (tool === null) return record.event_type;
      return failures === null
        ? `${tool} · circuit open`
        : `${tool} · circuit open after ${failures} failures`;
    }
    case "llm_call_completed": {
      const model = str(detail, "model");
      if (model === null) return record.event_type;
      const input = num(detail, "input_tokens");
      const output = num(detail, "output_tokens");
      return input === null || output === null
        ? model
        : `${model} · ${input}/${output} tokens`;
    }
    // A frame this build does not have a sentence for still shows, named by
    // what the daemon called it — better a bare `event_type` than a hidden row.
    default:
      return record.event_type;
  }
}

/** One page of persisted rows, projected onto the design's log rows. */
export function runEventsFromLog(
  records: readonly EventLogRecord[],
  taskId: string,
): RunEvent[] {
  const rows: RunEvent[] = [];
  for (const record of records) {
    // The server filters by run; this guards a caller that hands over a
    // mixed page rather than trusting the label on the box.
    if (record.task_id !== taskId) continue;
    if (record.event_type === "dag_node_status") continue;
    rows.push({
      id: record.id,
      task_id: taskId,
      tag: tagFor(record.event_type),
      text: textFor(record),
      at: record.timestamp,
    });
  }
  return rows;
}
