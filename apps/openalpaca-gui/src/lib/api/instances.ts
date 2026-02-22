/**
 * REST API client for agent instance endpoints.
 */

import { get } from "svelte/store";
import { connectionInfo, type ConnectionInfo } from "../daemon";
import type { AgentInstance } from "../types";

async function ensureConnection(): Promise<ConnectionInfo> {
  const conn = get(connectionInfo);
  if (!conn) throw new Error("Not connected to daemon");
  return conn;
}

/** GET /v1/agent-instances */
export async function getInstances(): Promise<AgentInstance[]> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agent-instances`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    throw new Error(`Failed to fetch instances: ${response.statusText}`);
  }
  return await response.json();
}
