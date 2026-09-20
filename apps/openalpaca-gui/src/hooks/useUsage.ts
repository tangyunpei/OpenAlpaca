/**
 * Spend and token counts.
 *
 * `useUsageSummary` reads `GET /v1/usage/summary?window=today` (GAP-08c,
 * closed): the daemon computes the UTC day, its total, the per-provider
 * breakdown and the caps. Before it existed the panel summed
 * `GET /v1/llm/usage/daily` client-side against a *local* date, and showed
 * per-provider *lifetime* tokens under a "today" heading.
 *
 * Per **N4** there is no daily budget and none is to be added: today's spend
 * is an informational total with no denominator, and what the panel names
 * instead are the two caps the daemon actually enforces — per workflow and
 * per agent turn.
 */

import { useQuery, type UseQueryResult } from "@tanstack/react-query";

import {
  getLlmUsage,
  getUsageSummary,
  type LlmUsageQuery,
} from "@/lib/api/usage";
import type { LlmCallLog, UsageCaps, UsageSummary } from "@/lib/api/types";
import { qk } from "@/lib/query-keys";

export function useLlmUsage(
  query: LlmUsageQuery = {},
): UseQueryResult<LlmCallLog[]> {
  return useQuery({
    queryKey: qk.usage.calls(query),
    queryFn: ({ signal }) => getLlmUsage(query, signal),
  });
}

/** Today's spend, its per-provider breakdown, and the caps that bound it. */
export function useUsageSummary(): UseQueryResult<UsageSummary> {
  return useQuery({
    queryKey: qk.usage.summary(),
    queryFn: ({ signal }) => getUsageSummary(signal),
    staleTime: 60_000,
  });
}

/**
 * The line under today's spend. It states the design's missing progress bar as
 * a decision rather than a gap — spend is not capped by the day at all — and
 * names the two caps that are real, with the daemon's own numbers.
 */
export function capsNote(caps: UsageCaps): string {
  return (
    `Spend is not capped daily by design — the caps are ` +
    `${formatCap(caps.workflow_max_cost_usd)} per workflow and ` +
    `${formatCap(caps.agent_max_cost_usd)} per agent turn`
  );
}

/**
 * A cap, in the dollars-and-cents the config is written in (`$5.00`) — not
 * `formatSpend`'s four places, which exist to show a spend of a fraction of a
 * cent and would render a round limit as `$5.0000`.
 */
export function formatCap(costUsd: number): string {
  return `$${costUsd.toFixed(2)}`;
}

/** Format a spend figure the way the design does (`$0.0184`). */
export function formatSpend(costUsd: number): string {
  return `$${costUsd.toFixed(4)}`;
}
