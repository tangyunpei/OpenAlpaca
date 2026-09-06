/** Stored conversations (Settings → Conversations), served by `/v1/sessions`. */

import { useQuery, type UseQueryResult } from "@tanstack/react-query";

import {
  getSessionMessages,
  listSessions,
  type ListSessionsQuery,
} from "@/lib/api/sessions";
import type {
  SessionMessagesResponse,
  SessionsResponse,
} from "@/lib/api/types";
import { qk } from "@/lib/query-keys";

export function useSessions(
  query: ListSessionsQuery = {},
): UseQueryResult<SessionsResponse> {
  return useQuery({
    queryKey: qk.sessions.list(query),
    queryFn: ({ signal }) => listSessions(query, signal),
  });
}

export function useSessionMessages(
  sessionId: string | null,
  query: { limit?: number; offset?: number; before_id?: number } = {},
): UseQueryResult<SessionMessagesResponse> {
  return useQuery({
    queryKey: qk.sessions.messages(sessionId ?? "", query),
    queryFn: ({ signal }) =>
      getSessionMessages(sessionId as string, query, signal),
    enabled: sessionId !== null,
  });
}
