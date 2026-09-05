/**
 * Artifact kinds and their badge abbreviations (DESIGN_SPEC §3.22).
 *
 * The design names seven kinds. The proposed artifact API (API_MAP §3, GAP-04)
 * names eight — its `markdown`/`terminal` spellings plus a `binary` catch-all —
 * so the translation lives here rather than in a view.
 *
 * The `code` badge is language-derived (`RS` in the design). With no language
 * the honest abbreviation is `SRC`, not a guessed one.
 */

import type { ArtifactKind } from "@/lib/api/unbacked";

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
  return derived === "" ? KIND_ABBR.code : derived;
}

/** `findings.md` → `md`; used when only a filename is known. */
export function languageFromName(name: string): string | null {
  const dot = name.lastIndexOf(".");
  if (dot < 0 || dot === name.length - 1) return null;
  return name.slice(dot + 1).toLowerCase();
}
