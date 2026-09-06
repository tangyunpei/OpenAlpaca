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
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";
import { useCallback, useEffect, useMemo } from "react";

import {
  getArtifact,
  getArtifactDiff,
  getArtifactText,
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

/** Enough rows to fill the pane; the rest is one `Load more` away. */
export const ARTIFACT_PAGE_SIZE = 200;

export interface ArtifactFeed {
  /** Every row loaded so far, in server order. */
  artifacts: Artifact[];
  /** The unpaged count the daemon reported, or `null` before the first page. */
  total: number | null;
  loading: boolean;
  error: Error | null;
  hasMore: boolean;
  loadingMore: boolean;
  loadMore: () => void;
}

/**
 * The Library's list: `GET /v1/artifacts` paged by `limit`/`offset`.
 *
 * Infinite rather than one big request because `total` can be any size and the
 * route caps a page at 500 rows; an invalidation (an `artifact_written` frame)
 * refetches the pages already loaded, so a new file appears without losing the
 * reader's place.
 */
export function useArtifactFeed(
  query: ListArtifactsQuery = {},
  pageSize: number = ARTIFACT_PAGE_SIZE,
): ArtifactFeed {
  const result = useInfiniteQuery({
    queryKey: qk.artifacts.list({ ...query, pageSize }),
    queryFn: ({ pageParam, signal }) =>
      listArtifacts({ ...query, limit: pageSize, offset: pageParam }, signal),
    initialPageParam: 0,
    getNextPageParam: (last, pages) => {
      const loaded = pages.reduce((n, page) => n + page.artifacts.length, 0);
      // A page that came back empty means the count and the rows disagree;
      // stop rather than asking for the same offset forever.
      if (last.artifacts.length === 0) return undefined;
      return loaded < last.total ? loaded : undefined;
    },
  });

  const pages = result.data?.pages;
  const artifacts = useMemo(
    () => (pages ?? []).flatMap((page) => page.artifacts),
    [pages],
  );
  useSyncedPins(artifacts);

  const { fetchNextPage } = result;
  const loadMore = useCallback(() => void fetchNextPage(), [fetchNextPage]);

  return {
    artifacts,
    total: pages === undefined ? null : (pages.at(-1)?.total ?? 0),
    loading: result.isLoading,
    error: result.error,
    hasMore: result.hasNextPage,
    loadingMore: result.isFetchingNextPage,
    loadMore,
  };
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

/**
 * An artifact's bytes as text, for the preview renderers.
 *
 * Guarded by the caller rather than here: images load by URL, binaries are not
 * text at all, and a very large file is not worth pulling into the webview —
 * `enabled` is where that policy lives (`views/library/preview.ts`).
 */
export function useArtifactText(
  id: string | null,
  enabled: boolean,
): UseQueryResult<string> {
  return useQuery({
    queryKey: qk.artifacts.content(id ?? ""),
    queryFn: ({ signal }) => getArtifactText(id as string, signal),
    enabled: id !== null && enabled,
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
