/** `/v1/connectors*`. */

import { apiFetch } from "../http";
import type { Connector, ConnectorAction, PluginInfo } from "./types";

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
 */
export function findUnwiredConnectors(
  plugins: PluginInfo[],
  connectors: Connector[],
): Array<{ connectorId: string; declaredBy: string }> {
  const registered = new Set(connectors.map((c) => c.id));
  return plugins
    .filter(
      (p): p is PluginInfo & { connector: string } => p.connector !== null,
    )
    .filter((p) => !registered.has(p.connector))
    .map((p) => ({ connectorId: p.connector, declaredBy: p.name }));
}
