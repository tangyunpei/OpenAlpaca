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
 */

import { Tag } from "@/components/ui";
import { useSessions } from "@/hooks/useSessions";

import { ListCard, ListRow, ListState } from "./primitives";
import { shortDate } from "./format";

export function ConversationsSection() {
  const sessions = useSessions({ limit: 50 });
  const rows = sessions.data?.sessions ?? [];

  return (
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
  );
}
