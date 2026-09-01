/** Settings → Plugins. Install is GAP-19. */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import {
  listPlugins,
  performPluginAction,
  setPluginConfig,
} from "@/lib/api/plugins";
import type { PluginAction, PluginInfo } from "@/lib/api/types";
import { qk } from "@/lib/query-keys";
import { unavailable, type Availability } from "@/lib/unavailable";

export function usePlugins(): UseQueryResult<PluginInfo[]> {
  return useQuery({
    queryKey: qk.plugins.list(),
    queryFn: ({ signal }) => listPlugins(signal),
  });
}

export interface PluginActionInput {
  name: string;
  action: PluginAction;
}

export function usePluginAction(): UseMutationResult<
  void,
  Error,
  PluginActionInput
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: PluginActionInput) =>
      performPluginAction(input.name, input.action),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: qk.plugins.all() });
      void client.invalidateQueries({ queryKey: qk.connectors.all() });
    },
  });
}

export interface PluginConfigInput {
  name: string;
  key: string;
  value: string;
}

export function useSetPluginConfig(): UseMutationResult<
  void,
  Error,
  PluginConfigInput
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: PluginConfigInput) =>
      setPluginConfig(input.name, input.key, input.value),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: qk.plugins.all() });
    },
  });
}

/** GAP-19 — installing means copying a directory in by hand and restarting. */
export function usePluginInstall(): Availability<never> {
  return unavailable("GAP-19");
}
