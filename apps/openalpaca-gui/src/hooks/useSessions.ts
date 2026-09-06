/**
 * Conversations, served by `/v1/sessions` (plan §5.7).
 *
 * Reads back the chat view's sidebar and Settings → Conversations; writes are
 * the five verbs §5.7 defines. Every write invalidates both `sessions` and
 * `chat`: activating or archiving moves which conversation the lane's next
 * turn lands in, so an open transcript is stale the moment one lands. The
 * daemon says the same thing over the WebSocket (`session_changed`, wired in
 * `query-client.ts`) — this is the half that works before the socket is up.
 */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import {
  activateSession,
  archiveSession,
  createSession,
  deleteSession,
  getSessionMessages,
  listSessions,
  updateSession,
  type CreateSessionInput,
  type ListSessionsQuery,
  type UpdateSessionInput,
} from "@/lib/api/sessions";
import type {
  Session,
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

/** Everything a session write changes: the list, and which transcript is live. */
function useSessionInvalidation(): () => void {
  const client = useQueryClient();
  return () => {
    void client.invalidateQueries({ queryKey: qk.sessions.all() });
    void client.invalidateQueries({ queryKey: qk.chat.all() });
  };
}

/** `POST /v1/sessions` — "New chat". Archives the lane's previous active. */
export function useCreateSession(): UseMutationResult<
  Session,
  Error,
  CreateSessionInput
> {
  const invalidate = useSessionInvalidation();
  return useMutation({
    mutationFn: (input: CreateSessionInput) => createSession(input),
    onSettled: invalidate,
  });
}

/** `POST /v1/sessions/{id}/activate` — resume one, stepping the incumbent down. */
export function useActivateSession(): UseMutationResult<
  Session,
  Error,
  string
> {
  const invalidate = useSessionInvalidation();
  return useMutation({
    mutationFn: (id: string) => activateSession(id),
    onSettled: invalidate,
  });
}

/** `POST /v1/sessions/{id}/archive` — close it. */
export function useArchiveSession(): UseMutationResult<Session, Error, string> {
  const invalidate = useSessionInvalidation();
  return useMutation({
    mutationFn: (id: string) => archiveSession(id),
    onSettled: invalidate,
  });
}

export interface RenameSessionInput extends UpdateSessionInput {
  id: string;
}

/** `PATCH /v1/sessions/{id}` — rename (and, unbound only, bind a project). */
export function useUpdateSession(): UseMutationResult<
  Session,
  Error,
  RenameSessionInput
> {
  const invalidate = useSessionInvalidation();
  return useMutation({
    mutationFn: ({ id, ...patch }: RenameSessionInput) =>
      updateSession(id, patch),
    onSettled: invalidate,
  });
}

/** `DELETE /v1/sessions/{id}` — `409` while a run this session started lives. */
export function useDeleteSession(): UseMutationResult<void, Error, string> {
  const invalidate = useSessionInvalidation();
  return useMutation({
    mutationFn: (id: string) => deleteSession(id),
    onSettled: invalidate,
  });
}
