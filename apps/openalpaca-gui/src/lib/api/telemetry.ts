/** `/v1/events/history` and `/v1/health`. */

import { apiFetch } from "../http";
import { ensureConnection } from "../connection";
import type { EventLogRecord, HealthResponse } from "./types";

export interface EventHistoryQuery {
  limit?: number;
  /** The only filter the route accepts — there is no `task_id` column (GAP-10). */
  agentId?: string;
}

/** `GET /v1/events/history` — bare array, server-capped at 1000. */
export async function getEventHistory(
  query: EventHistoryQuery = {},
  signal?: AbortSignal,
): Promise<EventLogRecord[]> {
  return await apiFetch<EventLogRecord[]>("/v1/events/history", {
    query: { limit: query.limit, agent_id: query.agentId },
    signal,
  });
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
