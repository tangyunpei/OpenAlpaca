/**
 * Conversations store — reactive state for cross-platform conversations.
 */

import { writable } from "svelte/store";
import { listConversations, getConversationMessages } from "../api/conversations";
import type { Conversation, ChatMessage } from "../types";

type StructuredMessagePart = {
  type: string;
  file_id?: string;
  filename?: string;
  mime_type?: string;
};

function normalizeHistoryMessage(message: ChatMessage): ChatMessage {
  const normalized: ChatMessage = { ...message };
  if (
    normalized.content.trim().length === 0 &&
    normalized.display_text &&
    normalized.display_text.trim().length > 0
  ) {
    normalized.content = normalized.display_text;
  }
  if (
    (!normalized.attachments || normalized.attachments.length === 0) &&
    normalized.content_json
  ) {
    try {
      const parsed = JSON.parse(normalized.content_json) as { parts?: StructuredMessagePart[] };
      const parts = Array.isArray(parsed.parts) ? parsed.parts : [];
      const seen = new Set<string>();
      const attachments = [];
      for (const part of parts) {
        if (
          (part.type === "file_ref" || part.type === "document") &&
          part.file_id &&
          part.filename &&
          part.mime_type &&
          !seen.has(part.file_id)
        ) {
          seen.add(part.file_id);
          attachments.push({
            file_id: part.file_id,
            filename: part.filename,
            mime_type: part.mime_type,
            size_bytes: 0,
          });
        }
      }
      if (attachments.length > 0) {
        normalized.attachments = attachments;
      }
    } catch {
      // ignore malformed content_json
    }
  }
  if (
    normalized.content.trim().length === 0 &&
    normalized.attachments &&
    normalized.attachments.length > 0
  ) {
    normalized.content = "[Attachment]";
  }
  return normalized;
}

/** All conversations */
export const conversations = writable<Conversation[]>([]);

/** Loading state */
export const conversationsLoading = writable(false);

/** Error state */
export const conversationsError = writable<string | null>(null);

/** Currently selected conversation ID */
export const selectedConversationId = writable<string | null>(null);

/** Messages for the selected conversation */
export const selectedMessages = writable<ChatMessage[]>([]);

/** Total message count for the selected conversation */
export const selectedMessagesTotal = writable(0);

/** Loading state for messages */
export const messagesLoading = writable(false);

/** Source filter */
export const sourceFilter = writable<string | undefined>(undefined);

/** Load conversations from the API */
export async function loadConversations(source?: string): Promise<void> {
  conversationsLoading.set(true);
  conversationsError.set(null);
  try {
    const resp = await listConversations(source, 50, 0);
    conversations.set(resp.conversations);
  } catch (e) {
    console.error("[conversations-store] Failed to load:", e);
    conversationsError.set(e instanceof Error ? e.message : String(e));
  } finally {
    conversationsLoading.set(false);
  }
}

/** Load messages for a specific conversation */
export async function loadConversationMessages(
  conversationId: string,
  limit?: number,
  offset?: number,
): Promise<void> {
  messagesLoading.set(true);
  selectedConversationId.set(conversationId);
  try {
    // If caller specifies explicit paging, preserve one-page behavior.
    if (limit !== undefined || offset !== undefined) {
      const resp = await getConversationMessages(conversationId, limit ?? 100, offset ?? 0);
      selectedMessages.set(resp.messages.map(normalizeHistoryMessage));
      selectedMessagesTotal.set(resp.total);
      return;
    }

    const pageSize = 100;
    let nextOffset = 0;
    let total = 0;
    const allMessages: ChatMessage[] = [];

    while (true) {
      const resp = await getConversationMessages(conversationId, pageSize, nextOffset);
      total = resp.total;
      if (resp.messages.length === 0) break;
      allMessages.push(...resp.messages);
      nextOffset += resp.messages.length;
      if (allMessages.length >= total) break;
    }

    selectedMessages.set(allMessages.map(normalizeHistoryMessage));
    selectedMessagesTotal.set(total);
  } catch (e) {
    console.error("[conversations-store] Failed to load messages:", e);
  } finally {
    messagesLoading.set(false);
  }
}

/** Clear the selected conversation */
export function clearSelection(): void {
  selectedConversationId.set(null);
  selectedMessages.set([]);
  selectedMessagesTotal.set(0);
}
