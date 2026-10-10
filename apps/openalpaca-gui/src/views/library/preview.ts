/**
 * What the Preview tab does with an artifact's bytes.
 *
 * One decision, in one place, because the same question is asked twice (the
 * Library detail at `size="full"`, the chat file panel at `"compact"`) and the
 * two must not drift:
 *
 *   * an **image** is loaded by the browser from `…/content?token=` — the
 *     content routes check the token inline for exactly this;
 *   * **HTML and SVG** are shown as *source*. Rendering agent-authored markup
 *     in the webview is a security review (`frame-src`/sandbox), deliberately
 *     deferred; showing the characters is not;
 *   * **binary** bytes are not characters, so nothing is drawn — the row says
 *     what it is and Export/Open open it;
 *   * **too large** is refused before the fetch: pulling a multi-megabyte file
 *     into the webview to render six lines of it helps nobody;
 *   * a **missing** row (its bytes deleted under the daemon) says so. That
 *     state is the reason `Artifact.missing` exists — never a blank pane. A
 *     file written into the project (storage design §13) has no history and
 *     its row is kept, so its note names its path in the project instead.
 *
 * A note names `Open` only for a row the daemon opens (`openable`, ST-18):
 * a project file of another kind — a `.sh`, a `Makefile` — is sent to Export.
 */

import type { FileKind } from "@/components/ui";
import { toFileKind } from "@/components/ui";
import type { Artifact } from "@/lib/api/artifacts";
import { formatFileSize } from "@/lib/api/types";
import { escapeHiddenLine, hiddenNotice } from "@/lib/hidden-chars";

/** Past this, the Preview tab points at Export instead of fetching. */
export const MAX_PREVIEW_BYTES = 512 * 1024;

export type PreviewPlan =
  /** Load `…/content?token=` into an `<img>`. */
  | { mode: "image" }
  /** Fetch the bytes as text and render them with this kind's renderer. */
  | { mode: "text"; kind: FileKind; note: string | null }
  /** Draw nothing but the sentence. */
  | { mode: "none"; note: string };

/** `image/svg+xml`, `;charset=` and casing aside. */
function isSvg(mimeType: string): boolean {
  return mimeType.split(";")[0]?.trim().toLowerCase() === "image/svg+xml";
}

export function previewPlan(artifact: Artifact): PreviewPlan {
  if (artifact.missing) {
    return {
      mode: "none",
      note: artifact.in_project
        ? missingProjectFileNote(artifact.rel_path)
        : "This file is no longer on disk. The daemon kept its record, so the history below is still real.",
    };
  }

  const markup = artifact.kind === "html" || isSvg(artifact.mime_type);

  if (artifact.kind === "image" && !markup) return { mode: "image" };

  // `Open` is named only where it is drawn: a row the daemon opens (ST-18).
  const binaryNote = artifact.openable
    ? "Export or Open opens it in its own app."
    : "Export saves it.";
  const wholeNote = artifact.openable
    ? "Export or Open opens the whole file."
    : "Export saves the whole file.";

  if (artifact.kind === "binary") {
    return {
      mode: "none",
      note: `Binary file, ${formatFileSize(artifact.size_bytes)} — ${binaryNote}`,
    };
  }

  if (artifact.size_bytes > MAX_PREVIEW_BYTES) {
    return {
      mode: "none",
      note: `Too large to preview here (${formatFileSize(artifact.size_bytes)}) — ${wholeNote}`,
    };
  }

  if (markup) {
    return {
      mode: "text",
      kind: "code",
      note: "Shown as source: rendering agent-authored markup in the app is a separate security review.",
    };
  }

  return { mode: "text", kind: toFileKind(artifact.kind), note: null };
}

/**
 * A missing project file's note. Its path is the one an agent wrote it at, so
 * it is shown through `lib/hidden-chars` — each hidden character as
 * `<U+XXXX>` — and the note ends with the count when there is one.
 */
function missingProjectFileNote(relPath: string): string {
  const path = escapeHiddenLine(relPath);
  const sentence = `This file is no longer at ${path.text} in the project. Its row is kept, and the file is shown again once it is back.`;
  const notice = hiddenNotice(path.hidden);
  return notice === null ? sentence : `${sentence} ${notice}`;
}

/**
 * The version pair the Diff tab asks about: the newest change, i.e. the head
 * against the version before it. `null` when there is no pair.
 */
export function diffPair(
  artifact: Artifact,
): { from: number; to: number } | null {
  if (artifact.version_count < 2 || artifact.version < 2) return null;
  return { from: artifact.version - 1, to: artifact.version };
}
