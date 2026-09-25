/**
 * Memory notes as React state (memory redesign, design §7, §7a; plan 3.9).
 *
 * Every query keys off `qk.memory.*`, whose `["memory"]` prefix is what the
 * `memory_changed` frame invalidates (`lib/query-client.ts`) — so a note the
 * model or the extractor writes appears without a reload, and the view never
 * polls (G30). Every write invalidates the same prefix on settle as well: that
 * is the half that works before the socket is up, and it is harmless after.
 *
 * Every hook takes the window's project path, because `x-workspace-path` is
 * what decides which partitions a request can see: another project is another
 * answer, not a stale one.
 */

import {
  useInfiniteQuery,
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";
import { useCallback, useMemo } from "react";

import {
  createMemory,
  forgetMemory,
  getMemory,
  getMemoryHistory,
  listMemories,
  listRecentlyLearned,
  listSuggestions,
  searchMemories,
  undoMemory,
  updateMemory,
  type CreateMemoryInput,
  type MemoryForgotten,
  type MemoryListItem,
  type MemoryNote,
  type MemoryRevision,
  type MemorySearchPage,
  type MemoryStoreResult,
  type MemorySuggestion,
  type MemoryUpdate,
  type MemoryUpdateResult,
  type MemoryWithdrawn,
  type NoteScope,
  type NoteStatus,
} from "@/lib/api/memories";
import { qk } from "@/lib/query-keys";

/** A page of the list — the route's ceiling is 100. */
export const MEMORY_PAGE_SIZE = 100;

export interface MemoryFeed {
  /** Every row loaded so far, newest `updated_at` first. */
  items: MemoryListItem[];
  loading: boolean;
  error: Error | null;
  hasMore: boolean;
  loadingMore: boolean;
  loadMore: () => void;
}

/**
 * `GET /v1/memories` for one partition and one status, paged by the daemon's
 * opaque cursor — valid for this filter set only, which is why the filters are
 * the key and a changed filter starts again from the first page.
 */
export function useMemoryFeed(
  scope: NoteScope,
  status: NoteStatus,
  workspacePath: string | null,
): MemoryFeed {
  const result = useInfiniteQuery({
    queryKey: qk.memory.list({ scope, status }, workspacePath),
    queryFn: ({ pageParam, signal }) =>
      listMemories(
        {
          scope,
          status,
          limit: MEMORY_PAGE_SIZE,
          cursor: pageParam ?? undefined,
        },
        workspacePath,
        signal,
      ),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor,
  });

  const pages = result.data?.pages;
  const items = useMemo(
    () => (pages ?? []).flatMap((page) => page.items),
    [pages],
  );
  const { fetchNextPage } = result;
  const loadMore = useCallback(() => void fetchNextPage(), [fetchNextPage]);

  return {
    items,
    loading: result.isLoading,
    error: result.error,
    hasMore: result.hasNextPage,
    loadingMore: result.isFetchingNextPage,
    loadMore,
  };
}

/** `GET /v1/memories/search` — off while the box is empty. */
export function useMemorySearch(
  q: string,
  scope: NoteScope,
  workspacePath: string | null,
): UseQueryResult<MemorySearchPage> {
  return useQuery({
    queryKey: qk.memory.search({ q, scope }, workspacePath),
    queryFn: ({ signal }) =>
      searchMemories({ q, scope }, workspacePath, signal),
    enabled: q !== "",
  });
}

/** `GET /v1/memories/recently-learned` — both partitions the header allows. */
export function useRecentlyLearned(
  workspacePath: string | null,
): UseQueryResult<MemoryListItem[]> {
  return useQuery({
    queryKey: qk.memory.recentlyLearned(workspacePath),
    queryFn: ({ signal }) => listRecentlyLearned(workspacePath, signal),
  });
}

/**
 * `GET /v1/memories/suggestions` — learned changes to notes the owner saved,
 * waiting for Replace or Keep mine (design 09 §3.5).
 */
export function useSuggestions(
  workspacePath: string | null,
): UseQueryResult<MemorySuggestion[]> {
  return useQuery({
    queryKey: qk.memory.suggestions(workspacePath),
    queryFn: ({ signal }) => listSuggestions(workspacePath, signal),
  });
}

/** `GET /v1/memories/{id}` — the detail, and a chat turn's "notes used". */
export function useMemoryNote(
  id: string | null,
  workspacePath: string | null,
): UseQueryResult<MemoryNote> {
  return useQuery({
    queryKey: qk.memory.detail(id ?? "", workspacePath),
    queryFn: ({ signal }) => getMemory(id as string, workspacePath, {}, signal),
    enabled: id !== null,
  });
}

/**
 * The titles of a few notes by id — what a recently-learned replacement row
 * names as the note it replaced. Each is the detail read, so it shares that
 * cache; an id not read yet (or gone) is simply absent from the map.
 */
export function useNoteTitles(
  ids: readonly string[],
  workspacePath: string | null,
): ReadonlyMap<string, string> {
  const results = useQueries({
    queries: ids.map((id) => ({
      queryKey: qk.memory.detail(id, workspacePath),
      queryFn: ({ signal }: { signal: AbortSignal }) =>
        getMemory(id, workspacePath, {}, signal),
    })),
  });
  const titles = results.map((r) => r.data?.title ?? null);
  const key = ids.map((id, i) => `${id}=${titles[i] ?? ""}`).join("|");
  return useMemo(() => {
    const map = new Map<string, string>();
    ids.forEach((id, i) => {
      const title = titles[i];
      if (typeof title === "string") map.set(id, title);
    });
    return map;
    // `key` is the ids and titles in one string: the map changes exactly
    // when either does, and not on every render.
  }, [key]);
}

/**
 * One kept `.history` copy. Its bytes can never change — the revision *is*
 * their hash — so it is never stale; only a forget takes it away.
 */
export function useMemoryRevision(
  id: string | null,
  revision: string | null,
  workspacePath: string | null,
): UseQueryResult<MemoryNote> {
  return useQuery({
    queryKey: qk.memory.revision(id ?? "", revision ?? "", workspacePath),
    queryFn: ({ signal }) =>
      getMemory(
        id as string,
        workspacePath,
        { revision: revision as string },
        signal,
      ),
    enabled: id !== null && revision !== null,
    staleTime: Infinity,
  });
}

/** `GET /v1/memories/{id}/history` — newest first, the current one included. */
export function useMemoryHistory(
  id: string | null,
  workspacePath: string | null,
): UseQueryResult<MemoryRevision[]> {
  return useQuery({
    queryKey: qk.memory.history(id ?? "", workspacePath),
    queryFn: ({ signal }) =>
      getMemoryHistory(id as string, workspacePath, signal),
    enabled: id !== null,
  });
}

function useMemoryInvalidation(): () => Promise<void> {
  const client = useQueryClient();
  return useCallback(
    () => client.invalidateQueries({ queryKey: qk.memory.all() }),
    [client],
  );
}

/** `POST /v1/memories` — origin `user`, the scope the view is showing. */
export function useCreateMemory(
  workspacePath: string | null,
): UseMutationResult<MemoryStoreResult, Error, CreateMemoryInput> {
  const invalidate = useMemoryInvalidation();
  return useMutation({
    mutationFn: (input: CreateMemoryInput) =>
      createMemory(input, workspacePath),
    onSettled: invalidate,
  });
}

export interface UpdateMemoryVariables {
  id: string;
  /** The revision this client read — sent as `If-Match`. */
  revision: string;
  update: MemoryUpdate;
}

/** `POST /v1/memories/{id}` — edit, archive or confirm. */
export function useUpdateMemory(
  workspacePath: string | null,
): UseMutationResult<MemoryUpdateResult, Error, UpdateMemoryVariables> {
  const invalidate = useMemoryInvalidation();
  return useMutation({
    mutationFn: ({ id, revision, update }: UpdateMemoryVariables) =>
      updateMemory(id, revision, update, workspacePath),
    onSettled: invalidate,
  });
}

export interface ForgetMemoryVariables {
  id: string;
  revision: string;
}

/**
 * `POST /v1/memories/{id}/undo` — a learned note withdrawn in one step, and
 * what it replaced back (design 09 §3.3).
 */
export function useUndoMemory(
  workspacePath: string | null,
): UseMutationResult<MemoryWithdrawn, Error, ForgetMemoryVariables> {
  const invalidate = useMemoryInvalidation();
  return useMutation({
    mutationFn: ({ id, revision }: ForgetMemoryVariables) =>
      undoMemory(id, revision, workspacePath),
    onSettled: invalidate,
  });
}

/** A suggestion to confirm: its own revision and its target's (S5). */
export interface ConfirmSuggestionVariables {
  suggestion: MemorySuggestion;
}

/**
 * Replace: `POST /v1/memories/{id}` with `action: confirm`, the suggestion's
 * revision as `If-Match` and the target's as `target_revision` — both as
 * this window read them, so an edit of either since is a `409`.
 */
export function useConfirmSuggestion(
  workspacePath: string | null,
): UseMutationResult<MemoryUpdateResult, Error, ConfirmSuggestionVariables> {
  const invalidate = useMemoryInvalidation();
  return useMutation({
    mutationFn: ({ suggestion }: ConfirmSuggestionVariables) =>
      updateMemory(
        suggestion.id,
        suggestion.revision,
        { action: "confirm", target_revision: suggestion.target.revision },
        workspacePath,
      ),
    onSettled: invalidate,
  });
}

/**
 * Keep mine: `POST /v1/memories/{id}/undo` on the suggestion — a withdrawal,
 * so nothing else the owner said is forgotten with it (design 09 §3.3).
 */
export function useDismissSuggestion(
  workspacePath: string | null,
): UseMutationResult<MemoryWithdrawn, Error, ConfirmSuggestionVariables> {
  const invalidate = useMemoryInvalidation();
  return useMutation({
    mutationFn: ({ suggestion }: ConfirmSuggestionVariables) =>
      undoMemory(suggestion.id, suggestion.revision, workspacePath),
    onSettled: invalidate,
  });
}

/** `DELETE /v1/memories/{id}`. */
export function useForgetMemory(
  workspacePath: string | null,
): UseMutationResult<MemoryForgotten, Error, ForgetMemoryVariables> {
  const invalidate = useMemoryInvalidation();
  return useMutation({
    mutationFn: ({ id, revision }: ForgetMemoryVariables) =>
      forgetMemory(id, revision, workspacePath),
    onSettled: invalidate,
  });
}
