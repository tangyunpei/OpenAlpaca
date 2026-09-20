/**
 * `/v1/artifacts*` — the artifact resource (plan §4.9).
 *
 * These types were the *proposed* shapes in `unbacked.ts` until Phase 3 landed
 * the routes; they now describe what the daemon actually serves, with the
 * additive superset fields (`origin`, `pinned`, `missing`, `path`,
 * `project_root`, `rel_path`) the store writes.
 *
 * Two URL builders sit beside the fetchers because the content routes are the
 * one part of the surface a *browser* loads rather than this client: they check
 * `?token=` inline (as the SSE and WebSocket routes do), so an `<img src>` can
 * point straight at the daemon. Both return `null` until the connection is
 * known — a caller renders its empty state rather than a broken image.
 */

import { getCachedConnection } from "../connection";
import { apiFetch, apiRequest } from "../http";

export type ArtifactKind =
  | "markdown"
  | "code"
  | "terminal"
  | "table"
  | "plan"
  | "image"
  | "html"
  | "binary";

/** How the row got here: an agent wrote it, or a user uploaded it. */
export type ArtifactOrigin = "produced" | "upload";

export interface Artifact {
  id: string;
  name: string;
  kind: ArtifactKind;
  mime_type: string;
  size_bytes: number;
  task_id: string | null;
  task_title: string | null;
  agent_id: string | null;
  agent_template_id: string | null;
  version: number;
  version_count: number;
  summary: string | null;
  metadata: Record<string, unknown> | null;
  created_at: string;
  updated_at: string;
  origin: ArtifactOrigin;
  /** Server-authoritative since `PUT …/pin`; the local cache only mirrors it. */
  pinned: boolean;
  /** The row outlived its bytes — the detail says so instead of a blank pane. */
  missing: boolean;
  path: string;
  project_root: string | null;
  rel_path: string;
}

export interface ArtifactListPage {
  artifacts: Artifact[];
  /** The unpaged `COUNT(*)`, so a page can say what it is a page of. */
  total: number;
}

export interface ArtifactVersion {
  version: number;
  note: string;
  author_agent_id: string | null;
  created_at: string;
  size_bytes: number;
  /** `null` on v1: there is nothing to have changed from. */
  added_lines: number | null;
  removed_lines: number | null;
}

export interface ArtifactDiff {
  from: number;
  to: number;
  added_lines: number;
  removed_lines: number;
  format: "unified";
  patch: string;
}

export interface ArtifactPinState {
  id: string;
  pinned: boolean;
}

export interface ListArtifactsQuery {
  taskId?: string;
  kind?: ArtifactKind;
  origin?: ArtifactOrigin;
  projectRoot?: string;
  pinned?: boolean;
  /** Substring match over the name, server-side. */
  q?: string;
  /** Rows whose bytes are gone are hidden unless this is set. */
  includeMissing?: boolean;
  limit?: number;
  offset?: number;
}

/** `GET /v1/artifacts` → `{ artifacts, total }`. */
export async function listArtifacts(
  query: ListArtifactsQuery = {},
  signal?: AbortSignal,
): Promise<ArtifactListPage> {
  return await apiFetch<ArtifactListPage>("/v1/artifacts", {
    signal,
    query: {
      task_id: query.taskId,
      kind: query.kind,
      origin: query.origin,
      project_root: query.projectRoot,
      pinned: query.pinned,
      q: query.q,
      include_missing: query.includeMissing,
      limit: query.limit,
      offset: query.offset,
    },
  });
}

/** `GET /v1/artifacts/{id}` — 404 for an unknown row and for another owner's. */
export async function getArtifact(
  id: string,
  signal?: AbortSignal,
): Promise<Artifact> {
  return await apiFetch<Artifact>(`/v1/artifacts/${encodeURIComponent(id)}`, {
    signal,
  });
}

/** `GET /v1/artifacts/{id}/versions` — newest first. */
export async function listArtifactVersions(
  id: string,
  signal?: AbortSignal,
): Promise<ArtifactVersion[]> {
  const page = await apiFetch<{ versions: ArtifactVersion[] }>(
    `/v1/artifacts/${encodeURIComponent(id)}/versions`,
    { signal },
  );
  return page.versions;
}

/**
 * `GET /v1/artifacts/{id}/diff?from=&to=`.
 *
 * Rejects with an `ApiError` carrying the daemon's own code: `NOT_DIFFABLE`
 * (image/binary) and `DIFF_TOO_LARGE` are both 409, and the tab renders each
 * refusal's message rather than an empty patch.
 */
export async function getArtifactDiff(
  id: string,
  from: number,
  to: number,
  signal?: AbortSignal,
): Promise<ArtifactDiff> {
  return await apiFetch<ArtifactDiff>(
    `/v1/artifacts/${encodeURIComponent(id)}/diff`,
    { signal, query: { from, to } },
  );
}

/**
 * `GET /v1/artifacts/{id}/content` as text, for the preview renderers.
 *
 * The same route the `<img>` loads by URL; this is the header-authenticated
 * read of it, used for the kinds that are rendered as characters rather than
 * fetched by the browser. A 410 `ARTIFACT_GONE` arrives as an `ApiError` and
 * the pane says the bytes are gone.
 */
export async function getArtifactText(
  id: string,
  signal?: AbortSignal,
): Promise<string> {
  const response = await apiRequest(
    `/v1/artifacts/${encodeURIComponent(id)}/content`,
    { signal },
  );
  return await response.text();
}

/** `PUT /v1/artifacts/{id}/pin` — the server is the authority on a pin. */
export async function setArtifactPinned(
  id: string,
  pinned: boolean,
): Promise<ArtifactPinState> {
  return await apiFetch<ArtifactPinState>(
    `/v1/artifacts/${encodeURIComponent(id)}/pin`,
    { method: "PUT", body: { pinned } },
  );
}

function contentUrl(path: string): string | null {
  const info = getCachedConnection();
  if (info === null) return null;
  return `${info.baseUrl}${path}?token=${encodeURIComponent(info.token)}`;
}

/**
 * A browser-loadable URL for an artifact's bytes — the head version, or a
 * numbered one. `null` before the connection is bootstrapped.
 */
export function artifactContentUrl(
  id: string,
  version?: number,
): string | null {
  const base = `/v1/artifacts/${encodeURIComponent(id)}`;
  return contentUrl(
    version === undefined
      ? `${base}/content`
      : `${base}/versions/${version}/content`,
  );
}

/** The same, for the `file_assets` content route an attachment resolves to. */
export function fileContentUrl(id: string): string | null {
  return contentUrl(`/v1/files/${encodeURIComponent(id)}/content`);
}
