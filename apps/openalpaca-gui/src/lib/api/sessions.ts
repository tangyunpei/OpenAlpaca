/**
 * `/v1/sessions*` — the conversation surface (daemon plan §5.7).
 *
 * A lane holds many conversations since migration 039, and exactly one of them
 * is `active`: the one the lane's next turn lands in. That invariant is a
 * partial unique index in the database, not a convention, and it is why the
 * write verbs here read the way they do — `activate` **archives the incumbent**
 * rather than opening a second live conversation, and `POST /v1/sessions`
 * ("New chat") does the same.
 *
 * Two rules this client must not try to implement for itself:
 *   * **R48 — changing project starts a new session.** The daemon opens it on
 *     the turn whose `x-workspace-path` differs from the active session's
 *     binding. A client that also created one would produce two. `PATCH` with
 *     a `workspace_path` on a session that already has one is
 *     `409 SESSION_WORKSPACE_BOUND`, deliberately.
 *   * **R49 — a turn that names a session takes that session's project**,
 *     overriding the window's. Resuming is therefore a real change of scope
 *     for that turn, and the UI has to say so rather than leave the window's
 *     project indicator looking authoritative.
 */

import { apiFetch, ApiError } from "../http";
import type {
  Session,
  SessionMessagesResponse,
  SessionsResponse,
} from "./types";

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

function sessionPath(id: string): string {
  return `/v1/sessions/${encodeURIComponent(id)}`;
}

export interface CreateSessionInput {
  /** The lane's source. The daemon defaults it to `gui`; so does this. */
  source?: string;
  /**
   * The project the new conversation is bound to. Omitted entirely when the
   * window has no project — `""` is the daemon's *unbind* spelling, not
   * "none", and sending it here would be a different request.
   */
  workspacePath?: string;
  title?: string;
}

/**
 * `POST /v1/sessions` — "New chat".
 *
 * Answers `201` with the created row, and **archives the lane's previous
 * active session** in the same transaction. There is no second live
 * conversation to fall back to afterwards; that is the point of the verb.
 */
export async function createSession(
  input: CreateSessionInput = {},
): Promise<Session> {
  return await apiFetch<Session>("/v1/sessions", {
    method: "POST",
    body: {
      source: input.source ?? "gui",
      ...(input.workspacePath === undefined
        ? {}
        : { workspace_path: input.workspacePath }),
      ...(input.title === undefined ? {} : { title: input.title }),
    },
  });
}

/** `POST /v1/sessions/{id}/activate` — resume, stepping the incumbent down. */
export async function activateSession(id: string): Promise<Session> {
  return await apiFetch<Session>(`${sessionPath(id)}/activate`, {
    method: "POST",
  });
}

/** `POST /v1/sessions/{id}/archive` — close it; the lane is then session-less. */
export async function archiveSession(id: string): Promise<Session> {
  return await apiFetch<Session>(`${sessionPath(id)}/archive`, {
    method: "POST",
  });
}

export interface UpdateSessionInput {
  title?: string;
  /** A path binds an *unbound* session; `""` unbinds. Both are 409 on a
   * session that already has a project (R48). */
  workspacePath?: string;
}

/**
 * `PATCH /v1/sessions/{id}` — rename, and (only on an unbound conversation)
 * bind a project.
 *
 * Only the named fields are sent: a rename that also carried the session's
 * current `workspace_path` would be refused with `SESSION_WORKSPACE_BOUND`
 * for no reason the user could see.
 */
export async function updateSession(
  id: string,
  input: UpdateSessionInput,
): Promise<Session> {
  return await apiFetch<Session>(sessionPath(id), {
    method: "PATCH",
    body: {
      ...(input.title === undefined ? {} : { title: input.title }),
      ...(input.workspacePath === undefined
        ? {}
        : { workspace_path: input.workspacePath }),
    },
  });
}

/** `DELETE /v1/sessions/{id}` — `204`, or `409` while a run it started lives. */
export async function deleteSession(id: string): Promise<void> {
  await apiFetch<void>(sessionPath(id), { method: "DELETE" });
}

/**
 * The sentence a failed session call shows the user.
 *
 * Every code the routes answer with gets its own line. The two `409`s are
 * different facts and must never collapse into one "conflict": a delete
 * blocked by a live run is recoverable by cancelling the run, while a project
 * that cannot be re-pointed is answered by starting a new conversation. An
 * unrecognised code falls back to the daemon's own message rather than to a
 * shrug.
 */
export function sessionErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    switch (error.code) {
      case "SESSION_HAS_ACTIVE_WORKFLOWS":
        return "This conversation has a run in flight — cancel it before deleting.";
      case "SESSION_WORKSPACE_BOUND":
        return "This conversation is already bound to a project. Start a new chat to work in another one.";
      case "SESSION_ARCHIVED":
        return "This conversation was archived — open it again from the list to continue it.";
      case "SESSION_LANE_MISMATCH":
        return "That conversation belongs to another lane, so this window cannot add to it.";
      case "SESSION_NOT_FOUND":
        return "That conversation is no longer there.";
      case "INVALID_STATUS":
        return "A conversation is either active or archived — nothing else.";
      case "SESSION_EVENTS_NOT_SERVED":
        return "The per-turn event log is not served yet (Phase 7b).";
      default:
        break;
    }
    if (error.isTransport)
      return "Could not reach the daemon — nothing changed.";
    return error.message;
  }
  if (error instanceof Error) return error.message;
  return "Could not reach the conversation list.";
}
