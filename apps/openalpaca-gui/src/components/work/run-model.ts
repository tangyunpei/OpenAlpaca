/**
 * The `Run` view model (DESIGN_SPEC §4.3) derived from a daemon `Task`.
 *
 * The design's `Run` carries several hand-written fields the daemon does not
 * serve directly. Each is handled explicitly rather than invented:
 *
 *   `short`  — there is no short title on the wire, so the full `title` is
 *              rendered and truncated by CSS (same call `RunningNowSection`
 *              made).
 *   `meta`   — `11m 04s · 5/8 steps · $0.41`. Duration and steps are real
 *              (`created_at`→`completed_at`, `progress_current/total`); the
 *              cost segment is included only when `task.cost_usd` is present
 *              (GAP-08b, closed — `GET /v1/tasks` list rows carry it, the
 *              single-task detail route does not yet), never estimated.
 *   `note`   — the daemon's own `result_summary` / `outcome_summary`, first
 *              line only. No note ⇒ no note row.
 *   `stamp`  — `completed_at` (falling back to `updated_at`) as `HH:MM`.
 *   `files`  — read out of `task.outcome.artifacts`, which is free-form JSON
 *              (`Value[]`), so entries that carry no readable name are dropped
 *              rather than rendered as a blank row (GAP-04 is why the shape is
 *              schema-less in the first place).
 *
 * Everything here is pure so the Work view's status → visual mapping can be
 * tested without a DOM.
 */

import {
  isLive,
  toUiStatus,
  type FileKind,
  type UiStatus,
} from "@/components/ui";
import type { ParsedOutcome, Task } from "@/lib/api/types";

// ── Timestamps ──────────────────────────────────────────────────────────────

/**
 * The daemon serializes `DateTime<Utc>`, i.e. RFC 3339 with a `Z`. Older rows
 * written as `"YYYY-MM-DD HH:MM:SS"` have no timezone and would be parsed as
 * local time by `Date`, so they are normalised to UTC first.
 */
export function parseTimestamp(value: string | null | undefined): Date | null {
  if (value === null || value === undefined || value.trim() === "") return null;
  let text = value.trim();
  if (!text.includes("T") && text.includes(" ")) text = text.replace(" ", "T");
  if (!/(Z|[+-]\d{2}:?\d{2})$/.test(text)) text = `${text}Z`;
  const date = new Date(text);
  return Number.isNaN(date.getTime()) ? null : date;
}

const pad2 = (n: number): string => String(Math.floor(n)).padStart(2, "0");

/** `41s` · `11m 04s` · `4h 02m` — the design's three shapes. */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  if (total < 60) return `${total}s`;
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return `${minutes}m ${pad2(total % 60)}s`;
  return `${Math.floor(minutes / 60)}h ${pad2(minutes % 60)}m`;
}

/** `14:22:41` (detail header) or `13:41` (completed row). */
export function formatClock(
  value: string | null | undefined,
  withSeconds = false,
): string | null {
  const date = parseTimestamp(value);
  if (date === null) return null;
  const hh = pad2(date.getHours());
  const mm = pad2(date.getMinutes());
  return withSeconds ? `${hh}:${mm}:${pad2(date.getSeconds())}` : `${hh}:${mm}`;
}

/** Same calendar day in the viewer's timezone — drives "Completed today". */
export function isSameLocalDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

// ── Artifacts referenced by a run's outcome ─────────────────────────────────

/**
 * One entry of `task.outcome.artifacts`. The daemon writes free-form JSON here
 * (GAP-04: artifacts are not a resource), so every field is optional and the
 * parser keeps only what it can actually read.
 */
export interface OutcomeArtifact {
  /** `null` when the entry carries no id — such a row cannot be opened. */
  id: string | null;
  name: string;
  kind: FileKind;
  stamp: string | null;
}

const EXTENSION_KIND: Record<string, FileKind> = {
  md: "md",
  markdown: "md",
  txt: "term",
  log: "term",
  out: "term",
  csv: "table",
  tsv: "table",
  html: "html",
  htm: "html",
  png: "image",
  jpg: "image",
  jpeg: "image",
  gif: "image",
  webp: "image",
  svg: "image",
};

const DECLARED_KIND: Record<string, FileKind> = {
  md: "md",
  markdown: "md",
  doc: "md",
  document: "md",
  code: "code",
  source: "code",
  term: "term",
  terminal: "term",
  output: "term",
  binary: "term",
  table: "table",
  data: "table",
  csv: "table",
  plan: "plan",
  checklist: "plan",
  image: "image",
  html: "html",
  web: "html",
};

/** Filename → the badge kind. Unknown extensions read as source. */
export function kindFromName(name: string): FileKind {
  const dot = name.lastIndexOf(".");
  if (dot < 0 || dot === name.length - 1) return "term";
  const ext = name.slice(dot + 1).toLowerCase();
  return EXTENSION_KIND[ext] ?? "code";
}

function readString(
  source: Record<string, unknown>,
  keys: string[],
): string | null {
  for (const key of keys) {
    const value = source[key];
    if (typeof value === "string" && value.trim() !== "") return value.trim();
  }
  return null;
}

/**
 * `ParsedOutcome.artifacts` → renderable rows. Entries without a readable name
 * are skipped: a row that says nothing is worse than no row.
 */
export function parseOutcomeArtifacts(
  artifacts: readonly unknown[] | undefined,
): OutcomeArtifact[] {
  if (artifacts === undefined) return [];
  const parsed: OutcomeArtifact[] = [];
  for (const entry of artifacts) {
    if (typeof entry === "string") {
      if (entry.trim() === "") continue;
      const name = entry.trim();
      parsed.push({ id: null, name, kind: kindFromName(name), stamp: null });
      continue;
    }
    if (typeof entry !== "object" || entry === null) continue;
    const source = entry as Record<string, unknown>;
    const name = readString(source, ["name", "filename", "path", "title"]);
    if (name === null) continue;
    const declared = readString(source, ["kind", "type"]);
    const kind =
      declared === null
        ? kindFromName(name)
        : (DECLARED_KIND[declared.toLowerCase()] ?? kindFromName(name));
    parsed.push({
      id: readString(source, ["id", "artifact_id", "file_id"]),
      name,
      kind,
      stamp: formatClock(
        readString(source, ["created_at", "updated_at", "at"]),
      ),
    });
  }
  return parsed;
}

// ── The run itself ──────────────────────────────────────────────────────────

export interface Run {
  id: string;
  title: string;
  status: UiStatus;
  /** `11m 04s · 5/8 steps · $0.41` — the cost segment only when known. */
  meta: string;
  /** `14:22:41`, or `null` if `created_at` is unreadable. */
  started: string | null;
  /** `13:41` — the completed row's right-hand stamp. */
  stamp: string | null;
  /** One-line human status, or `null`. */
  note: string | null;
  laneKey: string;
  artifactCount: number;
  artifacts: OutcomeArtifact[];
  /** `completed_at ?? updated_at`, for the "today" partition. */
  finishedAt: Date | null;
  /** `task.cost_usd` (list route only — GAP-08b) — `null`, never guessed. */
  costUsd: number | null;
}

/**
 * `isTerminal` — done, cancelled or failed. The live/terminal split itself is
 * `isLive` from the primitive set; this is only its complement, kept here so
 * the Work view does not spell the negation out at ten call sites.
 */
export function isTerminalRun(status: UiStatus): boolean {
  return !isLive(status);
}

/**
 * The design's run-card `active` — **running or paused** (§3.19), which is a
 * narrower set than §4.2's `activeCount` (`isActive`, which also counts
 * `queued`). Two different words for two different sets; do not merge them.
 */
export function isRaisedRun(status: UiStatus): boolean {
  return status === "running" || status === "paused";
}

function firstLine(value: string | null | undefined): string | null {
  if (value === null || value === undefined) return null;
  const line = value.split("\n").find((part) => part.trim() !== "");
  return line === undefined ? null : line.trim();
}

/** Wall clock so far: `created_at` → `completed_at`, or → now while live. */
export function runDurationMs(task: Task, now: Date): number | null {
  const started = parseTimestamp(task.created_at);
  if (started === null) return null;
  const status = toUiStatus(task.status);
  const ended = isTerminalRun(status)
    ? (parseTimestamp(task.completed_at) ?? parseTimestamp(task.updated_at))
    : now;
  if (ended === null) return null;
  return Math.max(0, ended.getTime() - started.getTime());
}

/**
 * `11m 04s · 5/8 steps · $0.41`. The cost segment is added only when
 * `task.cost_usd` is a number — omitted, never estimated, when it is not.
 */
export function runMeta(task: Task, now: Date): string {
  const segments: string[] = [];
  const duration = runDurationMs(task, now);
  if (duration !== null) segments.push(formatDuration(duration));
  const { progress_current: current, progress_total: total } = task;
  if (total !== null && total > 0) {
    segments.push(`${current ?? 0}/${total} steps`);
  }
  if (typeof task.cost_usd === "number") {
    segments.push(`$${task.cost_usd.toFixed(2)}`);
  }
  return segments.join(" · ");
}

function outcomeOf(task: Task): ParsedOutcome | undefined {
  return task.outcome;
}

export function toRun(task: Task, now: Date = new Date()): Run {
  const outcome = outcomeOf(task);
  const artifacts = parseOutcomeArtifacts(outcome?.artifacts);
  return {
    id: task.id,
    title: task.title,
    status: toUiStatus(task.status),
    meta: runMeta(task, now),
    started: formatClock(task.created_at, true),
    stamp: formatClock(task.completed_at ?? task.updated_at),
    note: firstLine(task.result_summary ?? outcome?.outcome_summary ?? null),
    laneKey: task.source_lane,
    artifactCount:
      task.artifact_count ?? outcome?.artifact_count ?? artifacts.length,
    artifacts,
    finishedAt:
      parseTimestamp(task.completed_at) ?? parseTimestamp(task.updated_at),
    costUsd: typeof task.cost_usd === "number" ? task.cost_usd : null,
  };
}

export interface RunPartition {
  /** `listRuns` / `paneRuns` — every run whose status is not `done` (§4.2). */
  live: Run[];
  /** `doneRuns`, newest first, limited to the current local day. */
  completedToday: Run[];
  /** `activeCount` — running, queued or paused. */
  activeCount: number;
  /** `doneCount` — every `done` run in the fetched window. */
  doneCount: number;
}

/**
 * §4.2's derived collections. `done` is the only status the design moves below
 * the divider — `cancelled` and `failed` stay in the main list, which is what
 * the mock's seeded cancelled run does.
 */
export function partitionRuns(
  runs: readonly Run[],
  now: Date = new Date(),
): RunPartition {
  const live: Run[] = [];
  const done: Run[] = [];
  for (const run of runs) {
    if (run.status === "done") done.push(run);
    else live.push(run);
  }
  const completedToday = done
    .filter(
      (run) => run.finishedAt !== null && isSameLocalDay(run.finishedAt, now),
    )
    .sort(
      (a, b) => (b.finishedAt?.getTime() ?? 0) - (a.finishedAt?.getTime() ?? 0),
    );
  return {
    live,
    completedToday,
    activeCount: runs.filter((run) => isLive(run.status)).length,
    doneCount: done.length,
  };
}
