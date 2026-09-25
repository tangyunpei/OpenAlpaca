/**
 * `/v1/memories*` — the user-directed memory API (memory redesign, design §7,
 * §7a; owner decisions G2 and G30).
 *
 * Every call here is a **user-directed surface** (design §4): what this client
 * writes is recorded with origin `user`, the only surface that can, and the
 * view is where a candidate is confirmed — the model's own `memory_update`
 * refuses `confirm` and names this view.
 *
 * Three rules shape the module:
 *
 *  * **The project is the header.** `x-workspace-path` selects the project
 *    partition exactly as it does on `POST /v1/chat` (the daemon walks up to
 *    the nearest `.openalpaca`/`.git`); no header means the global partition
 *    only. Every function takes the window's project path and sends it, so a
 *    note opened from either scope reads and writes the same way.
 *  * **A revision travels in `If-Match` and nowhere else.** A single-note
 *    mutation sends `If-Match: "<revision>"`; a body `expected_revision` is a
 *    `400 MEMORY_INVALID_INPUT` from the daemon. A stale one is
 *    `409 MEMORY_REVISION_CONFLICT` — `isMemoryConflict` — and the view, not
 *    the daemon, says what that means to the person holding a draft.
 *  * **The shapes are the daemon's own** (`memory/service/wire.rs`): the list
 *    and search results carry `effective_status` beside the stored `status`,
 *    because *superseded* is derived from links and never written to a file.
 *
 * Learning v2 (design 09 §3.5) adds the **suggestions**: a learned change to
 * a note the owner saved or edited, which replaces nothing until the owner
 * confirms it. Confirming one sends the replaced note's revision as
 * `target_revision` beside the suggestion's own `If-Match`; *Keep mine* is
 * the undo route, a withdrawal that forgets nothing else the owner said.
 */

import { ApiError, apiFetch } from "../http";
import { workspaceHeader } from "../workspace-header";

export type NoteKind =
  "fact" | "preference" | "decision" | "procedure" | "reference" | "episode";

/** The design's six kinds, in §4's order — the create form's options. */
export const NOTE_KINDS: readonly NoteKind[] = [
  "fact",
  "preference",
  "decision",
  "procedure",
  "reference",
  "episode",
];

export type NoteScope = "global" | "project";

/** What a file can say. Superseded is never stored. */
export type NoteStatus = "active" | "candidate" | "archived";

/** What the daemon derives: the stored status, or `superseded` by a link. */
export type EffectiveStatus = NoteStatus | "superseded";

/**
 * Who wrote the note, in authority order (design §4): `user` >
 * `extracted_user` > `imported` > `external` > `extracted_other`. Typed as the
 * five words plus `string`, because a daemon ahead of this build may add one
 * and the view prints an unknown word as the daemon spelled it.
 */
export type NoteOrigin =
  | "user"
  | "extracted_user"
  | "extracted_other"
  | "imported"
  | "external"
  | (string & {});

export type Freshness = "verified" | "stale" | "unverified";

/** One `memory_list` item — the list, `recently-learned` and the view read it. */
export interface MemoryListItem {
  id: string;
  revision: string;
  title: string;
  kind: NoteKind;
  scope: NoteScope;
  status: NoteStatus;
  effective_status: EffectiveStatus;
  origin: NoteOrigin;
  updated_at: string;
  created_at: string;
  pinned: boolean;
  /**
   * The notes this one's frontmatter names in `supersedes` (design 09 §3.2
   * S8): what a learned replacement replaced, or what a suggestion would.
   */
  supersedes: string[];
}

export interface MemoryListPage {
  items: MemoryListItem[];
  /** Opaque, valid for the same filter set only; `null` on the last page. */
  next_cursor: string | null;
}

/** A DB handle the note cites — a message row or a task — never a path. */
export interface MemorySource {
  type: "message" | "task" | (string & {});
  id: string;
  observed_at: string | null;
}

/** `intact`, or a cited source was forgotten after this note was written. */
export type SourcesStatus = "intact" | "source_forgotten";

export interface MemorySearchResult {
  id: string;
  revision: string;
  title: string;
  kind: NoteKind;
  scope: NoteScope;
  status: NoteStatus;
  effective_status: EffectiveStatus;
  origin: NoteOrigin;
  excerpt: string;
  heading: string | null;
  line_range: [number, number];
  match: string[];
  freshness: Freshness;
  conflict: boolean;
  same_key_loser: boolean;
  sources: MemorySource[];
  sources_status: SourcesStatus;
  superseded_by: string[];
  /** Partition-relative: `notes/<uuid>--<slug>.md`. */
  path: string;
  pinned: boolean;
}

export interface MemorySearchPage {
  results: MemorySearchResult[];
  count: number;
  /**
   * A scan cap was hit, so there may be more. Not a cue to search again
   * (design §7c Q10): the next query would stop at the same cap.
   */
  partial: boolean;
  index_state: "ready" | "rebuilding";
}

/** `memory_read`'s result — `GET /v1/memories/{id}`. */
export interface MemoryNote {
  id: string;
  revision: string;
  title: string;
  kind: NoteKind;
  scope: NoteScope;
  status: NoteStatus;
  effective_status: EffectiveStatus;
  origin: NoteOrigin;
  frontmatter: Record<string, unknown>;
  /** The Markdown body, at most 32 KiB — `truncated` says when it was cut. */
  body: string;
  truncated: boolean;
  path: string;
  sources: MemorySource[];
  sources_status: SourcesStatus;
  supersedes: string[];
  superseded_by: string[];
  freshness: Freshness;
  conflict: boolean;
}

/** One kept revision (the last five, the current one included), newest first. */
export interface MemoryRevision {
  revision: string;
  written_at: string | null;
}

export interface MemoryStoreResult {
  id: string;
  revision: string;
  scope: NoteScope;
  origin: NoteOrigin;
  superseded: string[];
  /** `existing` when the same note was already there (exact dedup). */
  dedup: "created" | "existing";
}

export interface MemoryUpdateResult {
  id: string;
  revision: string;
  status: NoteStatus;
  /** A confirmed suggestion: the note it now replaces. Empty otherwise. */
  superseded: string[];
  /** A confirmed suggestion: the other suggestions for that note, dismissed. */
  dismissed: string[];
}

/** What a forget did not erase (design §6, *Forget contract*). */
export interface MemoryRetained {
  transcript: boolean;
  session_logs: boolean;
  note: string;
}

/** `DELETE …/{id}` answers one forgotten note and what it did not erase. */
export interface MemoryForgotten {
  id: string;
  status: "forgotten";
  retained: MemoryRetained;
}

/**
 * `…/undo` answers a withdrawal (design 09 §3.3): the note is gone, nothing
 * it cited is suppressed, and the notes it had replaced are back.
 */
export interface MemoryWithdrawn {
  id: string;
  status: "withdrawn";
  /** The notes the withdrawn one had replaced, effective again. */
  restored: string[];
}

/** The note a suggestion would replace, as the daemon re-read it. */
export interface MemorySuggestionTarget {
  id: string;
  /** Sent back as `target_revision` when the suggestion is confirmed. */
  revision: string;
  title: string;
  /** Its knowledge, cut short — shown beside the suggested text. */
  excerpt: string;
  origin: NoteOrigin;
  scope: NoteScope;
}

/** One pending suggestion — `GET /v1/memories/suggestions` (design 09 §3.5). */
export interface MemorySuggestion {
  id: string;
  revision: string;
  title: string;
  /** The suggested text. */
  knowledge: string;
  /** Why it replaces the target, when the note says. */
  reason: string | null;
  created_at: string;
  /** When what it cites was said: the newest cited row's time. */
  observed_at: string | null;
  /** `extracted_user` — from what the owner said — or `extracted_other`. */
  origin: NoteOrigin;
  scope: NoteScope;
  sources: MemorySource[];
  /** Older suggestions for the same note, not listed. */
  older_count: number;
  target: MemorySuggestionTarget;
}

export interface ListMemoriesQuery {
  scope?: NoteScope;
  status?: NoteStatus;
  kind?: NoteKind;
  cursor?: string;
  limit?: number;
  includeHistory?: boolean;
  includeCandidates?: boolean;
}

export interface SearchMemoriesQuery {
  q: string;
  scope?: NoteScope;
  kind?: NoteKind;
  limit?: number;
}

/** `memory_update.patch`: an absent field is untouched. */
export interface MemoryPatch {
  title?: string;
  body?: string;
  aliases?: string[];
  tags?: string[];
  fact_key?: string | null;
}

export type MemoryUpdate =
  | { action: "edit"; patch: MemoryPatch; reason?: string }
  | { action: "archive"; reason?: string }
  | {
      action: "confirm";
      /**
       * A suggestion's: the revision of the note it replaces, as this window
       * read it (design 09 §3.2 S5) — a note edited since is a `409`.
       */
      target_revision?: string;
      reason?: string;
    };

/** `POST /v1/memories` — the `memory_store` body. */
export interface CreateMemoryInput {
  title: string;
  body: string;
  kind: NoteKind;
  scope?: NoteScope;
  aliases?: string[];
  tags?: string[];
  fact_key?: string;
}

/** `If-Match` carries the revision — quoted, as the daemon answers it. */
function ifMatch(revision: string): Record<string, string> {
  return { "If-Match": `"${revision}"` };
}

function headers(
  workspacePath: string | null,
  revision?: string,
): Record<string, string> | undefined {
  const project = workspaceHeader(workspacePath);
  if (revision === undefined) return project;
  return { ...project, ...ifMatch(revision) };
}

function notePath(id: string): string {
  return `/v1/memories/${encodeURIComponent(id)}`;
}

/** `GET /v1/memories` — one page of metadata, newest `updated_at` first. */
export async function listMemories(
  query: ListMemoriesQuery,
  workspacePath: string | null,
  signal?: AbortSignal,
): Promise<MemoryListPage> {
  return await apiFetch<MemoryListPage>("/v1/memories", {
    signal,
    headers: headers(workspacePath),
    query: {
      scope: query.scope,
      status: query.status,
      kind: query.kind,
      cursor: query.cursor,
      limit: query.limit,
      // Sent only when asked for, so the daemon's defaults stay the default.
      include_history: query.includeHistory === true ? true : undefined,
      include_candidates: query.includeCandidates === true ? true : undefined,
    },
  });
}

/** `GET /v1/memories/search` — lexical, cited, active-only by default. */
export async function searchMemories(
  query: SearchMemoriesQuery,
  workspacePath: string | null,
  signal?: AbortSignal,
): Promise<MemorySearchPage> {
  return await apiFetch<MemorySearchPage>("/v1/memories/search", {
    signal,
    headers: headers(workspacePath),
    query: {
      q: query.q,
      scope: query.scope,
      kind: query.kind,
      limit: query.limit,
    },
  });
}

/**
 * `GET /v1/memories/recently-learned` — active `extracted_user` notes, newest
 * first, over the daemon's default window (seven days). Both partitions the
 * header allows; the view narrows by `scope` itself.
 */
export async function listRecentlyLearned(
  workspacePath: string | null,
  signal?: AbortSignal,
): Promise<MemoryListItem[]> {
  const page = await apiFetch<{ items: MemoryListItem[] }>(
    "/v1/memories/recently-learned",
    { signal, headers: headers(workspacePath) },
  );
  return page.items;
}

/**
 * `GET /v1/memories/suggestions` — the pending suggestions, newest first, one
 * per note they would replace. Both partitions the header allows; the view
 * narrows by `scope` itself.
 */
export async function listSuggestions(
  workspacePath: string | null,
  signal?: AbortSignal,
): Promise<MemorySuggestion[]> {
  const page = await apiFetch<{ items: MemorySuggestion[] }>(
    "/v1/memories/suggestions",
    { signal, headers: headers(workspacePath) },
  );
  return page.items;
}

/** `GET /v1/memories/{id}`, or `?revision=` for one kept `.history` copy. */
export async function getMemory(
  id: string,
  workspacePath: string | null,
  options: { revision?: string } = {},
  signal?: AbortSignal,
): Promise<MemoryNote> {
  return await apiFetch<MemoryNote>(notePath(id), {
    signal,
    headers: headers(workspacePath),
    query: { revision: options.revision },
  });
}

/** `GET /v1/memories/{id}/history` — newest first, the current one included. */
export async function getMemoryHistory(
  id: string,
  workspacePath: string | null,
  signal?: AbortSignal,
): Promise<MemoryRevision[]> {
  const page = await apiFetch<{ revisions: MemoryRevision[] }>(
    `${notePath(id)}/history`,
    { signal, headers: headers(workspacePath) },
  );
  return page.revisions;
}

/** `POST /v1/memories` — `201`, or `200` with `dedup: "existing"`. */
export async function createMemory(
  input: CreateMemoryInput,
  workspacePath: string | null,
): Promise<MemoryStoreResult> {
  return await apiFetch<MemoryStoreResult>("/v1/memories", {
    method: "POST",
    headers: headers(workspacePath),
    body: input,
  });
}

/** `POST /v1/memories/{id}` — edit, archive or confirm, under `If-Match`. */
export async function updateMemory(
  id: string,
  revision: string,
  update: MemoryUpdate,
  workspacePath: string | null,
): Promise<MemoryUpdateResult> {
  return await apiFetch<MemoryUpdateResult>(notePath(id), {
    method: "POST",
    headers: headers(workspacePath, revision),
    body: update,
  });
}

/**
 * `POST /v1/memories/{id}/undo` — withdraw a learned note (what it replaced
 * comes back) or dismiss a suggestion (*Keep mine*); refused
 * (`409 MEMORY_NOT_UNDOABLE`) for a note the owner saved, edited or confirmed.
 */
export async function undoMemory(
  id: string,
  revision: string,
  workspacePath: string | null,
): Promise<MemoryWithdrawn> {
  return await apiFetch<MemoryWithdrawn>(`${notePath(id)}/undo`, {
    method: "POST",
    headers: headers(workspacePath, revision),
  });
}

/** `DELETE /v1/memories/{id}` — the note, its `.history` and its index rows. */
export async function forgetMemory(
  id: string,
  revision: string,
  workspacePath: string | null,
): Promise<MemoryForgotten> {
  return await apiFetch<MemoryForgotten>(notePath(id), {
    method: "DELETE",
    headers: headers(workspacePath, revision),
  });
}

/** The note moved since the revision this client sent — and nothing else. */
export function isMemoryConflict(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 409 &&
    error.code === "MEMORY_REVISION_CONFLICT"
  );
}
