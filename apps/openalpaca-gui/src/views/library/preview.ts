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
 *     state is the reason `Artifact.missing` exists — never a blank pane.
 *   * a file written into the project (storage design §13) that the daemon
 *     marked missing is **read again**: R37 clears the mark on the read that
 *     finds it, so a file the person put back is shown (d7-app-1). Only a
 *     failed read says why — `previewNote`: the file is not at its path in
 *     the project, or the daemon's own sentence for a project whose folder is
 *     gone or has moved (R-T26) — never a guess before the read. An image is
 *     read by the browser's load, whose failure carries no status, so a
 *     marked image that does not load says what the mark means — the last
 *     read did not find it — and a marked binary or too-large file, which is
 *     not read at all, says the same; neither sends the person to Export or
 *     Open, which fail the same way while the file cannot be found (X14,
 *     Y.4).
 *
 * A note names `Open` only for a row the daemon opens (`openable`, ST-18):
 * a project file of another kind — a `.sh`, a `Makefile` — is sent to Export.
 */

import type { FileKind } from "@/components/ui";
import { toFileKind } from "@/components/ui";
import type { Artifact } from "@/lib/api/artifacts";
import { formatFileSize } from "@/lib/api/types";
import { escapeHiddenLine, hiddenNotice } from "@/lib/hidden-chars";
import { ApiError } from "@/lib/http";

/** Past this, the Preview tab points at Export instead of fetching. */
export const MAX_PREVIEW_BYTES = 512 * 1024;

export type PreviewPlan =
  /**
   * Load `…/content?token=` into an `<img>`. `failedNote` is what the
   * placeholder says when that load fails, in place of its own sentence;
   * `null` keeps the placeholder's.
   */
  | { mode: "image"; failedNote: string | null }
  /** Fetch the bytes as text and render them with this kind's renderer. */
  | { mode: "text"; kind: FileKind; note: string | null }
  /** Draw nothing but the sentence. */
  | { mode: "none"; note: string };

/**
 * What a result file the daemon marked missing says when nothing is read
 * that could say more (X14): the mark is a read that did not find the file —
 * not at its path, or its project missing (R-T18) — and neither an image's
 * load nor a plan that reads nothing gives a status to tell the two apart,
 * so the note names both and claims neither.
 */
const MARKED_NOTE =
  "This file could not be read the last time it was asked for: it was not at its path in the project, or the project's folder is gone or has moved.";

/**
 * A marked result image whose load fails. It promises only what an open
 * pane keeps (Y.4): nothing watches the file, so the image is loaded again
 * when the preview is opened again — or, in an open one, when the row read
 * again comes back unmarked (`ImagePreview` tries anew when its
 * `failedNote` changes).
 */
const MARKED_IMAGE_NOTE = `${MARKED_NOTE} Its row is kept, and the image is loaded again the next time this preview is opened.`;

/** `image/svg+xml`, `;charset=` and casing aside. */
function isSvg(mimeType: string): boolean {
  return mimeType.split(";")[0]?.trim().toLowerCase() === "image/svg+xml";
}

export function previewPlan(artifact: Artifact): PreviewPlan {
  if (artifact.missing && !artifact.in_project) {
    return {
      mode: "none",
      note: "This file is no longer on disk. The daemon kept its record, so the history below is still real.",
    };
  }

  const plan = presentPlan(artifact);
  if (!artifact.missing) return plan;
  // A result file the daemon marked missing is planned as if it were there:
  // the read is what finds it again (R37). A plan that reads nothing says
  // what the mark means — the last read failed — without claiming the file
  // is gone, which is false for a project moved behind a link (R-T26), and
  // without sending the person to Export or Open, which read the file the
  // same way and fail the same way while it cannot be found (X14, Y.4).
  if (plan.mode === "image") {
    return { mode: "image", failedNote: MARKED_IMAGE_NOTE };
  }
  const unread = unreadFile(artifact);
  if (unread !== null) {
    return { mode: "none", note: `${MARKED_NOTE} ${unread.what}.` };
  }
  return plan;
}

/** The plan for a row whose bytes the daemon has not marked missing. */
function presentPlan(artifact: Artifact): PreviewPlan {
  const markup = artifact.kind === "html" || isSvg(artifact.mime_type);

  if (artifact.kind === "image" && !markup) {
    return { mode: "image", failedNote: null };
  }

  const unread = unreadFile(artifact);
  if (unread !== null) {
    return { mode: "none", note: `${unread.what} — ${unread.then}` };
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
 * A file the Preview draws nothing of — binary bytes, or more than it
 * fetches — said as what it is and what Export (and Open, where it is
 * drawn) does instead; `null` for a file the Preview reads.
 */
function unreadFile(artifact: Artifact): { what: string; then: string } | null {
  const size = formatFileSize(artifact.size_bytes);
  // `Open` is named only where it is drawn: a row the daemon opens (ST-18).
  if (artifact.kind === "binary") {
    return {
      what: `Binary file, ${size}`,
      then: artifact.openable
        ? "Export or Open opens it in its own app."
        : "Export saves it.",
    };
  }
  if (artifact.size_bytes > MAX_PREVIEW_BYTES) {
    return {
      what: `Too large to preview here (${size})`,
      then: artifact.openable
        ? "Export or Open opens the whole file."
        : "Export saves the whole file.",
    };
  }
  return null;
}

/** How the bytes' read is going, as React Query reports it. */
export interface PreviewRead {
  error: Error | null;
  isLoading: boolean;
}

/**
 * The note under (or in place of) a text or `none` plan's preview — one
 * ladder for the Library detail and the chat file panel, so the two cannot
 * drift: the plan's own note when nothing is read, the read's failure, the
 * wait, then the plan's note again.
 *
 * A failed read is said in the person's terms. For a file written into the
 * project a `410 ARTIFACT_GONE` means the file is not at its path there;
 * `410 ARTIFACT_PROJECT_MISSING` is the daemon's sentence for a project
 * whose folder is gone or has moved (R-T26), shown as it comes. Every
 * refusal carries a path an agent may have chosen, so it is escaped
 * (`lib/hidden-chars`) — never drawn raw.
 */
export function previewNote(
  artifact: Artifact,
  plan: Exclude<PreviewPlan, { mode: "image" }>,
  read: PreviewRead,
): string | null {
  if (plan.mode === "none") return plan.note;
  if (read.error !== null) {
    const { error } = read;
    if (
      error instanceof ApiError &&
      error.status === 410 &&
      error.code === "ARTIFACT_GONE" &&
      artifact.in_project
    ) {
      return missingProjectFileNote(artifact.rel_path);
    }
    return escapeHiddenLine(error.message).text;
  }
  if (read.isLoading) return "Reading the file…";
  return plan.note;
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
