/**
 * Settings → Conversations (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Fully backed for reading: `GET /v1/conversations` carries the title, message
 * count, source and last-message stamp the design shows, and `summary_version`
 * is what its `compacted` tag really means.
 *
 * Unavailable: renaming or deleting a lane. Both conversation routes are GETs
 * (GAP-21); `DELETE /v1/chat/history` clears messages but leaves the row.
 */

import { Tag } from "@/components/ui";
import { useConversations } from "@/hooks/useConversations";
import { GAPS, gapNote } from "@/lib/unavailable";

import { GapNote, ListCard, ListRow, ListState } from "./primitives";
import { shortDate } from "./format";

const CONVERSATION_WRITE_NOTE = gapNote(GAPS["GAP-21"]);

export function ConversationsSection() {
  const conversations = useConversations({ limit: 50 });
  const rows = conversations.data?.conversations ?? [];

  return (
    <>
      <ListCard>
        <ListState
          pending={conversations.isPending}
          error={conversations.error}
          empty={rows.length === 0}
          emptyCopy="No stored conversations."
        >
          {rows.map((conversation) => (
            <ListRow
              key={conversation.id}
              name={conversation.title}
              tags={
                conversation.summary_version > 0 ? (
                  <Tag value="compacted" />
                ) : undefined
              }
              description={`${conversation.lane_key} · ${conversation.source}`}
              meta={`${conversation.message_count} messages · ${shortDate(
                conversation.last_message_at,
              )}`}
            />
          ))}
        </ListState>
      </ListCard>

      <GapNote>{CONVERSATION_WRITE_NOTE}.</GapNote>
    </>
  );
}
