/**
 * The work detail's `Timeline` card (DESIGN_SPEC §5.2, §3.27 padded variant).
 *
 * `GET /v1/tasks/{id}/timeline` serves the swimlanes (Phase 4): one
 * `subagent_span` per lane, opened at the spawn, so a lane that is still
 * working is drawn with a start and no end. Before that route existed this
 * card fell back to listing agent runs out of `agent_task_history` — real data
 * but explicitly not a timeline, because that table stores no start time and
 * writes no row until a run finishes.
 *
 * A run with no lanes — nothing spawned yet, or a run that predates the
 * `subagent_span` table — gets the design's own empty sentence rather than an
 * empty axis. A failed read gets a different sentence entirely: `timeline:
 * null` alone cannot tell "nothing has run" from "the read failed", and
 * stating the former for a 500 or a dropped connection would assert
 * something about the run that may be untrue — the caller threads the
 * query's `error` through so this card can tell them apart.
 */

import { TimelineLanes } from "@/components/work/ParallelWork";
import { SectionCard, SectionEmpty } from "@/components/ui";
import type { TaskTimeline } from "@/lib/api/tasks";

/** The design's own empty sentence for this card. */
export const TIMELINE_EMPTY =
  "No steps have run yet. The timeline fills in once an agent slot frees up.";

/** Shown instead of `TIMELINE_EMPTY` when the read itself failed — a `null`
 * timeline is not evidence the run spawned nothing. */
export const TIMELINE_ERROR = "This run's timeline could not be loaded.";

export interface TimelineSectionProps {
  /** `null` while the timeline is loading, or if the read failed. */
  timeline: TaskTimeline | null;
  /** Why the read failed — a failed request, never a gap. */
  error?: string | null;
  /** This run holds the pending tool confirmation (§4.4's coupling). */
  blocked?: boolean;
}

export function TimelineSection({
  timeline,
  error = null,
  blocked = false,
}: TimelineSectionProps) {
  return (
    <SectionCard title="Timeline" variant="padded">
      {timeline === null || timeline.lanes.length === 0 ? (
        <SectionEmpty padded={false} note={error ?? undefined}>
          {error !== null ? TIMELINE_ERROR : TIMELINE_EMPTY}
        </SectionEmpty>
      ) : (
        <TimelineLanes timeline={timeline} blocked={blocked} />
      )}
    </SectionCard>
  );
}
