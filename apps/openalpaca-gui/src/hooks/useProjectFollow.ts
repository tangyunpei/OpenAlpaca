/**
 * The window follows its project to wherever it was re-pointed (R22, R14).
 *
 * A re-point rewrites every row that addressed the project's old root and
 * then records nothing at that root any more (storage design §6). The window
 * that pressed `Re-point…` takes the new root from the answer
 * (`ProjectsCard`), but one re-pointed from somewhere else — the CLI's
 * `openalpaca store rebase`, another window — keeps sending the old root, and
 * the daemon resolves it to no project: its turns would be filed with none,
 * silently joining the lane's conversation, which the re-point moved.
 *
 * The project's id is what survives the move (ST-13), so the window keeps it
 * beside the root (`stores/project`). When `GET /v1/status` answers that the
 * held root is in no project, the list of projects is read once and, if a
 * project with the held id is listed at another root, the window takes that
 * root and says so. A window with no id (an earlier build's value), or whose
 * project is listed nowhere (its data was deleted), is left as it is: the
 * Project card's note says the folder may have moved.
 *
 * Mounted once, at the app's root, so it runs whatever view is open — the
 * chat's turns are what it protects.
 */

import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";

import { useDaemonStatus, useStopIntent } from "@/hooks/useConnection";
import { listProjects, type Project } from "@/lib/api/workspaces";
import { qk } from "@/lib/query-keys";
import { useProjectStore } from "@/stores/project";
import { useUiStore } from "@/stores/ui";

/**
 * The listed project a window holding `path` and `id` should move to, or
 * `null`: the project with that id when it is listed at another root.
 */
export function projectToFollow(
  path: string,
  id: string,
  listed: readonly Project[],
): Project | null {
  const project = listed.find((entry) => entry.id === id);
  return project === undefined || project.root === path ? null : project;
}

/** What the window says when it followed its project. */
export function followedToast(root: string): string {
  return `This window's project was re-pointed to ${root}: its turns go there now.`;
}

export function useProjectFollow(): void {
  const path = useProjectStore((s) => s.path);
  const id = useProjectStore((s) => s.id);
  const setPath = useProjectStore((s) => s.setPath);
  const showToast = useUiStore((s) => s.showToast);
  const stopped = useStopIntent() !== null;
  const status = useDaemonStatus(path);

  // Strictly `null`: `undefined` is a daemon that has not answered yet.
  const lost =
    !stopped &&
    path !== null &&
    id !== null &&
    status.data !== undefined &&
    status.data.project_root === null;

  // Always stale for this hook (`staleTime: 0`): the list the Projects card
  // read a moment ago is fresh by the app's 30 s default, and may record the
  // project at the root it was just re-pointed away from — believed, it would
  // keep the window there for good (residual X8). Turning `lost` on therefore
  // always reads the list again.
  const projects = useQuery({
    queryKey: qk.workspaces.list(),
    queryFn: ({ signal }) => listProjects(signal),
    enabled: lost,
    staleTime: 0,
  });

  // Only a list read since the root stopped resolving is believed: a cached
  // one may predate the re-point, and following it would move the window
  // twice. While that read is out the cached one is ignored.
  const listed = lost && !projects.isFetching ? projects.data : undefined;

  useEffect(() => {
    if (path === null || id === null || listed === undefined) return;
    const target = projectToFollow(path, id, listed);
    if (target === null) return;
    setPath(target.root, target.id);
    showToast(followedToast(target.root));
  }, [path, id, listed, setPath, showToast]);
}
