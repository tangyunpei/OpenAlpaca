/** `/v1/llm/usage*` and `/v1/usage/summary`. */

import { apiFetch } from "../http";
import type { LlmCallLog, LlmUsageDaily, UsageSummary } from "./types";

export interface LlmUsageQuery {
  agentId?: string;
  keyId?: string;
  limit?: number;
  /** Wins over `agentId`/`keyId` when more than one is set (GAP-08b, closed). */
  taskId?: string;
}

/** `GET /v1/llm/usage` */
export async function getLlmUsage(
  query: LlmUsageQuery = {},
  signal?: AbortSignal,
): Promise<LlmCallLog[]> {
  return await apiFetch<LlmCallLog[]>("/v1/llm/usage", {
    query: {
      agent_id: query.agentId,
      key_id: query.keyId,
      limit: query.limit,
      task_id: query.taskId,
    },
    signal,
  });
}

/** `GET /v1/llm/usage/daily` */
export async function getLlmUsageDaily(
  query: { agentId?: string; date?: string; limit?: number } = {},
  signal?: AbortSignal,
): Promise<LlmUsageDaily[]> {
  return await apiFetch<LlmUsageDaily[]>("/v1/llm/usage/daily", {
    query: { agent_id: query.agentId, date: query.date, limit: query.limit },
    signal,
  });
}

/**
 * `GET /v1/usage/summary?window=today` (GAP-08c, closed).
 *
 * The daemon computes the day, its total, the per-provider breakdown and the
 * caps. `today` is the only window it accepts — anything else answers
 * `400 UNKNOWN_WINDOW` — so the parameter is fixed here rather than exposed:
 * offering a choice the route does not have would be the client inventing an
 * API.
 */
export async function getUsageSummary(
  signal?: AbortSignal,
): Promise<UsageSummary> {
  return await apiFetch<UsageSummary>("/v1/usage/summary", {
    query: { window: "today" },
    signal,
  });
}
