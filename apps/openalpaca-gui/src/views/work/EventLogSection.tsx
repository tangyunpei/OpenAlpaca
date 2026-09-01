/**
 * The work detail's `Event log` card (DESIGN_SPEC §5.2, §3.27, §3.28).
 *
 * There is no per-run history: `event_log` has no `task_id` column and
 * `GET /v1/events/history` filters by `agent_id` only (GAP-10). What the card
 * can show honestly is the live socket, filtered to this run — five
 * `ServerEvent` variants carry a `task_id`. That is this session only, and the
 * note says so; nothing is back-filled and no event is attributed by guesswork.
 */

import { LogTag, SectionCard, SectionEmpty } from "@/components/ui";
import { formatClock } from "@/components/work/run-model";
import type { RunEvent } from "@/lib/api/unbacked";
import { GAPS } from "@/lib/unavailable";

/** The design's own empty sentence for this card. */
export const EVENT_LOG_EMPTY = "No events for this run yet.";

const EVENT_LOG_NOTE = `Live events from this session only — ${GAPS["GAP-10"].missingApi}. Proposed: ${GAPS["GAP-10"].proposedEndpoint}`;

export interface EventLogSectionProps {
  events: readonly RunEvent[];
}

export function EventLogSection({ events }: EventLogSectionProps) {
  return (
    <SectionCard title="Event log">
      {events.length === 0 ? (
        <SectionEmpty note={EVENT_LOG_NOTE}>{EVENT_LOG_EMPTY}</SectionEmpty>
      ) : (
        <>
          <ul className="m-0 flex list-none flex-col py-[6px] pl-0">
            {events.map((event) => (
              <li
                key={event.id}
                className="flex items-center gap-[10px] px-[16px] py-[7px]"
              >
                <LogTag value={event.tag} />
                <span className="min-w-0 flex-1 truncate text-base text-secondary">
                  {event.text}
                </span>
                <span className="shrink-0 font-mono text-2xs-plus text-faint">
                  {formatClock(event.at, true) ?? ""}
                </span>
              </li>
            ))}
          </ul>
          <p className="m-0 px-[16px] pb-[12px] font-mono text-2xs-plus leading-[1.5] text-faint">
            {EVENT_LOG_NOTE}
          </p>
        </>
      )}
    </SectionCard>
  );
}
