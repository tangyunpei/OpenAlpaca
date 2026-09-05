/**
 * The persisted event log.
 *
 * `GET /v1/events/history` filters by `agent_id` only and the table has no
 * `task_id` column, so a run-scoped log is impossible (GAP-10). The live WS
 * ring (`useEventRing`) is richer but unbounded-lossy; use both.
 */

import { useQuery, type UseQueryResult } from "@tanstack/react-query";

import { getEventHistory, type EventHistoryQuery } from "@/lib/api/telemetry";
import type { EventLogRecord } from "@/lib/api/types";
import { qk } from "@/lib/query-keys";
import { GAPS, gapNote } from "@/lib/unavailable";

export function useEventHistory(
  query: EventHistoryQuery = {},
): UseQueryResult<EventLogRecord[]> {
  return useQuery({
    queryKey: qk.events.history(query),
    queryFn: ({ signal }) => getEventHistory(query, signal),
  });
}

export const RUN_EVENT_LOG_NOTE = gapNote(GAPS["GAP-10"]);
