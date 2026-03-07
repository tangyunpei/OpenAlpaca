/**
 * REST API client for agent instance endpoints.
 */

import { ensureConnection } from "./connection";
import type { AgentInstance } from "../types";

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
