/**
 * The persisted event log.
 *
 * `GET /v1/events/history` is an envelope with a keyset cursor, filterable by
 * run, agent and event type. Two readers use it: Settings shows the daemon-wide
 * tail, and a run detail shows its own log through `useRunEventLog`.
 *
 * The live WS ring (`useEventRing`) is still richer per frame but is
 * unbounded-lossy and vanishes on reload; freshness here comes from the cache
 * bridge instead — a run event invalidates that run's log key.
 */

import { useQuery, type UseQueryResult } from "@tanstack/react-query";

import {
  getEventHistory,
  getRunEventLog,
  type EventHistoryPage,
  type EventHistoryQuery,
} from "@/lib/api/telemetry";
import type { RunEventPage } from "@/lib/api/run-events";
import { qk } from "@/lib/query-keys";

export function useEventHistory(
  query: EventHistoryQuery = {},
): UseQueryResult<EventHistoryPage> {
  return useQuery({
    queryKey: qk.events.history(query),
    queryFn: ({ signal }) => getEventHistory(query, signal),
  });
}

/** One run's own log (GAP-10). Disabled until there is a run to ask about. */
export function useRunEventLog(
  taskId: string | null,
): UseQueryResult<RunEventPage> {
  return useQuery({
    queryKey: qk.tasks.eventLog(taskId ?? ""),
    queryFn: ({ signal }) =>
      getRunEventLog(taskId as string, 100, undefined, signal),
    enabled: taskId !== null && taskId !== "",
    staleTime: 5_000,
  });
}
