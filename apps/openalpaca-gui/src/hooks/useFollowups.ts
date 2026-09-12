/** The lane follow-up queue: the pending list, and the two writes. */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import {
  cancelFollowup,
  listFollowups,
  queueFollowup,
  type FollowupCancelled,
  type FollowupRecord,
} from "@/lib/api/followups";
import { qk } from "@/lib/query-keys";

/**
 * `GET /v1/lanes/{lane_key}/followups` — what is still pending on this lane.
 *
 * Disabled until the lane is known: a chat session has no lane key until its
 * first turn, and `""` is not a lane the daemon would accept. Both writes and
 * the daemon's own `followup_queued` / `followup_cancelled` frames invalidate
 * `qk.followups.*` (`lib/query-client.ts`), so this needs no polling — a
 * follow-up the *model* queued mid-run appears here without a refetch loop.
 */
export function useFollowups(
  laneKey: string | null,
): UseQueryResult<FollowupRecord[]> {
  return useQuery({
    queryKey: qk.followups.list(laneKey ?? ""),
    queryFn: ({ signal }) => listFollowups(laneKey as string, signal),
    enabled: laneKey !== null && laneKey !== "",
    staleTime: 10_000,
  });
}

export interface QueueFollowupInput {
  laneKey: string;
  content: string;
  /** The run this follow-up came out of, when the composer was aimed at one. */
  sourceTaskId?: string;
  /** The project the re-entered turn should be scoped to. */
  workspacePath?: string;
}

/** `POST /v1/lanes/{lane_key}/followups`. */
export function useQueueFollowup(): UseMutationResult<
  FollowupRecord,
  Error,
  QueueFollowupInput
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: QueueFollowupInput) =>
      queueFollowup(
        input.laneKey,
        input.content,
        input.sourceTaskId,
        input.workspacePath,
      ),
    onSuccess: (_data, input) => {
      void client.invalidateQueries({
        queryKey: qk.followups.list(input.laneKey),
      });
    },
  });
}

export interface CancelFollowupInput {
  laneKey: string;
  followupId: number;
}

/**
 * `DELETE /v1/lanes/{lane_key}/followups/{id}`.
 *
 * The list is invalidated on failure as well as on success, because the most
 * likely failure *is* a state change: `409 FOLLOWUP_NOT_QUEUED` means the
 * daemon claimed the item first, so the cached row is stale either way.
 */
export function useCancelFollowup(): UseMutationResult<
  FollowupCancelled,
  Error,
  CancelFollowupInput
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: CancelFollowupInput) =>
      cancelFollowup(input.laneKey, input.followupId),
    onSettled: (_data, _error, input) => {
      void client.invalidateQueries({
        queryKey: qk.followups.list(input.laneKey),
      });
    },
  });
}
