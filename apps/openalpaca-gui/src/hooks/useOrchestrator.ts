/** Orchestrator config and its latency/decision telemetry. */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import {
  getDispatchDecisions,
  getLatencyAggregates,
  getLatencyRecords,
  getOrchestratorConfig,
  updateOrchestratorConfig,
  type TelemetryQuery,
} from "@/lib/api/orchestrator";
import type {
  DispatchDecisionRecord,
  LatencyAggregate,
  OrchestratorConfigResponse,
  OrchestratorLatencyRecord,
  UpdateOrchestratorRequest,
} from "@/lib/api/types";
import { qk } from "@/lib/query-keys";

/**
 * The composer picker's footer line. GAP-13 closed, so this states a scope
 * rather than warning about one: the pick rides on that turn's
 * `POST /v1/chat` as `model` and is written nowhere, and the daemon-wide
 * default is a different control in a different place.
 */
export const MODEL_SCOPE_NOTE =
  "Applies to this conversation — the daemon default is in Settings → Models & keys";

export function useOrchestratorConfig(): UseQueryResult<OrchestratorConfigResponse> {
  return useQuery({
    queryKey: qk.orchestrator.config(),
    queryFn: ({ signal }) => getOrchestratorConfig(signal),
  });
}

export function useUpdateOrchestratorConfig(): UseMutationResult<
  void,
  Error,
  UpdateOrchestratorRequest
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (req: UpdateOrchestratorRequest) =>
      updateOrchestratorConfig(req),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: qk.orchestrator.all() });
    },
  });
}

export function useLatencyRecords(
  query: TelemetryQuery = {},
): UseQueryResult<OrchestratorLatencyRecord[]> {
  return useQuery({
    queryKey: qk.orchestrator.latency(query),
    queryFn: ({ signal }) => getLatencyRecords(query, signal),
  });
}

export function useLatencyAggregates(
  query: { from?: string; to?: string } = {},
): UseQueryResult<LatencyAggregate[]> {
  return useQuery({
    queryKey: qk.orchestrator.latencyAggregate(query),
    queryFn: ({ signal }) => getLatencyAggregates(query, signal),
  });
}

export function useDispatchDecisions(
  query: TelemetryQuery = {},
): UseQueryResult<DispatchDecisionRecord[]> {
  return useQuery({
    queryKey: qk.orchestrator.decisions(query),
    queryFn: ({ signal }) => getDispatchDecisions(query, signal),
  });
}
