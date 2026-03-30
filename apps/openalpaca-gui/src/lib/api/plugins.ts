/**
 * REST API client for plugin endpoints.
 */

import { ensureConnection } from "./connection";

export interface PluginInfo {
  name: string;
  version: string;
  status: string;
  tools: string[];
  connector: string | null;
  provider: string | null;
  models: string[];
}

/** GET /v1/plugins */
export async function getPlugins(): Promise<PluginInfo[]> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/plugins`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    throw new Error(`Failed to fetch plugins: ${response.statusText}`);
  }
  return await response.json();
}

/** POST /v1/plugins/{name}/approve */
export async function approvePlugin(name: string): Promise<void> {
  const conn = await ensureConnection();
  const response = await fetch(
    `${conn.baseUrl}/v1/plugins/${encodeURIComponent(name)}/approve`,
    {
      method: "POST",
      headers: { Authorization: `Bearer ${conn.token}` },
    },
  );

  if (!response.ok) {
    throw new Error(`Failed to approve plugin: ${response.statusText}`);
  }
}

/** POST /v1/plugins/{name}/deny */
export async function denyPlugin(name: string): Promise<void> {
  const conn = await ensureConnection();
  const response = await fetch(
    `${conn.baseUrl}/v1/plugins/${encodeURIComponent(name)}/deny`,
    {
      method: "POST",
      headers: { Authorization: `Bearer ${conn.token}` },
    },
  );

  if (!response.ok) {
    throw new Error(`Failed to deny plugin: ${response.statusText}`);
  }
}

/** POST /v1/plugins/{name}/enable */
export async function enablePlugin(name: string): Promise<void> {
  const conn = await ensureConnection();
  const response = await fetch(
    `${conn.baseUrl}/v1/plugins/${encodeURIComponent(name)}/enable`,
    {
      method: "POST",
      headers: { Authorization: `Bearer ${conn.token}` },
    },
  );

  if (!response.ok) {
    throw new Error(`Failed to enable plugin: ${response.statusText}`);
  }
}

/** POST /v1/plugins/{name}/disable */
export async function disablePlugin(name: string): Promise<void> {
  const conn = await ensureConnection();
  const response = await fetch(
    `${conn.baseUrl}/v1/plugins/${encodeURIComponent(name)}/disable`,
    {
      method: "POST",
      headers: { Authorization: `Bearer ${conn.token}` },
    },
  );

  if (!response.ok) {
    throw new Error(`Failed to disable plugin: ${response.statusText}`);
  }
}
