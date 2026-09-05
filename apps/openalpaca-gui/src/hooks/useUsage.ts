/**
 * Spend and token counts.
 *
 * `useTodaySpend` sums `GET /v1/llm/usage/daily` client-side because tokens
 * and request counts have no other source; the daemon now serves the same
 * day's total cost directly on `GET /v1/orchestrator/config.daily_cost_usd`
 * (GAP-08a, closed). What is still genuinely missing is a served cost cap
 * (GAP-08c) — there is no daily budget by design (N4: caps are per-workflow
 * and per-turn), so the design's spend progress bar has no denominator.
 */

import { useQuery, type UseQueryResult } from "@tanstack/react-query";

import {
  getLlmUsage,
  getLlmUsageDaily,
  summarizeDailyUsage,
  todayIsoDate,
  type DailySpend,
  type LlmUsageQuery,
} from "@/lib/api/usage";
import type { LlmCallLog } from "@/lib/api/types";
import { qk } from "@/lib/query-keys";
import { GAPS, gapNote } from "@/lib/unavailable";

export function useLlmUsage(
  query: LlmUsageQuery = {},
): UseQueryResult<LlmCallLog[]> {
  return useQuery({
    queryKey: qk.usage.calls(query),
    queryFn: ({ signal }) => getLlmUsage(query, signal),
  });
}

/** Today's spend, summed from the daily rollup rows. */
export function useTodaySpend(
  date: string = todayIsoDate(),
): UseQueryResult<DailySpend> {
  return useQuery({
    queryKey: qk.usage.todaySpend(date),
    queryFn: async ({ signal }) => {
      const rows = await getLlmUsageDaily({ date }, signal);
      return summarizeDailyUsage(date, rows);
    },
    staleTime: 60_000,
  });
}

/** The note to show beside a spend figure the daemon does not cap. */
export const COST_NOTE = gapNote(GAPS["GAP-08c"]);

/** Format a spend figure the way the design does (`$0.0184`). */
export function formatSpend(costUsd: number): string {
  return `$${costUsd.toFixed(4)}`;
}
