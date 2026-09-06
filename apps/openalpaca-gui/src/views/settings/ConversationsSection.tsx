/**
 * Settings → Conversations (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Fully backed for reading: `GET /v1/sessions` carries the title, message
 * count, source and last-message stamp the design shows, plus the workspace a
 * conversation is bound to and whether it is still the live one.
 *
 * Unavailable *here*: the rename and delete controls. The daemon serves both
 * (`PATCH`/`DELETE /v1/sessions/{id}`); this list has no UI for them yet, and
 * the session sidebar that will is its own task (GAP-21).
 */

import { Tag } from "@/components/ui";
import { useSessions } from "@/hooks/useSessions";
import { GAPS, gapNote } from "@/lib/unavailable";

import { GapNote, ListCard, ListRow, ListState } from "./primitives";
import { shortDate } from "./format";

const CONVERSATION_WRITE_NOTE = gapNote(GAPS["GAP-21"]);

export function ConversationsSection() {
  const sessions = useSessions({ limit: 50 });
  const rows = sessions.data?.sessions ?? [];

  return (
    <>
      <ListCard>
        <ListState
          pending={sessions.isPending}
          error={sessions.error}
          empty={rows.length === 0}
          emptyCopy="No stored conversations."
        >
          {rows.map((session) => (
            <ListRow
              key={session.id}
              name={session.title || "Untitled conversation"}
              tags={
                session.status === "archived" ? (
                  <Tag value="archived" />
                ) : undefined
              }
              description={`${session.lane_key} · ${session.source}`}
              meta={`${session.message_count} messages · ${shortDate(
                session.last_message_at,
              )}`}
            />
          ))}
        </ListState>
      </ListCard>

      <GapNote>{CONVERSATION_WRITE_NOTE}.</GapNote>
    </>
  );
}
