/**
 * Conversations store — reactive state for cross-platform conversations.
 */

import { writable } from "svelte/store";
import { listConversations, getConversationMessages } from "../api/conversations";
import type { Conversation, ChatMessage } from "../types";

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
    const resp = await getConversationMessages(conversationId, limit ?? 100, offset ?? 0);
    selectedMessages.set(resp.messages);
    selectedMessagesTotal.set(resp.total);
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
