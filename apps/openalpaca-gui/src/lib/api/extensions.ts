/**
 * `/v1/extensions*` — the ENABLE axis (ADR-030 §8).
 *
 * One list over both kinds. The Settings page asks for `include_orphaned=true`
 * because the API hides orphans by default (so scripts and `openalpaca ext
 * list` see only real extensions) and the settings page is exactly where an
 * owner needs to see and Remove one (§9.2).
 *
 * Errors are the **flat** `{"error": "<word>"}` envelope (§8, ruling R20), so
 * `ApiError.message` *is* the word — `extensionErrorCopy` turns it into row
 * copy.
 */

import { apiFetch } from "../http";
import type { ExtensionRow, ExtensionVerb, ExtensionKind } from "./types";

/** `GET /v1/extensions?include_orphaned=true` — bare array, both kinds. */
export async function listExtensions(
  signal?: AbortSignal,
): Promise<ExtensionRow[]> {
  return await apiFetch<ExtensionRow[]>("/v1/extensions", {
    query: { include_orphaned: true },
    signal,
  });
}

/**
 * `POST /v1/extensions/{kind}/{id}/{verb}` — returns the resulting row.
 *
 * `200` even when the bring-up half fails: the disposition write succeeded and
 * the connection outcome is a separate fact in the body (§8).
 */
export async function runExtensionVerb(
  kind: ExtensionKind,
  id: string,
  verb: ExtensionVerb,
): Promise<ExtensionRow> {
  return await apiFetch<ExtensionRow>(
    `/v1/extensions/${kind}/${encodeURIComponent(id)}/${verb}`,
    { method: "POST" },
  );
}

/**
 * `DELETE /v1/extensions/plugin/{id}` — orphaned rows only (`409
 * not_orphaned` otherwise). Removes the `.permissions.toml` entry and the
 * ledger record; it never touches a plugin directory (that is GAP-24).
 */
export async function removeExtension(id: string): Promise<void> {
  await apiFetch<void>(`/v1/extensions/plugin/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
}

/**
 * `POST /v1/extensions/plugin/{id}/config` — one key at a time.
 *
 * On success the daemon writes the key and, if the row is
 * `Failed{NeedsConfig}` with the bit set and consent recorded, invokes the
 * `enable` verb itself, so setting the last missing key starts the plugin
 * without a second call (§8).
 */
export async function setExtensionConfig(
  id: string,
  key: string,
  value: string,
): Promise<void> {
  await apiFetch<void>(
    `/v1/extensions/plugin/${encodeURIComponent(id)}/config`,
    { method: "POST", body: { key, value } },
  );
}
