/**
 * Hooks for the design surfaces the daemon cannot serve.
 *
 * These deliberately look like the real hooks — same call sites, same place in
 * a component — but they return `Availability<T>` instead of query state. A
 * view renders the design's own empty state plus `result.reason`; it must never
 * substitute invented rows.
 *
 * When a route lands, the hook body swaps to a `useQuery` and the view keeps
 * its `available` branch unchanged.
 *
 * Only the gaps a view actually renders through get a hook. Three others —
 * steering (GAP-02), follow-ups (GAP-03) and re-run/start (GAP-06) — are
 * handled where they surface instead, because each has a working alternative
 * rather than an empty state: `run-actions` disables the verbs and names the
 * route, and `useChatSession` steers down the `/steer …` text channel. Their
 * adapters stay in `lib/api/unbacked` as the shape the routes would take.
 */

import { useMemo } from "react";

import {
  getDaemonStatusDetail,
  getTaskTimeline,
  type DaemonStatusDetail,
  type TaskTimeline,
} from "@/lib/api/unbacked";
import { type Availability } from "@/lib/unavailable";

/** GAP-09 — the Parallel work swimlanes. */
export function useTaskTimeline(
  taskId: string | null,
): Availability<TaskTimeline> {
  return useMemo(() => getTaskTimeline(taskId ?? ""), [taskId]);
}

/** GAP-14 — uptime, `Schema vNN`, `Copy log path`. */
export function useDaemonStatusDetail(): Availability<DaemonStatusDetail> {
  return useMemo(() => getDaemonStatusDetail(), []);
}
