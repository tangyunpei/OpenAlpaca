/**
 * `/v1/workspaces` — the project a store belongs to, and re-basing it when the
 * directory moved (plan §4.8, P-12).
 *
 * A project's path is its identity in four places — its artifacts, its
 * conversations, its runs and its workspace memories — so moving the directory
 * strands all four at once. The daemon can re-attach them in one transaction,
 * but only when someone tells it where the project went, and the picker is the
 * only place that knows: the owner points it at the new path.
 *
 * The detection is exact rather than guessed. A project store records the root
 * it was seeded at (`.openalpaca/.layout`), so a store standing at a path it
 * does not name is a project that moved — `moved: true` — and `recorded_root`
 * is where its history still lives. Nothing here re-bases on its own: the
 * `PATCH` happens when the owner asks for it.
 */

import { apiFetch } from "../http";

/** The four members of §4.8's one transaction, counted. */
export interface WorkspaceCounts {
  artifacts: number;
  sessions: number;
  tasks: number;
  memories: number;
}

export interface Workspace {
  /** The root the daemon resolved the requested path to. */
  path: string;
  /** Whether a `.openalpaca` store directory stands there. */
  store_present: boolean;
  /**
   * The root that store records for itself, or `null` when there is no store
   * or it predates the marker. `null` is "nothing to say", not "not moved".
   */
  recorded_root: string | null;
  /** `store_present` and `recorded_root` names a *different* path. */
  moved: boolean;
  /** What is addressed under `path` right now. */
  rows: WorkspaceCounts;
  /** Runs there that are `running` or `paused`; a re-base waits for them. */
  active_tasks: number;
}

export interface WorkspaceRebase {
  old_path: string;
  new_path: string;
  /** What actually moved. */
  moved: WorkspaceCounts;
  /** Whether the store directory itself was moved, rather than already there. */
  store_moved: boolean;
}

/** Total rows across the four members — what a confirmation counts. */
export function totalRows(counts: WorkspaceCounts): number {
  return counts.artifacts + counts.sessions + counts.tasks + counts.memories;
}

/** `GET /v1/workspaces?path=` — 400 for a relative or missing path. */
export async function getWorkspace(
  path: string,
  signal?: AbortSignal,
): Promise<Workspace> {
  return await apiFetch<Workspace>("/v1/workspaces", {
    signal,
    query: { path },
  });
}

/**
 * `PATCH /v1/workspaces` — the one transaction.
 *
 * Rejects with an `ApiError` carrying the daemon's own code:
 * `WORKSPACE_NOT_FOUND` (404), `WORKSPACE_BUSY`, `WORKSPACE_EXISTS` and
 * `WORKSPACE_MOVE_BLOCKED` (409). Each is a refusal with a reason, and the
 * caller shows the reason rather than a generic failure.
 */
export async function rebaseWorkspace(
  oldPath: string,
  newPath: string,
): Promise<WorkspaceRebase> {
  return await apiFetch<WorkspaceRebase>("/v1/workspaces", {
    method: "PATCH",
    body: { old_path: oldPath, new_path: newPath },
  });
}
