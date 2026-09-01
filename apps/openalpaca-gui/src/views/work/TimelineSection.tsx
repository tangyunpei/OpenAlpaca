/**
 * The work detail's `Timeline` card (DESIGN_SPEC §5.2, §3.27 padded variant).
 *
 * The swimlanes need per-subagent spans, which nothing serves: `agent_task_history`
 * stores an agent, a role, a status and a runtime — but no `started_at` — so a
 * span cannot be placed on an axis at all (GAP-09).
 *
 * Rather than draw an empty box, the card falls back to what the same table
 * *does* answer: which agents ran, in what role, for how long. That is real
 * data, it is clearly not a timeline, and the note says exactly which field is
 * missing and which route would supply it.
 */

import { TimelineLanes } from "@/components/work/ParallelWork";
import { SectionCard, SectionEmpty } from "@/components/ui";
import type { TaskAgentAssignment, TaskAssignedAgent } from "@/lib/api/types";
import type { TaskTimeline } from "@/lib/api/unbacked";
import { GAPS, isAvailable, type Availability } from "@/lib/unavailable";

/** The design's own empty sentence for this card. */
export const TIMELINE_EMPTY =
  "No steps have run yet. The timeline fills in once an agent slot frees up.";

const TIMELINE_NOTE = `${GAPS["GAP-09"].label} not yet available — ${GAPS["GAP-09"].missingApi}. Proposed: ${GAPS["GAP-09"].proposedEndpoint}`;

/** Both list shapes the daemon serves for the same underlying rows. */
export type RunAssignment = TaskAgentAssignment | TaskAssignedAgent;

export interface TimelineSectionProps {
  timeline: Availability<TaskTimeline>;
  /** `assignments` from `GET /v1/tasks/{id}`, or the list route's summaries. */
  assignments: readonly RunAssignment[];
  /** This run holds the pending tool confirmation (§4.4's coupling). */
  blocked?: boolean;
}

export function TimelineSection({
  timeline,
  assignments,
  blocked = false,
}: TimelineSectionProps) {
  if (isAvailable(timeline)) {
    return (
      <SectionCard title="Timeline" variant="padded">
        {timeline.data.lanes.length === 0 ? (
          <SectionEmpty padded={false}>{TIMELINE_EMPTY}</SectionEmpty>
        ) : (
          <TimelineLanes timeline={timeline.data} blocked={blocked} />
        )}
      </SectionCard>
    );
  }

  return (
    <SectionCard title="Timeline" variant="padded">
      {assignments.length === 0 ? (
        <SectionEmpty padded={false} note={TIMELINE_NOTE}>
          {TIMELINE_EMPTY}
        </SectionEmpty>
      ) : (
        <div>
          <ul className="m-0 flex list-none flex-col gap-[6px] p-0">
            {assignments.map((assignment) => (
              <li
                key={`${assignment.agent_id}-${assignment.role}-${assignment.completed_at ?? ""}`}
                className="flex items-center gap-[11px]"
              >
                <span className="w-[96px] shrink-0 truncate text-right font-mono text-xs-plus text-secondary">
                  {assignment.role}
                </span>
                <span className="min-w-0 flex-1 truncate text-base text-body">
                  {assignment.agent_id}
                </span>
                <span className="w-[88px] shrink-0 text-right font-mono text-2xs-plus text-muted-fg">
                  {assignment.runtime_seconds === null
                    ? assignment.status
                    : `${assignment.runtime_seconds}s · ${assignment.status}`}
                </span>
              </li>
            ))}
          </ul>
          <p className="mt-[10px] mb-0 font-mono text-2xs-plus leading-[1.5] text-faint">
            {`Agent runs, not a timeline — ${TIMELINE_NOTE}`}
          </p>
        </div>
      )}
    </SectionCard>
  );
}
