/**
 * Artifact kinds and their badge abbreviations (DESIGN_SPEC §3.22).
 *
 * The design names seven kinds; `/v1/artifacts` serves eight — its
 * `markdown`/`terminal` spellings plus a `binary` catch-all — so the
 * translation lives here rather than in a view.
 *
 * The `code` badge is language-derived (`RS` in the design). With no language
 * the honest abbreviation is `SRC`, not a guessed one.
 */

import type { ArtifactKind } from "@/lib/api/artifacts";

export type FileKind =
  "md" | "code" | "plan" | "term" | "table" | "html" | "image";

export const FILE_KINDS: readonly FileKind[] = [
  "md",
  "code",
  "plan",
  "term",
  "table",
  "html",
  "image",
];

/** Fallback abbreviation per kind; `code` refines from the language. */
const KIND_ABBR: Record<FileKind, string> = {
  md: "MD",
  code: "SRC",
  plan: "PLN",
  term: "OUT",
  table: "CSV",
  html: "WEB",
  image: "IMG",
};

/** Extension or language id → the 2–3 letter badge text. */
const LANGUAGE_ABBR: Record<string, string> = {
  rs: "RS",
  rust: "RS",
  ts: "TS",
  typescript: "TS",
  tsx: "TSX",
  js: "JS",
  javascript: "JS",
  jsx: "JSX",
  py: "PY",
  python: "PY",
  go: "GO",
  sh: "SH",
  bash: "SH",
  zsh: "SH",
  sql: "SQL",
  toml: "TML",
  json: "JSN",
  yaml: "YML",
  yml: "YML",
  css: "CSS",
  swift: "SWF",
  java: "JAV",
  rb: "RB",
  ruby: "RB",
  c: "C",
  h: "H",
  cpp: "CPP",
};

/**
 * Non-`text/*` types the daemon stores as `code` — `CODE_MIME_TYPES` in
 * `crates/openalpaca_storage/src/models/artifact.rs`, kept equal by hand.
 */
const CODE_MIME_TYPES: ReadonlySet<string> = new Set([
  "application/json",
  "application/ld+json",
  "application/yaml",
  "application/x-yaml",
  "application/toml",
  "application/x-toml",
  "application/xml",
  "application/javascript",
  "application/ecmascript",
  "application/typescript",
  "application/sql",
  "application/x-python-code",
  "application/x-sh",
]);

/**
 * The kind the daemon stores for an upload of this type —
 * `ArtifactKind::for_mime` (R25), the kind the Library then badges. A surface
 * that knows a file's type but not its stored kind (a transcript file part)
 * reads it here rather than guessing a badge of its own (G3a): `text/plain` is
 * `code`, so a `.txt` reads `TXT` in both places, never `MD`.
 */
export function artifactKindForMime(
  mimeType: string | null | undefined,
): ArtifactKind {
  const essence = (mimeType ?? "").split(";")[0]?.trim().toLowerCase() ?? "";
  if (essence.startsWith("image/")) return "image";
  if (essence === "text/html" || essence === "application/xhtml+xml")
    return "html";
  if (essence === "text/markdown" || essence === "text/x-markdown")
    return "markdown";
  if (essence === "text/csv" || essence === "text/tab-separated-values")
    return "table";
  if (essence.startsWith("text/") || CODE_MIME_TYPES.has(essence))
    return "code";
  return "binary";
}

export function toFileKind(kind: ArtifactKind): FileKind {
  switch (kind) {
    case "markdown":
      return "md";
    case "terminal":
      return "term";
    case "binary":
      // No badge exists for opaque bytes; tool output is the nearest truth.
      return "term";
    default:
      return kind;
  }
}

/** `language` is an extension (`rs`) or a highlight.js id (`rust`). */
export function fileAbbr(kind: FileKind, language?: string | null): string {
  if (kind !== "code") return KIND_ABBR[kind];
  if (language === undefined || language === null) return KIND_ABBR.code;
  const key = language.trim().toLowerCase().replace(/^\./, "");
  const known = LANGUAGE_ABBR[key];
  if (known !== undefined) return known;
  const derived = key.slice(0, 3).toUpperCase();
  // The extension may be an agent's (a result file's own name): anything but
  // letters and digits — a bidi override, a zero-width space — is never drawn
  // on the badge (d7-app-3).
  return /^[A-Z0-9]+$/.test(derived) ? derived : KIND_ABBR.code;
}

/** `findings.md` → `md`; used when only a filename is known. */
export function languageFromName(name: string): string | null {
  const dot = name.lastIndexOf(".");
  if (dot < 0 || dot === name.length - 1) return null;
  return name.slice(dot + 1).toLowerCase();
}
