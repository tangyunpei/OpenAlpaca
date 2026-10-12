/**
 * The projects the daemon knows, and their life cycle (storage design §6,
 * §9): one read per question, and three writes, each taken only on the
 * owner's say-so in Settings → Connection.
 *
 * Every write changes what other surfaces answer: a project's root is the
 * address of its conversations, runs, files and notes (R14), and
 * `GET /v1/status.project_root` — what the Memory view's `This project` and
 * the conversation sidebar read — is the daemon's resolution of a window's
 * path, which `Use` and a delete can flip for a folder with no git. So each
 * write invalidates those domains as well as its own; a dry run is a read
 * and invalidates nothing.
 */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import {
  chooseProject,
  deleteProjectData,
  getWorkspace,
  listProjects,
  repointProject,
  type Project,
  type ProjectDataDeletion,
  type ProjectRepoint,
  type Workspace,
} from "@/lib/api/workspaces";
import { qk } from "@/lib/query-keys";

/**
 * `GET /v1/workspaces`. Re-read on every mount: no event announces a project
 * a chat turn or an upload registered, and the list changes too rarely to
 * poll.
 */
export function useProjects(): UseQueryResult<Project[]> {
  return useQuery({
    queryKey: qk.workspaces.list(),
    queryFn: ({ signal }) => listProjects(signal),
    refetchOnMount: "always",
  });
}

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

/**
 * What a re-point re-addresses and a delete removes. The follow-up queue is
 * among them (R24): a delete takes a queued follow-up that names the
 * project, and a re-point rewrites its path, and neither is announced by a
 * `followup_*` frame.
 */
function invalidateProjectData(client: QueryClient): void {
  for (const key of [
    qk.workspaces.all(),
    qk.artifacts.all(),
    qk.sessions.all(),
    qk.tasks.all(),
    qk.memory.all(),
    qk.followups.all(),
    qk.statusAll(),
  ]) {
    void client.invalidateQueries({ queryKey: key });
  }
}

/** `POST /v1/workspaces` — `Use`. */
export function useChooseProject(): UseMutationResult<Project, Error, string> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (path: string) => chooseProject(path),
    onSuccess: () => {
      for (const key of [qk.workspaces.all(), qk.statusAll()]) {
        void client.invalidateQueries({ queryKey: key });
      }
    },
  });
}

/** `PATCH /v1/workspaces` — `Re-point…`, on the owner's press. */
export function useRepointProject(): UseMutationResult<
  ProjectRepoint,
  Error,
  { from: string; to: string }
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ from, to }: { from: string; to: string }) =>
      repointProject(from, to),
    onSuccess: () => invalidateProjectData(client),
  });
}

/** `POST /v1/workspaces/purge` — the plan (`dryRun`), then `Delete data`. */
export function useDeleteProjectData(): UseMutationResult<
  ProjectDataDeletion,
  Error,
  { path: string; dryRun: boolean }
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ path, dryRun }: { path: string; dryRun: boolean }) =>
      deleteProjectData(path, dryRun),
    onSuccess: (_answer, { dryRun }) => {
      if (!dryRun) invalidateProjectData(client);
    },
  });
}
