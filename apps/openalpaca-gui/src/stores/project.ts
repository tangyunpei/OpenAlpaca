/**
 * The chosen project — the workspace a chat turn belongs to (plan §4.7 item
 * 2; storage design §3, §9).
 *
 * The daemon reads `x-workspace-path` on `POST /v1/chat` and resolves it to a
 * project: a registered root, or the git repository it is in. That root is
 * what a run records as its `workspace_id`, and the files it produces are
 * kept under the project's folder in `<home>/projects/`, never in the
 * project folder itself.
 *
 * The value is the root `POST /v1/workspaces` answered when the owner pressed
 * `Use` in Settings → Connection, or the root a re-point moved the project
 * to, kept per machine in `localStorage` like the pane widths and the
 * artifact pins — with that project's id (R22). The root is what a turn
 * sends; the id is what survives a move (ST-13), so when the root stops
 * resolving because the project was re-pointed from somewhere else — the
 * CLI's `store rebase`, another window — the window can find where it went
 * (`hooks/useProjectFollow`). A value an earlier build wrote is the root, or
 * the text that was typed, alone: it is sent as it is, the daemon resolves
 * it like any path, and it has no id to follow. It is never guessed: an
 * empty value means "no project", and the GUI then sends no header, which is
 * the honest signal for "this turn belongs to no project". The projects the
 * daemon knows are `GET /v1/workspaces`; this store is only which one this
 * window uses.
 */

import { create } from "zustand";

export const PROJECT_STORAGE_KEY = "oa-project";

/** What a relative path is refused with, wherever a project folder is typed. */
export const RELATIVE_PATH_COPY =
  "An absolute path, please — a relative one is never a project: the daemon refuses it.";

/**
 * Whether a path is one the daemon can resolve.
 *
 * A relative path is never a project: the daemon refuses one on `Use` and on a
 * re-point (`400 INVALID_PATH`) and resolves a turn's relative
 * `x-workspace-path` to no project, because read against the *daemon's*
 * current directory rather than the GUI's it would name another folder — the
 * bug ruling R22 fixed. Only an absolute path names the same directory to both
 * sides, so a relative one is refused here rather than sent. POSIX (`/…`) and
 * Windows (`C:\…`, `C:/…`, `\\server\share`) shapes both count; the daemon
 * does the real existence check.
 */
export function isAbsolutePath(path: string): boolean {
  if (path.startsWith("/") || path.startsWith("\\\\")) return true;
  return /^[A-Za-z]:[\\/]/.test(path);
}

/**
 * Normalise a candidate to what the store holds: an absolute path, or `null`.
 *
 * Trailing separators are trimmed so `/repo` and `/repo/` are one project
 * rather than two spellings of it — the daemon canonicalises anyway, but the
 * value the user sees back should match what they typed once.
 */
export function normalizeProjectPath(value: string | null): string | null {
  if (value === null) return null;
  const trimmed = value.trim();
  if (trimmed === "") return null;
  if (!isAbsolutePath(trimmed)) return null;
  // Keep a bare root (`/`, `C:\`) intact; trim separators elsewhere.
  const stripped = trimmed.replace(/[\\/]+$/, "");
  return stripped === "" ? trimmed : stripped;
}

/** What the store holds: the root a turn sends, and its project's id. */
export interface HeldProject {
  path: string | null;
  /** `null` when the project is not known — no path, or an earlier build's value. */
  id: string | null;
}

/**
 * Read a stored `oa-project`: `{"root", "id"}` as JSON, or — written by an
 * earlier build, which kept no id — the root alone. Anything else is no
 * project.
 */
export function parseStoredProject(raw: string | null): HeldProject {
  if (raw === null) return { path: null, id: null };
  let value: unknown = raw;
  try {
    value = JSON.parse(raw);
  } catch {
    // Not JSON: an earlier build's root, read below as that root.
  }
  if (typeof value === "string") {
    return { path: normalizeProjectPath(value), id: null };
  }
  if (typeof value !== "object" || value === null) {
    return { path: null, id: null };
  }
  const { root, id } = value as { root?: unknown; id?: unknown };
  const path = normalizeProjectPath(typeof root === "string" ? root : null);
  return {
    path,
    id: path !== null && typeof id === "string" && id !== "" ? id : null,
  };
}

function loadProject(): HeldProject {
  try {
    if (typeof localStorage === "undefined") return { path: null, id: null };
    return parseStoredProject(localStorage.getItem(PROJECT_STORAGE_KEY));
  } catch {
    return { path: null, id: null };
  }
}

function saveProject({ path, id }: HeldProject): void {
  try {
    if (typeof localStorage === "undefined") return;
    if (path === null) localStorage.removeItem(PROJECT_STORAGE_KEY);
    else
      localStorage.setItem(
        PROJECT_STORAGE_KEY,
        JSON.stringify({ root: path, id }),
      );
  } catch {
    // Non-fatal: the choice simply does not survive a restart.
  }
}

export interface ProjectState {
  /** The chosen project root, or `null` when the owner chose none. */
  path: string | null;
  /**
   * The chosen project's id (ST-13), or `null` when it is not known — no
   * project, or a path an earlier build stored, or one set without an id.
   */
  id: string | null;
  /**
   * Set (or clear) the project, with its id when the daemon answered one. A
   * value that is not an absolute path clears both — `setPath` is the only
   * writer, so an unusable path can never be the one the composer sends.
   */
  setPath: (path: string | null, id?: string | null) => void;
}

export const useProjectStore = create<ProjectState>((set) => ({
  ...loadProject(),
  setPath: (path, id = null) => {
    const next = normalizeProjectPath(path);
    const held = { path: next, id: next === null ? null : id };
    set(held);
    saveProject(held);
  },
}));

/**
 * The `workspacePath` half of a `sendChatMessage` call.
 *
 * Returns `{}` — not `{ workspacePath: undefined }` — when no project is
 * chosen, so the header is genuinely absent from the request rather than
 * present-and-empty.
 */
export function workspaceOption(path: string | null): {
  workspacePath?: string;
} {
  return path === null ? {} : { workspacePath: path };
}
