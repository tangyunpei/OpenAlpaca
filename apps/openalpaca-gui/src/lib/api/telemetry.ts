/** `/v1/events/history` and `/v1/health`. */

import { apiFetch } from "../http";
import { ensureConnection } from "../connection";
import { runEventsFromLog, type RunEventPage } from "./run-events";
import type { EventLogRecord, HealthResponse } from "./types";

export interface EventHistoryQuery {
  limit?: number;
  /** Scope to one run — `event_log.task_id`, filled since migration 037. */
  taskId?: string;
  agentId?: string;
  /** Exact match on `event_type`; `tool_` does not select `tool_executed`. */
  eventType?: string;
  /** Exclusive upper bound on `id` — the `next_before` of the page before. */
  before?: number;
}

/**
 * `GET /v1/events/history` — always this envelope, with filters and without
 * (P20). Paging walks the autoincrement `id`, not the timestamp.
 */
export interface EventHistoryPage {
  events: EventLogRecord[];
  next_before: number | null;
}

/** `GET /v1/events/history` — server default 100, clamped at 1000. */
export async function getEventHistory(
  query: EventHistoryQuery = {},
  signal?: AbortSignal,
): Promise<EventHistoryPage> {
  return await apiFetch<EventHistoryPage>("/v1/events/history", {
    query: {
      limit: query.limit,
      task_id: query.taskId,
      agent_id: query.agentId,
      event_type: query.eventType,
      before: query.before,
    },
    signal,
  });
}

/**
 * One run's event log (GAP-10, closed).
 *
 * Over-fetches relative to the six rows the run detail draws: a page can still
 * carry `dag_node_status` rows written before P9 deleted the emitter (T53), and
 * the projection drops them as duplicates of `subagent_span`, so asking for
 * exactly six could answer with none.
 */
export async function getRunEventLog(
  taskId: string,
  limit = 100,
  before?: number,
  signal?: AbortSignal,
): Promise<RunEventPage> {
  const page = await getEventHistory({ taskId, limit, before }, signal);
  return {
    events: runEventsFromLog(page.events, taskId),
    next_before: page.next_before,
  };
}

/**
 * `GET /v1/health` — unauthenticated, so it bypasses `apiFetch`. Used for the
 * instance-id guard and the Connection panel's liveness dot.
 */
export async function getHealth(signal?: AbortSignal): Promise<HealthResponse> {
  const info = await ensureConnection();
  const response = await fetch(`${info.baseUrl}/v1/health`, { signal });
  if (!response.ok) {
    throw new Error(
      `Health check failed: ${response.status} ${response.statusText}`,
    );
  }
  return (await response.json()) as HealthResponse;
}
