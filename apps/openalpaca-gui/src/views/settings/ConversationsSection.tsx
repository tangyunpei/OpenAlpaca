/**
 * Settings → Conversations (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * A read-only inventory across **every** lane: `GET /v1/sessions` carries the
 * title, message count, source and last-message stamp the design shows, plus
 * the workspace a conversation is bound to and whether it is still the live
 * one. The GUI's own lane is one of them.
 *
 * The write verbs (rename, archive, delete, "New chat") live on the chat
 * view's conversation sidebar, where the conversation being operated on is the
 * one on screen — GAP-21 closed there, not here. This list stays a list on
 * purpose: a delete button beside a Telegram thread in a settings inventory is
 * a different, and worse, affordance than one beside the transcript it removes.
 *
 * One page, and it says so. The route pages (`limit`/`offset` with a `total`
 * in the envelope) and this asks for 50; a lane with more than that used to
 * end at the fiftieth row with nothing to say it had. The footer names both
 * numbers, so a missing conversation reads as "further down the list" rather
 * than as "gone".
 */

import { Tag } from "@/components/ui";
import { useSessions } from "@/hooks/useSessions";

import { ListCard, ListRow, ListState } from "./primitives";
import { shortDate } from "./format";

/** What one read of `GET /v1/sessions` asks for. */
const PAGE = 50;

export function ConversationsSection() {
  const sessions = useSessions({ limit: PAGE });
  const rows = sessions.data?.sessions ?? [];
  const total = sessions.data?.total;

  return (
    <ListCard
      footer={
        total !== undefined && total > rows.length
          ? `Showing ${rows.length} of ${total}.`
          : undefined
      }
    >
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
  );
}
