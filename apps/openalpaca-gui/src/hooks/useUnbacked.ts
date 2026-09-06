/**
 * Hooks for the design surfaces the daemon cannot serve.
 *
 * These deliberately look like the real hooks — same call sites, same place in
 * a component — but they return `Availability<T>` instead of query state. A
 * view renders the design's own empty state plus `result.reason`; it must never
 * substitute invented rows.
 *
 * When a route lands, the hook body swaps to a `useQuery` and the view keeps
 * its `available` branch unchanged — which is exactly what happened to the
 * subagent timeline in Phase 4: `useTaskTimeline` is a real query in
 * `useTasks` now that `GET /v1/tasks/{id}/timeline` exists.
 *
 * Only the gaps a view actually renders through get a hook. The run action bar
 * used to be the exception — it disabled its gapped verbs at the point of use
 * rather than showing an empty state — and it no longer needs to: steering
 * (GAP-02), follow-ups (GAP-03) and re-run/start (GAP-06) are all served, and
 * `run-actions` names no route it cannot reach.
 */

import { useMemo } from "react";

import {
  getDaemonStatusDetail,
  type DaemonStatusDetail,
} from "@/lib/api/unbacked";
import { type Availability } from "@/lib/unavailable";

/** GAP-14 — uptime, `Schema vNN`, `Copy log path`. */
export function useDaemonStatusDetail(): Availability<DaemonStatusDetail> {
  return useMemo(() => getDaemonStatusDetail(), []);
}
