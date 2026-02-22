/**
 * REST API client for daemon config endpoints.
 */

import { get } from "svelte/store";
import { connectionInfo, type ConnectionInfo } from "../daemon";

async function ensureConnection(): Promise<ConnectionInfo> {
  const conn = get(connectionInfo);
  if (!conn) throw new Error("Not connected to daemon");
  return conn;
}

// ── Types ───────────────────────────────────────────────────────────

export interface DaemonProvidersResponse {
  web_search: {
    api_key_configured: boolean;
    api_key_hint: string;
    timeout_secs: number;
  };
}

export interface UpdateWebSearchRequest {
  api_key?: string;
  timeout_secs?: number;
}

// ── API calls ───────────────────────────────────────────────────────

/** GET /v1/daemon/config/providers */
export async function getDaemonProviders(): Promise<DaemonProvidersResponse> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/daemon/config/providers`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(
      data.error?.message ||
        `Failed to fetch daemon providers: ${response.statusText}`,
    );
  }
  return await response.json();
}

/** PUT /v1/daemon/config/providers/web-search */
export async function updateWebSearchConfig(
  req: UpdateWebSearchRequest,
): Promise<void> {
  const conn = await ensureConnection();
  const response = await fetch(
    `${conn.baseUrl}/v1/daemon/config/providers/web-search`,
    {
      method: "PUT",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${conn.token}`,
      },
      body: JSON.stringify(req),
    },
  );

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(
      data.error?.message ||
        `Failed to update web search config: ${response.statusText}`,
    );
  }
}
