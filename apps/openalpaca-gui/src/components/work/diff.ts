/**
 * Unified-diff parsing for the `Diff` tab (DESIGN_SPEC §3.25 `DiffTab`) and for
 * the code preview's added/removed line states (§3.25b).
 *
 * The proposed artifact-diff endpoint returns `{ format: "unified", patch }`
 * (API_MAP GAP-05), so the client parses a real patch rather than inventing a
 * line model. Nothing here renders; it is the parse step, and it is where the
 * `+9 / −2` counters come from — those are *counted*, never taken on trust
 * from a caller.
 */

export type DiffLineKind = "context" | "added" | "removed" | "hunk" | "meta";

export interface DiffLine {
  kind: DiffLineKind;
  /** The line without its `+`/`-`/space marker. Hunk and meta keep theirs. */
  text: string;
  /** Line number in the old file, or `null` for an added line. */
  oldNumber: number | null;
  /** Line number in the new file, or `null` for a removed line. */
  newNumber: number | null;
}

export interface ParsedDiff {
  lines: DiffLine[];
  added: number;
  removed: number;
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

/** `diff --git`, `index …`, `--- a/x`, `+++ b/x`, `\ No newline …`. */
function isFileHeader(line: string): boolean {
  return (
    line.startsWith("diff --git") ||
    line.startsWith("index ") ||
    line.startsWith("--- ") ||
    line.startsWith("+++ ") ||
    line.startsWith("\\ ") ||
    line.startsWith("new file mode") ||
    line.startsWith("deleted file mode") ||
    line.startsWith("similarity index") ||
    line.startsWith("rename ")
  );
}

export function parseUnifiedDiff(patch: string): ParsedDiff {
  const lines: DiffLine[] = [];
  let added = 0;
  let removed = 0;
  let oldNumber = 1;
  let newNumber = 1;

  // A trailing newline must not produce a phantom empty context line.
  const source = patch.endsWith("\n") ? patch.slice(0, -1) : patch;
  if (source === "") return { lines, added, removed };

  for (const raw of source.split("\n")) {
    const hunk = HUNK.exec(raw);
    if (hunk !== null) {
      oldNumber = Number(hunk[1]);
      newNumber = Number(hunk[2]);
      lines.push({ kind: "hunk", text: raw, oldNumber: null, newNumber: null });
      continue;
    }
    if (isFileHeader(raw)) {
      lines.push({ kind: "meta", text: raw, oldNumber: null, newNumber: null });
      continue;
    }
    if (raw.startsWith("+")) {
      added += 1;
      lines.push({
        kind: "added",
        text: raw.slice(1),
        oldNumber: null,
        newNumber: newNumber++,
      });
      continue;
    }
    if (raw.startsWith("-")) {
      removed += 1;
      lines.push({
        kind: "removed",
        text: raw.slice(1),
        oldNumber: oldNumber++,
        newNumber: null,
      });
      continue;
    }
    lines.push({
      kind: "context",
      // A context line carries a leading space in a well-formed patch; a
      // hand-written one may not, so only a real marker is stripped.
      text: raw.startsWith(" ") ? raw.slice(1) : raw,
      oldNumber: oldNumber++,
      newNumber: newNumber++,
    });
  }

  return { lines, added, removed };
}

/** `+41` / `−6` — the design's counters, with a real minus sign. */
export function formatDiffStat(
  added: number,
  removed: number,
): {
  added: string;
  removed: string;
} {
  return { added: `+${added}`, removed: `−${removed}` };
}

/** A plain source file as diff lines: every line is context. */
export function sourceAsDiffLines(source: string): DiffLine[] {
  const text = source.endsWith("\n") ? source.slice(0, -1) : source;
  if (text === "") return [];
  return text.split("\n").map((line, index) => ({
    kind: "context" as const,
    text: line,
    oldNumber: index + 1,
    newNumber: index + 1,
  }));
}
