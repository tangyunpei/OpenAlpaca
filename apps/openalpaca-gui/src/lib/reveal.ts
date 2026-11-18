/**
 * `Reveal in Finder` — show an artifact's file in its folder (Round G, G4).
 *
 * The daemon does it (Q06): `POST /v1/files/{id}/reveal` resolves the file id
 * to its stored file, checks it (a regular file in an OpenAlpaca store or
 * under the root of a registered project that is not missing, symlinks
 * resolved) and runs the platform's reveal on its own host — the one the
 * app's own shell starts and connects to — answering no path. This window
 * sends the id only, never a path, so nothing it holds is authority.
 * *Opening* a file is the daemon's route too (`POST /v1/files/{id}/open`,
 * `useOpenFile`) and is labelled `Open`.
 */

import { revealFile } from "./api/files";

/**
 * Whether this window offers Reveal: only inside the Tauri shell. In the Vite
 * preview (or jsdom) nothing draws the button.
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
 * Ask the daemon to show the file `id` in its folder. Rejects with the
 * daemon's own sentence, written to be shown as it is: `409 REVEAL_REFUSED`
 * for a file it does not show, `410 ARTIFACT_PROJECT_MISSING` for a missing
 * project's, `500 REVEAL_FAILED` for one the platform could not show
 * (`ApiError`, `lib/http.ts`).
 */
export async function revealInFolder(id: string): Promise<void> {
  await revealFile(id);
}
