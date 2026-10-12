/**
 * `GET|PUT /v1/persona/user` — the `USER.md` editor (memory redesign, design §6
 * *USER.md*, §7a; owner decisions MD-3 and MD-9).
 *
 * `USER.md` is the owner's standing instructions about themselves. The
 * background learner stopped writing into it at the cutover — learned facts are
 * memory notes now — so this editor (and `openalpaca persona edit user`) is how
 * it changes.
 *
 * The revision is the lowercase hex SHA-256 of the file's bytes: `GET` answers
 * it as `ETag`, `PUT` takes it back as **`If-Match`** and answers
 * `409 PERSONA_REVISION_CONFLICT` when the file moved underneath — the
 * onboarding `update_persona` writes the same file, so a stale editor gets the
 * 409 instead of overwriting it. The body is `{content}` both ways; the new
 * revision comes back as `{etag}`.
 */

import { ApiError, apiFetch, apiRequest } from "../http";

export interface PersonaUser {
  /** The whole document, raw Markdown. */
  content: string;
  /** The revision this content is — what a `PUT` sends back as `If-Match`. */
  etag: string;
}

/** Lowercase hex SHA-256 of `text`'s UTF-8 bytes. */
export async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(text),
  );
  return Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}

/** `"<sha>"`, `W/"<sha>"` or a bare `<sha>` — one revision, three spellings. */
function parseEtag(value: string | null): string | null {
  if (value === null) return null;
  const bare = value.trim().replace(/^W\//, "").replace(/^"|"$/g, "").trim();
  return bare === "" ? null : bare;
}

/**
 * `GET /v1/persona/user`.
 *
 * The revision is read from `ETag`. Should the webview not expose the header,
 * the same value is derived from the content — the tag *is* the SHA-256 of
 * those bytes — so an unreadable header can never turn every save into a
 * conflict the owner has no way to resolve.
 */
export async function getPersonaUser(
  signal?: AbortSignal,
): Promise<PersonaUser> {
  const response = await apiRequest("/v1/persona/user", { signal });
  const body = (await response.json()) as { content: string };
  const etag =
    parseEtag(response.headers.get("ETag")) ?? (await sha256Hex(body.content));
  return { content: body.content, etag };
}

/** `PUT /v1/persona/user` — the whole document, under the revision it edited. */
export async function putPersonaUser(
  content: string,
  etag: string,
): Promise<{ etag: string }> {
  return await apiFetch<{ etag: string }>("/v1/persona/user", {
    method: "PUT",
    headers: { "If-Match": `"${etag}"` },
    body: { content },
  });
}

/** `USER.md` moved since the revision this client sent — and nothing else. */
export function isPersonaConflict(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 409 &&
    error.code === "PERSONA_REVISION_CONFLICT"
  );
}
