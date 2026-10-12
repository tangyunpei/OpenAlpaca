/**
 * `/v1/workspaces` — the projects the daemon knows, and their life cycle
 * (storage design §6, §9; owner decisions ST-6, ST-7, ST-19, ST-20).
 *
 * A project is a folder under `<home>/projects/` whose `project.json` records
 * its permanent id and its root (ST-13). Only the daemon can resolve a path
 * to one — canonicalize it, walk up to a registered root or a git
 * repository — so this window never guesses: it asks, and keeps the root
 * the daemon answered.
 *
 * Nothing is written inside a project folder (ST-1), so nothing travels with
 * a moved folder: a project whose root is gone, or no longer leads to its own
 * folder (renamed by case only, or moved with a link left at the old path),
 * is `missing` (ST-7, ST-23) until its owner re-points it or deletes its
 * data. Every refusal below is an
 * `ApiError` carrying the daemon's code and a sentence written for the
 * owner, which the caller shows as written.
 */

import { apiFetch } from "../http";

/** One project as its folder under `projects/` records it. */
export interface Project {
  /** The permanent id, a v4 UUID (ST-13). */
  id: string;
  /**
   * The root as recorded — the path every row of the project holds, canonical
   * when it was recorded (R14); a `missing` project's may no longer be its own
   * canonical form (ST-23).
   */
  root: string;
  /** Its folder's name under `<home>/projects/` (R4–R7). Never parsed back. */
  name: string;
  /**
   * The root is not a directory any more, or no longer leads to its own
   * folder — renamed by case only, or moved with a link left at the old path
   * (ST-7, ST-23); its data is kept.
   */
  missing: boolean;
}

/** What is recorded under one root, by the five path columns (R14). */
export interface WorkspaceCounts {
  /** `file_assets` rows: uploads and by-products. */
  artifacts: number;
  sessions: number;
  tasks: number;
  /** `conversation_messages` rows. */
  messages: number;
  /** Queued `lane_followups`. */
  followups: number;
}

/** `GET /v1/workspaces?path=` — what one path is. */
export interface Workspace {
  /**
   * The path asked about: as sent when it is a root a project records (that
   * project is the answer, matched before anything is resolved); otherwise
   * canonicalized (as written when it is gone).
   */
  path: string;
  /** The project root it resolves to; `null` when it is in no project. */
  root: string | null;
  /** The project's id; `null` for a root nobody has used yet, or no project. */
  id: string | null;
  /** The project's folder under `projects/`; `null` as `id` is. */
  name: string | null;
  /**
   * The root is not a directory, or no longer leads to its own folder (ST-7,
   * ST-23); `false` when `root` is `null`.
   */
  missing: boolean;
  /** What is recorded under `root`; zeros when `root` is `null`. */
  rows: WorkspaceCounts;
  /** Runs under `root` that are `running` or `paused`. */
  active_tasks: number;
  /** Runs under `root` waiting to start. */
  queued_tasks: number;
}

/** `PATCH /v1/workspaces`'s answer. */
export interface ProjectRepoint {
  old_path: string;
  new_path: string;
  /** What the one transaction rewrote. */
  moved: WorkspaceCounts;
  /** The project now: its new root and its folder's new name. */
  project: Project;
}

/** One line of the delete's plan, in the daemon's words. */
export interface DeletePlanEntry {
  entry: string;
  holds: string;
  retention: string;
  action: "delete" | "keep";
}

/** What the delete removes (the plan) or removed. */
export interface DeleteCounts {
  sessions: number;
  messages: number;
  tool_calls: number;
  followups: number;
  tasks: number;
  spans: number;
  run_events: number;
  uploads: number;
  /** By-products. */
  artifacts: number;
  memory_notes: number;
}

/** `POST /v1/workspaces/purge`'s answer: the plan on a dry run, what went otherwise. */
export interface ProjectDataDeletion {
  dry_run: boolean;
  applied: boolean;
  project: Project;
  entries: DeletePlanEntry[];
  counts: DeleteCounts;
  /** Files another conversation still links, moved to the root store (ST-19). */
  kept: { uploads: number; artifacts: number };
  /** `null` on a dry run. */
  removed: {
    session_dirs: number;
    memory_partitions: number;
    project_folder: boolean;
  } | null;
}

/** Every row a re-point rewrites, summed. */
export function totalRows(counts: WorkspaceCounts): number {
  return (
    counts.artifacts +
    counts.sessions +
    counts.tasks +
    counts.messages +
    counts.followups
  );
}

/** `GET /v1/workspaces` — every project the home store records, by folder name. */
export async function listProjects(signal?: AbortSignal): Promise<Project[]> {
  return await apiFetch<Project[]>("/v1/workspaces", { signal });
}

/** `GET /v1/workspaces?path=` — `400 INVALID_PATH` for a relative or empty path. */
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
 * `POST /v1/workspaces` — `Use`: the project a folder is in, made one when it
 * is in none (R10). Inside a git repository the answer is the repository;
 * otherwise the folder itself is registered. Refusals: `WORKSPACE_IS_HOME`
 * (409), `WORKSPACE_NOT_A_DIRECTORY` (422), `INVALID_PATH` (400),
 * `WORKSPACE_REGISTER_FAILED` (500).
 */
export async function chooseProject(path: string): Promise<Project> {
  return await apiFetch<Project>("/v1/workspaces", {
    method: "POST",
    body: { path },
  });
}

/**
 * `PATCH /v1/workspaces` — re-point a project at the folder it is in now
 * (§6). Refusals: `WORKSPACE_NOT_FOUND` (404); `WORKSPACE_BUSY`,
 * `WORKSPACE_EXISTS` and `WORKSPACE_IS_HOME` (409);
 * `WORKSPACE_NOT_A_DIRECTORY` (422); `INVALID_PATH` (400).
 */
export async function repointProject(
  oldPath: string,
  newPath: string,
): Promise<ProjectRepoint> {
  return await apiFetch<ProjectRepoint>("/v1/workspaces", {
    method: "PATCH",
    body: { old_path: oldPath, new_path: newPath },
  });
}

/**
 * `POST /v1/workspaces/purge` — everything OpenAlpaca holds for a project
 * (ST-19). `dryRun` answers the plan and deletes nothing. The daemon defaults
 * to a dry run; the flag is sent either way, so a missing argument can never
 * be the deletion. Refusals: `WORKSPACE_NOT_FOUND` (404), `WORKSPACE_BUSY`
 * (409), `INVALID_PATH` (400), and `WORKSPACE_NOT_A_ROOT` (422) for a path
 * inside a project rather than its root — the app sends a listed project's
 * root, so it never meets that one.
 */
export async function deleteProjectData(
  path: string,
  dryRun: boolean,
): Promise<ProjectDataDeletion> {
  return await apiFetch<ProjectDataDeletion>("/v1/workspaces/purge", {
    method: "POST",
    body: { path, dry_run: dryRun },
  });
}
