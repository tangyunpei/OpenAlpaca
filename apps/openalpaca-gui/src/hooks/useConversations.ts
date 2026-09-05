/** Stored lanes (Settings → Conversations). Rename/delete are GAP-21. */

import { useQuery, type UseQueryResult } from "@tanstack/react-query";

import {
  getConversationMessages,
  listConversations,
  type ListConversationsQuery,
} from "@/lib/api/conversations";
import type {
  ConversationMessagesResponse,
  ConversationsResponse,
} from "@/lib/api/types";
import { qk } from "@/lib/query-keys";

export function useConversations(
  query: ListConversationsQuery = {},
): UseQueryResult<ConversationsResponse> {
  return useQuery({
    queryKey: qk.conversations.list(query),
    queryFn: ({ signal }) => listConversations(query, signal),
  });
}

export function useConversationMessages(
  conversationId: string | null,
  query: { limit?: number; offset?: number } = {},
): UseQueryResult<ConversationMessagesResponse> {
  return useQuery({
    queryKey: qk.conversations.messages(conversationId ?? "", query),
    queryFn: ({ signal }) =>
      getConversationMessages(conversationId as string, query, signal),
    enabled: conversationId !== null,
  });
}
