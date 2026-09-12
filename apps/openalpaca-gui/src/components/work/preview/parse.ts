/**
 * Artifact bytes → the renderer models of `types.ts`.
 *
 * These are the only place a preview decides what its content *means*, which
 * is why they are pure and tested: a mis-parsed CSV silently invents a column,
 * and inventing data is the one thing this build must not do.
 */

import type { PlanStep, TableModel, TerminalLine } from "./types";

// ── Tables (§3.25d) ─────────────────────────────────────────────────────────

/** Tab wins when the first line has more tabs than commas. */
export function detectDelimiter(text: string): string {
  const firstLine = text.split("\n", 1)[0] ?? "";
  const tabs = (firstLine.match(/\t/g) ?? []).length;
  const commas = (firstLine.match(/,/g) ?? []).length;
  return tabs > commas ? "\t" : ",";
}

/**
 * RFC 4180-ish: quoted fields may contain the delimiter, a newline, or a
 * doubled quote. Anything else is taken literally.
 */
export function parseDelimited(text: string, delimiter?: string): string[][] {
  const sep = delimiter ?? detectDelimiter(text);
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;

  const endField = (): void => {
    row.push(field);
    field = "";
  };
  const endRow = (): void => {
    endField();
    rows.push(row);
    row = [];
  };

  const source = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  for (let i = 0; i < source.length; i += 1) {
    const char = source[i] as string;
    if (quoted) {
      if (char === '"') {
        if (source[i + 1] === '"') {
          field += '"';
          i += 1;
        } else {
          quoted = false;
        }
      } else {
        field += char;
      }
      continue;
    }
    if (char === '"' && field === "") {
      quoted = true;
      continue;
    }
    if (char === sep) {
      endField();
      continue;
    }
    if (char === "\n") {
      endRow();
      continue;
    }
    field += char;
  }
  if (field !== "" || row.length > 0) endRow();

  // A trailing newline leaves one empty row; a genuinely blank row does not
  // exist in a table.
  return rows.filter((cells) => cells.some((cell) => cell.trim() !== ""));
}

/** First row is the header. An empty document has no columns and no rows. */
export function parseTable(text: string, delimiter?: string): TableModel {
  const rows = parseDelimited(text, delimiter);
  const [header, ...body] = rows;
  if (header === undefined) return { columns: [], rows: [] };
  return {
    columns: header.map((cell) => cell.trim()),
    // Ragged rows are padded so a cell never lands under the wrong column.
    rows: body.map((cells) =>
      header.map((_column, index) => cells[index]?.trim() ?? ""),
    ),
  };
}

const NUMERIC = /^[-+]?[$€£]?\d[\d,]*(\.\d+)?%?$/;
const IDENTIFIER = /^[A-Za-z0-9_.:/@-]+$/;

/** Identifier and numeric columns are drawn in mono (§3.25d). */
export function isMonoColumn(values: readonly string[]): boolean {
  const filled = values.filter((value) => value.trim() !== "");
  if (filled.length === 0) return false;
  return filled.every(
    (value) => NUMERIC.test(value.trim()) || IDENTIFIER.test(value.trim()),
  );
}

/** A yes/no column is the only one that gets green/red cells. */
export function isBooleanColumn(values: readonly string[]): boolean {
  const filled = values.filter((value) => value.trim() !== "");
  if (filled.length === 0) return false;
  return filled.every((value) => /^(yes|no)$/i.test(value.trim()));
}

export function columnValues(table: TableModel, index: number): string[] {
  return table.rows.map((row) => row[index] ?? "");
}

/**
 * Column weights (§3.25d): the first column is widest, the rest even, and the
 * compact table's last of three narrows to `.8`.
 */
export function columnFlex(
  index: number,
  count: number,
  size: "compact" | "full",
): number {
  if (index === 0) return size === "compact" ? 1.6 : 2;
  if (size === "compact" && count === 3 && index === count - 1) return 0.8;
  return 1;
}

// ── Plans (§3.25e) ──────────────────────────────────────────────────────────

const CHECKBOX = /^\s*(?:[-*]|\d+[.)])\s*\[([ xX!~])\]\s*(.+)$/;

/**
 * A markdown checklist. `[x]` is complete, `[!]` (or `[~]`) is blocked, `[ ]`
 * is pending; a trailing `— note` becomes the mono annotation the design puts
 * after a blocked step.
 */
export function parsePlan(text: string): PlanStep[] {
  const steps: PlanStep[] = [];
  for (const line of text.split("\n")) {
    const match = CHECKBOX.exec(line);
    if (match === null) continue;
    const mark = match[1] as string;
    let label = (match[2] as string).trim();
    let note: string | null = null;

    const dash = label.lastIndexOf(" — ");
    if (dash > 0) {
      note = label.slice(dash + 3).trim();
      label = label.slice(0, dash).trim();
    }

    const state =
      mark === "x" || mark === "X"
        ? "complete"
        : mark === " "
          ? "pending"
          : "blocked";
    if (state === "blocked" && note === null) note = "awaiting approval";
    steps.push({ label, state, note });
  }
  return steps;
}

/** "5 of 8 complete" — the progress eyebrow. */
export function planProgress(steps: readonly PlanStep[]): string {
  const done = steps.filter((step) => step.state === "complete").length;
  return `${done} of ${steps.length} complete`;
}

// ── Terminal output (§3.25c) ────────────────────────────────────────────────

/** `$ …` / `> …` lines are the command echo and are drawn dimmer. */
export function parseTerminal(text: string): TerminalLine[] {
  const source = text.endsWith("\n") ? text.slice(0, -1) : text;
  if (source === "") return [];
  return source
    .split("\n")
    .map((line) => ({ text: line, prompt: /^\s*[$>]\s/.test(line) }));
}
