/**
 * The artifact resource as React state (plan §4.9).
 *
 * These four queries replace the `Availability`-returning stubs the Library was
 * written against while `/v1/artifacts*` did not exist. Every one of them keys
 * off `qk.artifacts.*`, which is what the `artifact_written` WebSocket frame
 * invalidates (`lib/query-client.ts`), so a file an agent writes appears in the
 * Library without a reload.
 *
 * Pins are server state: `useTogglePin` writes the local cache first so the
 * star flips instantly, sends `PUT …/pin`, and then takes whatever the daemon
 * answered — including a revert if the call failed. The list and detail queries
 * do the same reconciliation for every row they carry, so `oa-pins` can never
 * drift into claiming a pin the daemon does not have.
 */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";
import { useEffect, useMemo } from "react";

import {
  getArtifact,
  getArtifactDiff,
  listArtifactVersions,
  listArtifacts,
  setArtifactPinned,
  type Artifact,
  type ArtifactDiff,
  type ArtifactListPage,
  type ArtifactPinState,
  type ArtifactVersion,
  type ListArtifactsQuery,
} from "@/lib/api/artifacts";
import { qk } from "@/lib/query-keys";
import { useUiStore } from "@/stores/ui";

/** Mirror what the server said about these rows' pins into the local cache. */
function useSyncedPins(artifacts: readonly Artifact[] | undefined): void {
  useEffect(() => {
    if (artifacts === undefined || artifacts.length === 0) return;
    useUiStore
      .getState()
      .syncPins(
        Object.fromEntries(
          artifacts.map((artifact) => [artifact.id, artifact.pinned]),
        ),
      );
  }, [artifacts]);
}

export interface UseArtifactsOptions {
  /** Left unset the query runs; the palette turns it off while it is closed. */
  enabled?: boolean;
}

/** `GET /v1/artifacts` — the Library list, and any run- or name-scoped slice. */
export function useArtifacts(
  query: ListArtifactsQuery = {},
  options: UseArtifactsOptions = {},
): UseQueryResult<ArtifactListPage> {
  const result = useQuery({
    queryKey: qk.artifacts.list(query as Record<string, unknown>),
    queryFn: ({ signal }) => listArtifacts(query, signal),
    enabled: options.enabled ?? true,
  });
  useSyncedPins(result.data?.artifacts);
  return result;
}

/** `GET /v1/artifacts/{id}` — the detail pane's own row. */
export function useArtifact(id: string | null): UseQueryResult<Artifact> {
  const result = useQuery({
    queryKey: qk.artifacts.detail(id ?? ""),
    queryFn: ({ signal }) => getArtifact(id as string, signal),
    enabled: id !== null,
  });
  const row = result.data;
  useSyncedPins(useMemo(() => (row === undefined ? undefined : [row]), [row]));
  return result;
}

/** `GET /v1/artifacts/{id}/versions` — the History tab. */
export function useArtifactVersions(
  id: string | null,
): UseQueryResult<ArtifactVersion[]> {
  return useQuery({
    queryKey: qk.artifacts.versions(id ?? ""),
    queryFn: ({ signal }) => listArtifactVersions(id as string, signal),
    enabled: id !== null,
  });
}

/**
 * `GET /v1/artifacts/{id}/diff` — the Diff tab.
 *
 * Disabled while there is no pair to compare: a v1-only artifact has nothing
 * to diff, and asking would be a 404 the tab would then have to explain away.
 */
export function useArtifactDiff(
  id: string | null,
  from: number,
  to: number,
): UseQueryResult<ArtifactDiff> {
  return useQuery({
    queryKey: qk.artifacts.diff(id ?? "", from, to),
    queryFn: ({ signal }) => getArtifactDiff(id as string, from, to, signal),
    enabled: id !== null && from >= 1 && to > from,
  });
}

export interface PinVariables {
  id: string;
  pinned: boolean;
}

/** `PUT /v1/artifacts/{id}/pin`, optimistic in the local cache. */
export function useTogglePin(): UseMutationResult<
  ArtifactPinState,
  Error,
  PinVariables,
  { previous: boolean }
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ id, pinned }: PinVariables) => setArtifactPinned(id, pinned),
    onMutate: ({ id, pinned }) => {
      const previous = useUiStore.getState().isPinned(id);
      useUiStore.getState().setPin(id, pinned);
      return { previous };
    },
    onError: (_error, { id }, context) => {
      if (context !== undefined)
        useUiStore.getState().setPin(id, context.previous);
    },
    onSuccess: (state) => {
      useUiStore.getState().setPin(state.id, state.pinned);
      void client.invalidateQueries({ queryKey: qk.artifacts.all() });
    },
  });
}
