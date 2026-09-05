/** Settings → Connectors. Call counts, `unwired`, and Connect service are GAP-17. */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";
import { useMemo } from "react";

import {
  configureConnector,
  findUnwiredConnectors,
  getConnectorSettings,
  listConnectors,
  performConnectorAction,
  updateConnectorSettings,
} from "@/lib/api/connectors";
import type { Connector, ConnectorAction } from "@/lib/api/types";
import { qk } from "@/lib/query-keys";
import { GAPS, gapNote } from "@/lib/unavailable";

import { useExtensions } from "./useExtensions";

export function useConnectors(): UseQueryResult<Connector[]> {
  return useQuery({
    queryKey: qk.connectors.list(),
    queryFn: ({ signal }) => listConnectors(signal),
  });
}

/** The design's `unwired` tag, derived from the extension rows' `connector`. */
export function useUnwiredConnectors(): Array<{
  connectorId: string;
  declaredBy: string;
}> {
  const connectors = useConnectors();
  const extensions = useExtensions();
  return useMemo(
    () => findUnwiredConnectors(extensions.data ?? [], connectors.data ?? []),
    [extensions.data, connectors.data],
  );
}

/** Call counts and the `Connect service` flow do not exist yet. */
export const CONNECTOR_DETAIL_NOTE = gapNote(GAPS["GAP-17"]);

export interface ConnectorActionInput {
  id: string;
  action: ConnectorAction;
}

export function useConnectorAction(): UseMutationResult<
  void,
  Error,
  ConnectorActionInput
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: ConnectorActionInput) =>
      performConnectorAction(input.id, input.action),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: qk.connectors.all() });
    },
  });
}

export function useConfigureConnector(): UseMutationResult<
  void,
  Error,
  { id: string; token: string }
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: { id: string; token: string }) =>
      configureConnector(input.id, input.token),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: qk.connectors.all() });
    },
  });
}

export function useConnectorSettings(
  id: string | null,
): UseQueryResult<Record<string, string | null>> {
  return useQuery({
    queryKey: qk.connectors.settings(id ?? ""),
    queryFn: ({ signal }) => getConnectorSettings(id as string, signal),
    enabled: id !== null,
  });
}

export function useUpdateConnectorSettings(): UseMutationResult<
  void,
  Error,
  { id: string; settings: Record<string, string> }
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: { id: string; settings: Record<string, string> }) =>
      updateConnectorSettings(input.id, input.settings),
    onSuccess: (_data, input) => {
      void client.invalidateQueries({
        queryKey: qk.connectors.settings(input.id),
      });
    },
  });
}
