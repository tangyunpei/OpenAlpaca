/**
 * `GET /v1/workspaces?path=` as a query (storage design §9): what the daemon
 * resolves one path to — its project root, the project's id and folder when
 * it is registered, whether its folder is missing, and what is recorded
 * under it.
 */

import { useQuery, type UseQueryResult } from "@tanstack/react-query";

import { getWorkspace, type Workspace } from "@/lib/api/workspaces";
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
    // What is recorded under a root changes only when work is filed there,
    // and a failed lookup should not retry against a path the owner mistyped.
    staleTime: 30_000,
    retry: false,
  });
}
