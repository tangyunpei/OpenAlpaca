/** `/v1/sessions*` — envelope-style responses (daemon plan §5.7). */

import { apiFetch } from "../http";
import type { SessionMessagesResponse, SessionsResponse } from "./types";

export interface ListSessionsQuery {
  workspace_id?: string;
  source?: string;
  status?: "active" | "archived";
  q?: string;
  limit?: number;
  offset?: number;
}

/** `GET /v1/sessions` */
export async function listSessions(
  query: ListSessionsQuery = {},
  signal?: AbortSignal,
): Promise<SessionsResponse> {
  return await apiFetch<SessionsResponse>("/v1/sessions", {
    query: {
      workspace_id: query.workspace_id,
      source: query.source,
      status: query.status,
      q: query.q,
      limit: query.limit,
      offset: query.offset,
    },
    signal,
  });
}

/** `GET /v1/sessions/{id}/messages` */
export async function getSessionMessages(
  sessionId: string,
  query: { limit?: number; offset?: number; before_id?: number } = {},
  signal?: AbortSignal,
): Promise<SessionMessagesResponse> {
  return await apiFetch<SessionMessagesResponse>(
    `/v1/sessions/${encodeURIComponent(sessionId)}/messages`,
    {
      query: {
        limit: query.limit,
        offset: query.offset,
        before_id: query.before_id,
      },
      signal,
    },
  );
}
