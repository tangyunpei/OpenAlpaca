/**
 * `Reveal in Finder` — show an artifact's file in its folder (Round G, G4).
 *
 * The one action here the daemon cannot do: the daemon may run on another
 * host, but the folder has to open on this one, so the Tauri shell's
 * `reveal_path` command does it. The shell checks the path itself (a regular
 * file in an OpenAlpaca store, symlinks resolved) and runs the platform's
 * reveal with the path as an argument. *Opening* a file is the daemon's route
 * (`POST /v1/files/{id}/open`, `useOpenFile`) and is labelled `Open`.
 */

import { invoke } from "@tauri-apps/api/core";

/**
 * Whether this window can reveal a file: only inside the Tauri shell. In the
 * Vite preview (or jsdom) there is no shell to ask, so nothing draws the
 * button.
 *
 * Reads the marker the shell injects — the same one `@tauri-apps/api/core`'s
 * `isTauri()` reads — directly, so a test that mocks that module with
 * `invoke` alone does not have to stub it too.
 */
export function canReveal(): boolean {
  return (globalThis as { isTauri?: unknown }).isTauri === true;
}

/** "Reveal in Finder" on macOS, where Finder is the name people know; "Show in folder" elsewhere. */
export function revealLabel(
  userAgent: string = globalThis.navigator?.userAgent ?? "",
): string {
  return /Macintosh|Mac OS X/.test(userAgent)
    ? "Reveal in Finder"
    : "Show in folder";
}

/**
 * Ask the shell to show `path` in its folder. Rejects with the shell's own
 * sentence — it is written to be shown as it is.
 */
export async function revealInFolder(path: string): Promise<void> {
  try {
    await invoke("reveal_path", { path });
  } catch (cause) {
    throw new Error(
      typeof cause === "string"
        ? cause
        : "The file could not be shown in its folder.",
    );
  }
}
