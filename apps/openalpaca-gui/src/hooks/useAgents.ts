/** Settings → Agents: templates, running instances, per-agent config. */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import {
  getAgent,
  getAgentConfig,
  listAgentInstances,
  listAgentTemplates,
  performAgentAction,
  updateAgentConfig,
} from "@/lib/api/agents";
import type {
  AgentActionResponse,
  AgentConfigFile,
  AgentConfigResponse,
  AgentDetailResponse,
  AgentInstance,
  AgentTemplate,
} from "@/lib/api/types";
import { qk } from "@/lib/query-keys";
import { GAPS, gapNote } from "@/lib/unavailable";

export function useAgentTemplates(): UseQueryResult<AgentTemplate[]> {
  return useQuery({
    queryKey: qk.agents.templates(),
    queryFn: ({ signal }) => listAgentTemplates(signal),
  });
}

/** `12 runs 7d` and the per-template toggle are not served (GAP-20). */
export const TEMPLATE_METRICS_NOTE = gapNote(GAPS["GAP-20"]);

export function useAgentInstances(): UseQueryResult<AgentInstance[]> {
  return useQuery({
    queryKey: qk.agents.instances(),
    queryFn: ({ signal }) => listAgentInstances(signal),
    staleTime: 10_000,
  });
}

export function useAgent(
  id: string | null,
): UseQueryResult<AgentDetailResponse> {
  return useQuery({
    queryKey: qk.agents.detail(id ?? ""),
    queryFn: ({ signal }) => getAgent(id as string, signal),
    enabled: id !== null,
  });
}

export function useAgentConfig(
  id: string | null,
): UseQueryResult<AgentConfigResponse> {
  return useQuery({
    queryKey: qk.agents.config(id ?? ""),
    queryFn: ({ signal }) => getAgentConfig(id as string, signal),
    enabled: id !== null,
  });
}

export interface UpdateAgentConfigInput {
  id: string;
  config: AgentConfigFile;
  /** Optimistic lock — a stale version 409s. */
  configVersion: number;
}

export function useUpdateAgentConfig(): UseMutationResult<
  void,
  Error,
  UpdateAgentConfigInput
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: UpdateAgentConfigInput) =>
      updateAgentConfig(input.id, input.config, input.configVersion),
    onSuccess: (_data, input) => {
      void client.invalidateQueries({ queryKey: qk.agents.config(input.id) });
      void client.invalidateQueries({ queryKey: qk.agents.templates() });
    },
  });
}

/** Pauses/resumes a running *instance* — not the same as disabling a template. */
export function useAgentAction(): UseMutationResult<
  AgentActionResponse,
  Error,
  { id: string; action: "pause" | "resume" }
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: { id: string; action: "pause" | "resume" }) =>
      performAgentAction(input.id, input.action),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: qk.agents.instances() });
    },
  });
}
