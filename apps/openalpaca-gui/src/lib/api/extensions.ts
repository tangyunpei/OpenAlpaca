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
import type {
  ExtensionRow,
  ExtensionVerb,
  ExtensionKind,
  InstallResponse,
  McpDeclaration,
  UninstallResponse,
  ValidateResponse,
} from "./types";

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
 * ledger record; it never touches a plugin directory — `uninstallExtension`
 * is the verb that does.
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

/**
 * POST /v1/extensions/plugin
 *
 * Copies the directory at `path` into the plugins root under its own name and
 * runs the load path. **It grants nothing:** the plugin lands `enabled = true`
 * with consent `never_seen`, so the row comes back `unapproved` and the
 * `approve` verb is the single action that starts it. The `manifest` beside the
 * row is the approval preview.
 *
 * `source` is always `"path"`. Installing from a URL is declined until it has
 * had its own security review.
 */
export async function installPlugin(path: string): Promise<InstallResponse> {
  return await apiFetch<InstallResponse>("/v1/extensions/plugin", {
    method: "POST",
    body: { source: "path", path },
  });
}

/**
 * POST /v1/extensions/plugin/validate
 *
 * The dry run: parse the manifest at `path` and report it without copying
 * anything, so the Add form can show what an install would land — and what
 * approving it would grant — before the directory is in the store.
 */
export async function validatePlugin(path: string): Promise<ValidateResponse> {
  return await apiFetch<ValidateResponse>("/v1/extensions/plugin/validate", {
    method: "POST",
    body: { source: "path", path },
  });
}

/**
 * PUT /v1/extensions/plugin/{id}
 *
 * Replaces an installed plugin's tree: T0–T5, the incumbent to
 * `plugins/.trash/`, the replacement renamed into place, then the load path.
 * Consent survives an update whose declared capabilities are unchanged;
 * otherwise `consent_reset` is set and `added_capabilities` says what the
 * replacement additionally asks for.
 */
export async function updatePlugin(
  id: string,
  path: string,
): Promise<InstallResponse> {
  return await apiFetch<InstallResponse>(
    `/v1/extensions/plugin/${encodeURIComponent(id)}`,
    { method: "PUT", body: { source: "path", path } },
  );
}

/**
 * POST /v1/extensions/mcp
 *
 * Writes a `[servers.<name>]` block into `config/mcp.toml` and connects it.
 * Writing a server into your own config *is* the consent, so there is no
 * approve step — `enabled: false` is how you declare one without starting it.
 */
export async function addMcpServer(
  declaration: McpDeclaration,
): Promise<InstallResponse> {
  return await apiFetch<InstallResponse>("/v1/extensions/mcp", {
    method: "POST",
    body: declaration as unknown as Record<string, unknown>,
  });
}

/**
 * DELETE /v1/extensions/{kind}/{id}?uninstall=true
 *
 * The real removal — and it deletes nothing: a plugin's directory is *moved* to
 * `plugins/.trash/<id>-<ts>/` (the response says where), and `keepData` decides
 * `plugins/.data/<id>/`, which is moved rather than removed when it is not
 * kept. An MCP server's `[servers.<name>]` block is removed from
 * `config/mcp.toml`, and the server has to be turned off first
 * (`409 not_disabled`).
 *
 * The `uninstall` flag is what separates this from `removeExtension`, which
 * only ever drops an orphan's row.
 */
export async function uninstallExtension(
  kind: ExtensionKind,
  id: string,
  keepData = true,
): Promise<UninstallResponse> {
  return await apiFetch<UninstallResponse>(
    `/v1/extensions/${kind}/${encodeURIComponent(id)}`,
    { method: "DELETE", query: { uninstall: true, keep_data: keepData } },
  );
}
