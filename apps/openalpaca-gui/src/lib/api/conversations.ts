/** `/v1/conversations*` — envelope-style responses. */

import { apiFetch } from "../http";
import type {
  ConversationMessagesResponse,
  ConversationsResponse,
} from "./types";

export interface ListConversationsQuery {
  source?: string;
  limit?: number;
  offset?: number;
}

/** `GET /v1/conversations` */
export async function listConversations(
  query: ListConversationsQuery = {},
  signal?: AbortSignal,
): Promise<ConversationsResponse> {
  return await apiFetch<ConversationsResponse>("/v1/conversations", {
    query: { source: query.source, limit: query.limit, offset: query.offset },
    signal,
  });
}

/** `GET /v1/conversations/{id}/messages` */
export async function getConversationMessages(
  conversationId: string,
  query: { limit?: number; offset?: number } = {},
  signal?: AbortSignal,
): Promise<ConversationMessagesResponse> {
  return await apiFetch<ConversationMessagesResponse>(
    `/v1/conversations/${encodeURIComponent(conversationId)}/messages`,
    { query: { limit: query.limit, offset: query.offset }, signal },
  );
}
