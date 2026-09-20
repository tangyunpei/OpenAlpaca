/**
 * The `Parallel work` swimlanes (DESIGN_SPEC §3.19, §3.21) in both sizes: the
 * compact block inside a run card and the wide block inside the work detail's
 * Timeline card.
 *
 * `GET /v1/tasks/{id}/timeline` serves this (Phase 4): one `subagent_span` per
 * lane, opened at the spawn rather than written at completion, so a lane that
 * is still working has a start and no end instead of no row at all. A run with
 * no lanes yet renders the design's own empty sentence — never invented bars.
 *
 * The red→green coupling of §4.4 lives in `LaneBar` and is driven from here by
 * one `blocked` flag: a `block` lane is red with an amber pending hatch only
 * while this run actually holds the confirmation.
 *
 * One honest limitation: §3.21's vocabulary has three lane states and none of
 * them is *failed*. A failed or cancelled span is drawn in the neutral
 * in-progress grey with its state spelled out in the trailing detail, because
 * painting it green ("done") would state something untrue and painting it red
 * would claim it is waiting on the user.
 */

import { Eyebrow, LaneAxis, LaneBar, type Lane } from "@/components/ui";
import { cn } from "@/lib/cn";
import type { TaskTimeline, TimelineLane } from "@/lib/api/tasks";

import { formatClock, parseTimestamp } from "./run-model";

/** Percent of the run's wall clock, clamped by `LaneBar` afterwards. */
function percent(at: Date, start: number, span: number): number {
  if (span <= 0) return 0;
  return ((at.getTime() - start) / span) * 100;
}

function laneState(state: TimelineLane["state"]): Lane["state"] {
  switch (state) {
    case "done":
      return "done";
    case "blocked":
      return "block";
    // `failed` / `cancelled` have no colour in §3.21; neutral grey plus the
    // detail text is the least-wrong reading of the design's vocabulary.
    default:
      return "run";
  }
}

function laneDetail(lane: TimelineLane): string | undefined {
  if (lane.detail !== null && lane.detail.trim() !== "") return lane.detail;
  if (lane.state === "failed" || lane.state === "cancelled") return lane.state;
  if (lane.steps_total !== undefined && lane.steps_total > 0) {
    return `${lane.steps_current ?? 0}/${lane.steps_total} steps`;
  }
  return undefined;
}

/** `TimelineLane[]` (absolute times) → `Lane[]` (percent of wall clock). */
export function lanesFromTimeline(timeline: TaskTimeline): Lane[] {
  const start = parseTimestamp(timeline.started_at);
  const end =
    parseTimestamp(timeline.completed_at) ?? parseTimestamp(timeline.now);
  if (start === null || end === null) return [];
  const span = end.getTime() - start.getTime();

  const lanes: Lane[] = [];
  for (const lane of timeline.lanes) {
    const laneStart = parseTimestamp(lane.started_at);
    if (laneStart === null) continue;
    const laneEnd = parseTimestamp(lane.ended_at) ?? end;
    lanes.push({
      label: lane.label,
      start: percent(laneStart, start.getTime(), span),
      end: percent(laneEnd, start.getTime(), span),
      state: laneState(lane.state),
      detail: laneDetail(lane),
    });
  }
  return lanes;
}

/** The wide axis's three ticks: start · midpoint · "HH:MM now" (§3.21). */
export function axisLabels(
  timeline: TaskTimeline,
): [string, string, string] | null {
  const start = parseTimestamp(timeline.started_at);
  const now = parseTimestamp(timeline.now);
  if (start === null || now === null) return null;
  const mid = new Date((start.getTime() + now.getTime()) / 2);
  const label = (date: Date): string =>
    `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
  const live = timeline.completed_at === null;
  return [
    label(start),
    label(mid),
    live
      ? `${label(now)} now`
      : (formatClock(timeline.completed_at) ?? label(now)),
  ];
}

// ── Compact: the run card's Parallel work block (§3.19) ─────────────────────

export interface ParallelWorkBlockProps {
  /** `null` while the run's timeline is loading, or if the read failed. */
  timeline: TaskTimeline | null;
  /** Why the read failed — a failed request, never a gap. */
  error?: string | null;
  /** This run holds the pending tool confirmation. */
  blocked?: boolean;
  className?: string;
}

export function ParallelWorkBlock({
  timeline,
  error = null,
  blocked = false,
  className,
}: ParallelWorkBlockProps) {
  const lanes = timeline === null ? [] : lanesFromTimeline(timeline);

  return (
    <div
      className={cn("mt-[13px] border-t border-line-hair pt-[12px]", className)}
    >
      <div className="mb-[8px] flex items-center justify-between">
        <Eyebrow tone="faint">Parallel work</Eyebrow>
        <Eyebrow tone="faint">now →</Eyebrow>
      </div>

      {lanes.length > 0 ? (
        <div className="flex flex-col gap-[6px]">
          {lanes.map((lane) => (
            <LaneBar
              key={lane.label}
              lane={lane}
              size="compact"
              blocked={blocked}
            />
          ))}
        </div>
      ) : (
        <p className="m-0 font-mono text-2xs-plus leading-[1.5] text-faint">
          {error !== null
            ? `Could not load this run's timeline — ${error}`
            : "No subagent spans yet."}
        </p>
      )}
    </div>
  );
}

// ── Wide: the work detail's Timeline card (§5.2) ────────────────────────────

export interface TimelineLanesProps {
  timeline: TaskTimeline;
  blocked?: boolean;
}

export function TimelineLanes({
  timeline,
  blocked = false,
}: TimelineLanesProps) {
  const lanes = lanesFromTimeline(timeline);
  const axis = axisLabels(timeline);

  return (
    <div>
      {axis !== null && <LaneAxis labels={axis} />}
      <div className="flex flex-col gap-[8px]">
        {lanes.map((lane) => (
          <LaneBar key={lane.label} lane={lane} size="wide" blocked={blocked} />
        ))}
      </div>
    </div>
  );
}
