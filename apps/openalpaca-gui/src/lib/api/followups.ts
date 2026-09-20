/**
 * `/v1/lanes/{lane_key}/followups*` — the lane follow-up queue (GAP-03, closed).
 *
 * A follow-up is work the user parks for *after* the run that is currently
 * going: the daemon claims the oldest queued item when a workflow finalizes and
 * re-enters it as a fresh turn. Until Phase 5 the only writer was the model's
 * own `queue_followup` tool, so the design's `Queue follow-up` control had
 * nothing to call and nothing to show.
 *
 * Two shape notes that are contract, not style:
 *   * the row carries **no principal** — the daemon stores the identity a
 *     queued item re-enters with, and never serializes it;
 *   * `kind` is a two-value union but only one of them is writable. An
 *     `unprocessed_steering` row is a steering message the workflow exited
 *     before draining; the daemon mints those itself and never auto-claims
 *     them, so `queueFollowup` cannot ask for one.
 *
 * Cancel is a compare-and-swap against the daemon's own autostart, not a
 * delete: it wins only while the item is still queued. Render its refusal
 * through {@link followupErrorMessage} — `FOLLOWUP_NOT_QUEUED` means the turn
 * is already running, which is a different fact from "gone".
 */

import { apiFetch, ApiError } from "../http";
import { workspaceHeader } from "../workspace-header";

export type FollowupKind = "followup" | "unprocessed_steering";
export type FollowupStatus = "queued" | "running" | "done" | "cancelled";

/** One row of the queue, exactly as the daemon's `FollowupView` serializes it. */
export interface FollowupRecord {
  id: number;
  lane_key: string;
  kind: FollowupKind;
  content: string;
  /** The run it was queued from, when there was one. */
  source_task_id: string | null;
  status: FollowupStatus;
  created_at: string;
  updated_at: string;
}

/** What `DELETE` answers with — the row it cancelled, by id. */
export interface FollowupCancelled {
  id: number;
  status: "cancelled";
}

function lanePath(laneKey: string): string {
  return `/v1/lanes/${encodeURIComponent(laneKey)}/followups`;
}

/**
 * `GET /v1/lanes/{lane_key}/followups` — the lane's **pending** queue, oldest
 * first, which is the order the daemon will claim them in.
 *
 * Queued rows only: a finished or cancelled item is history, and the daemon
 * deliberately does not grow this list for the life of the lane. Both kinds
 * appear — a steering leftover is pending too, even though it will be shown on
 * the lane's next turn rather than auto-run.
 */
export async function listFollowups(
  laneKey: string,
  signal?: AbortSignal,
): Promise<FollowupRecord[]> {
  return await apiFetch<FollowupRecord[]>(lanePath(laneKey), { signal });
}

/**
 * `POST /v1/lanes/{lane_key}/followups` — park one item on the lane.
 *
 * The daemon fills in the principal (this user) and the project (from
 * `x-workspace-path`), so the re-entered turn runs as the person who queued it,
 * scoped where they queued it. `sourceTaskId` records which run the follow-up
 * came out of.
 *
 * Throws `ApiError`: `EMPTY_CONTENT` / `INVALID_KIND` / `INVALID_LANE_KEY`
 * (400).
 */
export async function queueFollowup(
  laneKey: string,
  content: string,
  sourceTaskId?: string,
  workspacePath?: string,
): Promise<FollowupRecord> {
  return await apiFetch<FollowupRecord>(lanePath(laneKey), {
    method: "POST",
    body: {
      content,
      ...(sourceTaskId === undefined ? {} : { source_task_id: sourceTaskId }),
    },
    ...(workspacePath === undefined
      ? {}
      : { headers: workspaceHeader(workspacePath) }),
  });
}

/**
 * `DELETE /v1/lanes/{lane_key}/followups/{id}` — take one back out.
 *
 * Throws `ApiError`: `NOT_FOUND` (404 — no such item in this lane) or
 * `FOLLOWUP_NOT_QUEUED` (409 — the daemon claimed it first and the turn is
 * running).
 */
export async function cancelFollowup(
  laneKey: string,
  followupId: number,
): Promise<FollowupCancelled> {
  return await apiFetch<FollowupCancelled>(
    `${lanePath(laneKey)}/${followupId}`,
    { method: "DELETE" },
  );
}

/**
 * The sentence a failed follow-up call shows the user.
 *
 * Every code the two write routes can answer with gets its own line: a queue
 * that did not happen must never read like one that did, and "Request failed
 * with status 409" tells nobody what to do next. An unrecognised code falls
 * back to the daemon's own message rather than to a shrug.
 */
export function followupErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    switch (error.code) {
      case "FOLLOWUP_NOT_QUEUED":
        return "That follow-up already started — it is running now, so it was not cancelled.";
      case "NOT_FOUND":
        return "That follow-up is no longer on this lane.";
      case "EMPTY_CONTENT":
        return "A follow-up cannot be empty.";
      case "INVALID_KIND":
        return "That follow-up kind is the daemon's own — it cannot be queued from here.";
      case "INVALID_LANE_KEY":
        return "This conversation has no lane yet — send a message first, then queue a follow-up.";
      default:
        break;
    }
    if (error.isTransport)
      return "Could not reach the daemon — nothing was queued.";
    return error.message;
  }
  if (error instanceof Error) return error.message;
  return "Could not reach the follow-up queue.";
}
