/**
 * REST API client for orchestrator config endpoints.
 */

import { ensureConnection } from "./connection";
import type { OrchestratorConfigResponse, UpdateOrchestratorRequest } from "../types";

/** GET /v1/orchestrator/config */
export async function getOrchestratorConfig(): Promise<OrchestratorConfigResponse> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/orchestrator/config`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch orchestrator config: ${response.statusText}`);
  }
  return await response.json();
}

/** PUT /v1/orchestrator/config */
export async function updateOrchestratorConfig(
  req: UpdateOrchestratorRequest,
): Promise<void> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/orchestrator/config`, {
    method: "PUT",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify(req),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to update orchestrator config: ${response.statusText}`);
  }
}
