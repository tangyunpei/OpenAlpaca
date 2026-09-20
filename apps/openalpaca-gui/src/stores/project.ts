/**
 * The chosen project — the workspace a chat turn belongs to (plan §4.7 item 2).
 *
 * The daemon reads `x-workspace-path` on `POST /v1/chat` and resolves it to a
 * project root (the nearest `.git`/`.openalpaca` above it). That root is what
 * a run records as its `workspace_id` and where `artifact_write` puts the
 * files it produces. Until this store had a value, **no client sent the
 * header at all**, so every GUI run was project-less and every artifact landed
 * in the home store.
 *
 * Deliberately small. There is no project *concept* on the daemon yet — no
 * `GET /v1/projects`, no activation, no per-project state (plan §10 keeps that
 * out of scope) — so this is one path, chosen by the owner, kept per machine
 * in `localStorage`, exactly like the pane widths and the artifact pins. It is
 * not a list, not a recent-projects menu, and it is never guessed: an empty
 * value means "no project", and the GUI then sends no header, which is the
 * honest signal for "this turn belongs to no project".
 */

import { create } from "zustand";

export const PROJECT_STORAGE_KEY = "oa-project";

/**
 * Whether a path is one the daemon can resolve.
 *
 * A relative path would be resolved against the *daemon's* current directory,
 * not the GUI's — a different machine-state entirely, and the source of the
 * bug ruling R22 fixed. Only an absolute path names the same directory to both
 * sides, so a relative one is refused here rather than sent and silently
 * misread. POSIX (`/…`) and Windows (`C:\…`, `C:/…`, `\\server\share`) shapes
 * both count; the daemon does the real existence check.
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

export function loadProjectPath(): string | null {
  try {
    if (typeof localStorage === "undefined") return null;
    return normalizeProjectPath(localStorage.getItem(PROJECT_STORAGE_KEY));
  } catch {
    return null;
  }
}

export function saveProjectPath(path: string | null): void {
  try {
    if (typeof localStorage === "undefined") return;
    if (path === null) localStorage.removeItem(PROJECT_STORAGE_KEY);
    else localStorage.setItem(PROJECT_STORAGE_KEY, path);
  } catch {
    // Non-fatal: the choice simply does not survive a restart.
  }
}

export interface ProjectState {
  /** The chosen project root, or `null` when the owner chose none. */
  path: string | null;
  /**
   * Set (or clear) the project. A value that is not an absolute path clears
   * it — `setPath` is the only writer, so an unusable path can never be the
   * one the composer sends.
   */
  setPath: (path: string | null) => void;
}

export const useProjectStore = create<ProjectState>((set) => ({
  path: loadProjectPath(),
  setPath: (path) => {
    const next = normalizeProjectPath(path);
    set({ path: next });
    saveProjectPath(next);
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
