/**
 * The moved-project offer behind the picker (plan §4.8, P-12).
 *
 * Two questions, in order, and neither is asked speculatively:
 *
 *   1. Does the chosen path hold a store that records a **different** root?
 *      That is `GET /v1/workspaces?path=<chosen>`, and `moved` is the answer.
 *   2. If so, what is still attached to the root it names? That is the same
 *      route again, on `recorded_root`, and it is what the offer counts.
 *
 * The second query is enabled only when the first says `moved`, so a window
 * pointed at a project that never moved makes exactly one request and gets
 * nothing to show.
 *
 * `useRebaseWorkspace` is the write, and it is never called on its own: the
 * card asks, the owner confirms, and only then does the `PATCH` go out. The
 * whole `workspaces` domain plus everything the re-base re-addressed is
 * invalidated afterwards, because artifacts, sessions and tasks all just
 * changed the root they answer for.
 */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import {
  getWorkspace,
  rebaseWorkspace,
  type Workspace,
  type WorkspaceRebase,
} from "@/lib/api/workspaces";
import { qk } from "@/lib/query-keys";

/** `GET /v1/workspaces?path=`. Disabled until there is a path to ask about. */
export function useWorkspace(
  path: string | null,
): UseQueryResult<Workspace | null> {
  return useQuery({
    queryKey: qk.workspaces.detail(path ?? ""),
    queryFn: ({ signal }) =>
      path === null ? null : getWorkspace(path, signal),
    enabled: path !== null,
    // A moved directory is not something that changes while the panel is open,
    // and a failed lookup should not retry against a path the owner mistyped.
    staleTime: 30_000,
    retry: false,
  });
}

export interface MovedProject {
  /** The root the store still records — where the history is. */
  from: string;
  /** The path the owner chose, and where the store now stands. */
  to: string;
  /** What is attached to `from`, and what a re-base would move. */
  rows: Workspace["rows"];
  /** Runs under `from` that are still in flight; the daemon refuses until 0. */
  activeTasks: number;
}

/**
 * The offer, or `null` when there is nothing to offer.
 *
 * `pending` is true only while the *first* question is outstanding — the panel
 * says "checking" then, rather than showing a project as settled before it has
 * been asked about.
 */
export function useMovedProject(path: string | null): {
  moved: MovedProject | null;
  pending: boolean;
  error: Error | null;
} {
  const chosen = useWorkspace(path);
  const recorded =
    chosen.data?.moved === true ? chosen.data.recorded_root : null;
  const previous = useWorkspace(recorded);

  const moved: MovedProject | null =
    chosen.data?.moved === true &&
    chosen.data.recorded_root !== null &&
    previous.data != null
      ? {
          from: chosen.data.recorded_root,
          to: chosen.data.path,
          rows: previous.data.rows,
          activeTasks: previous.data.active_tasks,
        }
      : null;

  return {
    moved,
    pending: chosen.isPending && path !== null,
    error: (chosen.error ?? previous.error) as Error | null,
  };
}

/** `PATCH /v1/workspaces` — the one transaction, on the owner's say-so. */
export function useRebaseWorkspace(): UseMutationResult<
  WorkspaceRebase,
  Error,
  { from: string; to: string }
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ from, to }: { from: string; to: string }) =>
      rebaseWorkspace(from, to),
    onSuccess: () => {
      for (const key of [
        qk.workspaces.all(),
        qk.artifacts.all(),
        qk.sessions.all(),
        qk.tasks.all(),
      ]) {
        void client.invalidateQueries({ queryKey: key });
      }
    },
  });
}
