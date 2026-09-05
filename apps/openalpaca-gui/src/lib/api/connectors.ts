/** `/v1/connectors*`. */

import { apiFetch } from "../http";
import type { Connector, ConnectorAction, ExtensionRow } from "./types";

/**
 * `GET /v1/connectors` — bare array of `{ id, name, status, configured }`.
 * Display names are a hardcoded match on `telegram`/`imessage`; MCP servers and
 * plugin-declared connectors never appear here (GAP-17).
 */
export async function listConnectors(
  signal?: AbortSignal,
): Promise<Connector[]> {
  return await apiFetch<Connector[]>("/v1/connectors", { signal });
}

/** `POST /v1/connectors/{id}/action` */
export async function performConnectorAction(
  id: string,
  action: ConnectorAction,
): Promise<void> {
  await apiFetch<void>(`/v1/connectors/${encodeURIComponent(id)}/action`, {
    method: "POST",
    body: { action },
  });
}

/** `POST /v1/connectors/{id}/config` — bearer-token connectors only. */
export async function configureConnector(
  id: string,
  token: string,
): Promise<void> {
  await apiFetch<void>(`/v1/connectors/${encodeURIComponent(id)}/config`, {
    method: "POST",
    body: { token },
  });
}

/** `GET /v1/connectors/{id}/settings` */
export async function getConnectorSettings(
  id: string,
  signal?: AbortSignal,
): Promise<Record<string, string | null>> {
  return await apiFetch<Record<string, string | null>>(
    `/v1/connectors/${encodeURIComponent(id)}/settings`,
    { signal },
  );
}

/** `PUT /v1/connectors/{id}/settings` — keys must be `"{id}."`-prefixed. */
export async function updateConnectorSettings(
  id: string,
  settings: Record<string, string>,
): Promise<void> {
  await apiFetch<void>(`/v1/connectors/${encodeURIComponent(id)}/settings`, {
    method: "PUT",
    body: { settings },
  });
}

/**
 * The design's `unwired` badge: a plugin declares a connector that never
 * registered. Derivable client-side, which is why it is not a gap on its own.
 *
 * The join reads the extension rows: `connector` is non-null only while the
 * plugin is loaded, so a disabled plugin no longer claims a connector it is
 * not serving (ADR-030 §8 — a `disabled` row with a non-null `connector` is a
 * teardown bug, not an `unwired` badge).
 */
export function findUnwiredConnectors(
  extensions: ExtensionRow[],
  connectors: Connector[],
): Array<{ connectorId: string; declaredBy: string }> {
  const registered = new Set(connectors.map((c) => c.id));
  return extensions
    .filter(
      (e): e is ExtensionRow & { connector: string } => e.connector !== null,
    )
    .filter((e) => !registered.has(e.connector))
    .map((e) => ({ connectorId: e.connector, declaredBy: e.id }));
}
