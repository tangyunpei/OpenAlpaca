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
 * a moved folder: a project whose root is gone is `missing` (ST-7) until its
 * owner re-points it or deletes its data. Every refusal below is an
 * `ApiError` carrying the daemon's code and a sentence written for the
 * owner, which the caller shows as written.
 */

import { apiFetch } from "../http";

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
  /** The path asked about, canonicalized (as written when it is gone). */
  path: string;
  /** The project root it resolves to; `null` when it is in no project. */
  root: string | null;
  /** The project's id; `null` for a root nobody has used yet, or no project. */
  id: string | null;
  /** The project's folder under `projects/`; `null` as `id` is. */
  name: string | null;
  /** The root is not a directory (ST-7); `false` when `root` is `null`. */
  missing: boolean;
  /** What is recorded under `root`; zeros when `root` is `null`. */
  rows: WorkspaceCounts;
  /** Runs under `root` that are `running` or `paused`. */
  active_tasks: number;
  /** Runs under `root` waiting to start. */
  queued_tasks: number;
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
