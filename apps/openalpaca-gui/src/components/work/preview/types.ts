/**
 * The shapes the seven artifact renderers draw (DESIGN_SPEC §3.25).
 *
 * Each renderer is presentational: it takes an already-parsed model and never
 * fetches. The parsers in `parse.ts` build these from artifact bytes, so the
 * same components serve the compact (file panel) and full (library) sizes with
 * one prop.
 */

import type { FileKind } from "@/components/ui";

/** Compact = the chat aside's file panel; full = the Library detail. */
export type PreviewSize = "compact" | "full";

export interface TableModel {
  columns: string[];
  rows: string[][];
}

export type PlanStepState = "complete" | "blocked" | "pending";

export interface PlanStep {
  label: string;
  state: PlanStepState;
  /** Trailing mono annotation — "awaiting approval" on a blocked step. */
  note: string | null;
}

export interface TerminalLine {
  text: string;
  /** `$ …` — the command echo, drawn dimmer than its output. */
  prompt: boolean;
}

/** One bar of the HTML preview's chart (§3.25g). Heights are percentages. */
export interface ChartBar {
  label: string;
  height: number;
  /** De-emphasised bars are drawn in `#C9C2B5` rather than the accent. */
  emphasis: boolean;
}

/**
 * What a renderer needs beyond the bytes: the header strip's byline, the
 * terminal's exit code, the image's dimensions. Every field is nullable —
 * none of it is served today (GAP-05 records no per-kind metadata), so the
 * renderers must draw correctly without any of it.
 */
export interface PreviewMeta {
  name: string;
  kind: FileKind;
  /** `v2 of 2 · review_agent · 14:31` — whatever the caller can honestly say. */
  byline?: string | null;
  language?: string | null;
  addedLines?: number | null;
  removedLines?: number | null;
  exitCode?: number | null;
  duration?: string | null;
  width?: number | null;
  height?: number | null;
}
