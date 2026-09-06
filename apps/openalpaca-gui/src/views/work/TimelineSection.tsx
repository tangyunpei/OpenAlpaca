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
 * empty axis.
 */

import { TimelineLanes } from "@/components/work/ParallelWork";
import { SectionCard, SectionEmpty } from "@/components/ui";
import type { TaskTimeline } from "@/lib/api/tasks";

/** The design's own empty sentence for this card. */
export const TIMELINE_EMPTY =
  "No steps have run yet. The timeline fills in once an agent slot frees up.";

export interface TimelineSectionProps {
  /** `null` while the timeline is loading, or if the read failed. */
  timeline: TaskTimeline | null;
  /** This run holds the pending tool confirmation (§4.4's coupling). */
  blocked?: boolean;
}

export function TimelineSection({
  timeline,
  blocked = false,
}: TimelineSectionProps) {
  return (
    <SectionCard title="Timeline" variant="padded">
      {timeline === null || timeline.lanes.length === 0 ? (
        <SectionEmpty padded={false}>{TIMELINE_EMPTY}</SectionEmpty>
      ) : (
        <TimelineLanes timeline={timeline} blocked={blocked} />
      )}
    </SectionCard>
  );
}
