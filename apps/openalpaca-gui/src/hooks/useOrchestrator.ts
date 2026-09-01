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
import { GAPS, gapNote } from "@/lib/unavailable";

export function useOrchestratorConfig(): UseQueryResult<OrchestratorConfigResponse> {
  return useQuery({
    queryKey: qk.orchestrator.config(),
    queryFn: ({ signal }) => getOrchestratorConfig(signal),
  });
}

/** Warn the user: this picker writes the daemon-wide default (GAP-13). */
export const MODEL_SCOPE_NOTE = gapNote(GAPS["GAP-13"]);

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
