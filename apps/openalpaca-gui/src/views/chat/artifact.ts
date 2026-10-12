/**
 * File → artifact presentation helpers for the transcript.
 *
 * A message's `attachments` and a `done` frame's `attachments_used` are file
 * ids that `GET /v1/files/{id}` answers for, with a real filename, mime type
 * and `extracted_text`; the same id is an artifact id, so the panel reads the
 * row, its versions and its bytes. Nothing here is synthesised.
 */

import {
  artifactKindForMime,
  languageFromName,
  toFileKind,
  type FileKind,
} from "@/components/ui";
import type { ArtifactKind } from "@/lib/api/artifacts";

/**
 * The badge a transcript file reads: the Library's mapping (G3a). The kind the
 * daemon stored when the server named it (an artifact link), else the kind an
 * upload of that type is stored with — what the Library badges that row — so
 * a `.txt` reads `TXT` here as it does there. The language half of the badge
 * is the filename's in both places (`fileLanguage`).
 */
export function fileKind(
  kind: string | null,
  mimeType?: string | null,
): FileKind {
  return toFileKind(
    (kind as ArtifactKind | null) ?? artifactKindForMime(mimeType),
  );
}

export function fileLanguage(filename: string): string | null {
  return languageFromName(filename);
}

export interface TextPreview {
  lines: string[];
  /** `… n more lines`, or `null` when the preview is the whole file. */
  remaining: number | null;
}

/** The first `maxLines` lines of a file's extracted text. */
export function textPreview(
  text: string | null | undefined,
  maxLines = 6,
): TextPreview {
  if (text === null || text === undefined || text.trim() === "") {
    return { lines: [], remaining: null };
  }
  const all = text.replace(/\s+$/, "").split("\n");
  const lines = all.slice(0, maxLines);
  const remaining = all.length > maxLines ? all.length - maxLines : null;
  return { lines, remaining };
}
