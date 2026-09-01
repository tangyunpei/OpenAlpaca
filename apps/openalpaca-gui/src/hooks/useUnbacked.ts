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
 * Only the gaps a view actually renders through get a hook. Four others —
 * steering (GAP-02), follow-ups (GAP-03), re-run/start (GAP-06) and the
 * identity route (GAP-16) — are handled where they surface instead, because
 * each has a working alternative rather than an empty state: `run-actions`
 * disables the verbs and names the route, `useChatSession` steers down the
 * `/steer …` text channel, and the lane key is learned from the first reply.
 * Their adapters stay in `lib/api/unbacked` as the shape the routes would take.
 */

import { useMemo } from "react";

import {
  getArtifactDiff,
  getDaemonStatusDetail,
  getTaskTimeline,
  listArtifacts,
  listArtifactVersions,
  type Artifact,
  type ArtifactDiff,
  type ArtifactListPage,
  type ArtifactVersion,
  type DaemonStatusDetail,
  type ListArtifactsQuery,
  type TaskTimeline,
} from "@/lib/api/unbacked";
import { unavailable, type Availability } from "@/lib/unavailable";

/** GAP-04 — the whole Library list. */
export function useArtifacts(
  query: ListArtifactsQuery = {},
): Availability<ArtifactListPage> {
  const key = JSON.stringify(query);
  return useMemo(
    () => listArtifacts(JSON.parse(key) as ListArtifactsQuery),
    [key],
  );
}

/** GAP-04 — one artifact's metadata. There is no single-artifact route either. */
export function useArtifact(artifactId: string | null): Availability<Artifact> {
  void artifactId;
  return useMemo(() => unavailable("GAP-04"), []);
}

/** GAP-05 — the History tab. */
export function useArtifactVersions(
  artifactId: string | null,
): Availability<ArtifactVersion[]> {
  return useMemo(() => listArtifactVersions(artifactId ?? ""), [artifactId]);
}

/** GAP-05 — the Diff tab. */
export function useArtifactDiff(
  artifactId: string | null,
  from = 1,
  to = 2,
): Availability<ArtifactDiff> {
  return useMemo(
    () => getArtifactDiff(artifactId ?? "", from, to),
    [artifactId, from, to],
  );
}

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
