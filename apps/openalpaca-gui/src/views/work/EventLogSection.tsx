/**
 * The work detail's `Event log` card (DESIGN_SPEC §5.2, §3.27, §3.28).
 *
 * `GET /v1/events/history?task_id=` serves this run's own log (GAP-10):
 * persisted, restart-surviving, and including the tool calls the live socket
 * could never attribute to a run. Before that route existed this card showed
 * the socket ring filtered to the frames that happened to carry a `task_id` —
 * this session only, tool rows missing.
 *
 * A run with nothing logged gets the design's own empty sentence. A failed
 * read gets a different one: an empty list alone cannot tell "nothing has
 * happened" from "the read failed", and asserting the former for a 500 would
 * say something about the run that may be untrue — so the caller threads the
 * query's `error` through, as `TimelineSection` above it does.
 */

import { LogTag, SectionCard, SectionEmpty } from "@/components/ui";
import { formatClock } from "@/components/work/run-model";
import type { RunEvent } from "@/lib/api/run-events";

/** The design's own empty sentence for this card. */
export const EVENT_LOG_EMPTY = "No events for this run yet.";

/** Shown instead of `EVENT_LOG_EMPTY` when the read itself failed. */
export const EVENT_LOG_ERROR = "This run's event log could not be loaded.";

export interface EventLogSectionProps {
  events: readonly RunEvent[];
  /** Why the read failed — a failed request, never a gap. */
  error?: string | null;
}

export function EventLogSection({
  events,
  error = null,
}: EventLogSectionProps) {
  return (
    <SectionCard title="Event log">
      {events.length === 0 ? (
        <SectionEmpty note={error ?? undefined}>
          {error !== null ? EVENT_LOG_ERROR : EVENT_LOG_EMPTY}
        </SectionEmpty>
      ) : (
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
      )}
    </SectionCard>
  );
}
