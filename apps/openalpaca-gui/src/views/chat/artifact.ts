/**
 * File → artifact presentation helpers for the transcript.
 *
 * The daemon has no artifact resource (GAP-04), but it does have real files:
 * a message's `attachments` and a `done` frame's `attachments_used` are
 * `FileAsset` ids that `GET /v1/files/{id}` answers for, with a real filename,
 * mime type and `extracted_text`. Everything the chat shows about an artifact
 * comes from there — nothing is synthesised.
 */

import { languageFromName, type FileKind } from "@/components/ui";

const EXTENSION_KIND: Record<string, FileKind> = {
  md: "md",
  markdown: "md",
  txt: "md",
  html: "html",
  htm: "html",
  csv: "table",
  tsv: "table",
  log: "term",
  out: "term",
  json: "code",
  yaml: "code",
  yml: "code",
  toml: "code",
  rs: "code",
  ts: "code",
  tsx: "code",
  js: "code",
  jsx: "code",
  py: "code",
  go: "code",
  sh: "code",
  sql: "code",
  css: "code",
  png: "image",
  jpg: "image",
  jpeg: "image",
  gif: "image",
  webp: "image",
  svg: "image",
};

/** Extension first (it is the more specific signal), then the mime type. */
export function fileKind(filename: string, mimeType?: string | null): FileKind {
  const extension = languageFromName(filename);
  if (extension !== null) {
    const byExtension = EXTENSION_KIND[extension];
    if (byExtension !== undefined) return byExtension;
  }
  const mime = (mimeType ?? "").toLowerCase();
  if (mime.startsWith("image/")) return "image";
  if (mime === "text/html") return "html";
  if (mime === "text/csv") return "table";
  if (mime === "text/markdown") return "md";
  if (mime.startsWith("text/")) return "md";
  if (mime === "application/json") return "code";
  // Opaque bytes have no badge of their own; tool output is the nearest truth.
  return "term";
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
