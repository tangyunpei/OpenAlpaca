/**
 * Spend and token counts.
 *
 * Everything here is client-side arithmetic because of GAP-08:
 * `orchestrator/config.daily_cost_usd` is hardcoded `0.0`, there is no
 * per-task cost query param, and the cost cap is not served at all.
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

/** The note to show beside any cost figure the daemon cannot cap or attribute. */
export const COST_NOTE = gapNote(GAPS["GAP-08"]);

/** Format a spend figure the way the design does (`$0.0184`). */
export function formatSpend(costUsd: number): string {
  return `$${costUsd.toFixed(4)}`;
}
