/**
 * Settings → Event log (DESIGN_SPEC §5.4, §3.32 "Log row").
 *
 * `GET /v1/events/history` is real; the rows below are its rows, newest first.
 *
 * The design's five `LogTag` tones are a *categorisation* of the daemon's
 * `event_type`, not a field it serves — `tagForEvent` maps the real string onto
 * the palette and leaves the untouched `event_type` in the message column, so
 * nothing is invented and nothing is hidden.
 *
 * The persisted log is thinner than the live socket, which carries far more
 * event variants and cannot be replayed (GAP-10).
 */

import { useEventHistory } from "@/hooks/useEventHistory";
import { GAPS, gapNote } from "@/lib/unavailable";

import { GapNote, ListCard, ListState, LogRow } from "./primitives";
import { timeOfDay } from "./format";

const EVENT_SCOPE_NOTE = gapNote(GAPS["GAP-10"]);

/** The design's five tones (§3.28), keyed off the daemon's `event_type`. */
export function tagForEvent(eventType: string): string {
  const type = eventType.toLowerCase();
  if (type.includes("tool")) return "tool";
  if (type.includes("steer")) return "steer";
  if (type.includes("artifact") || type.includes("file")) return "artifact";
  if (type.includes("spawn") || type.includes("agent")) return "spawn";
  return "run";
}

export function EventLogSection() {
  const events = useEventHistory({ limit: 50 });
  const rows = [...(events.data ?? [])].sort((a, b) =>
    b.timestamp.localeCompare(a.timestamp),
  );

  return (
    <>
      <ListCard>
        <ListState
          pending={events.isPending}
          error={events.error}
          empty={rows.length === 0}
          emptyCopy="The daemon has not logged anything yet."
        >
          {rows.map((event) => (
            <LogRow
              key={event.id}
              tag={tagForEvent(event.event_type)}
              text={
                event.agent_id === null
                  ? event.event_type
                  : `${event.event_type} · ${event.agent_id}`
              }
              at={timeOfDay(event.timestamp)}
            />
          ))}
        </ListState>
      </ListCard>

      <GapNote>{EVENT_SCOPE_NOTE}.</GapNote>
    </>
  );
}
