/** `/v1/llm/usage*`. */

import { apiFetch } from "../http";
import type { LlmCallLog, LlmUsageDaily } from "./types";

export interface LlmUsageQuery {
  agentId?: string;
  keyId?: string;
  limit?: number;
  /**
   * GAP-08.1: the repository has `get_task_usage` but the route's query struct
   * accepts only `{ agent_id, key_id, limit }`, so this is ignored today. Sent
   * anyway — serde drops unknown params and the day it lands the UI is correct.
   */
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

export interface DailySpend {
  date: string;
  costUsd: number;
  tokensIn: number;
  tokensOut: number;
  requests: number;
}

/** Local `YYYY-MM-DD`, which is what the `date` query param expects. */
export function todayIsoDate(now: Date = new Date()): string {
  const year = now.getFullYear();
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

/**
 * Sum a day's per-agent/per-model rows into one figure.
 *
 * GAP-08.2 is why this exists client-side: `GET /v1/orchestrator/config`
 * reports `daily_cost_usd` but the handler hardcodes `0.0`.
 */
export function summarizeDailyUsage(
  date: string,
  rows: LlmUsageDaily[],
): DailySpend {
  return rows.reduce<DailySpend>(
    (acc, row) => ({
      date: acc.date,
      costUsd: acc.costUsd + row.total_cost_usd,
      tokensIn: acc.tokensIn + row.total_input_tokens,
      tokensOut: acc.tokensOut + row.total_output_tokens,
      requests: acc.requests + row.total_requests,
    }),
    { date, costUsd: 0, tokensIn: 0, tokensOut: 0, requests: 0 },
  );
}
