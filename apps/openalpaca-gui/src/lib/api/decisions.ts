/**
 * REST API client for dispatch decisions endpoint.
 */

import { ensureConnection } from "./connection";
import type { DispatchDecisionRecord } from "../types";

/** GET /v1/orchestrator/decisions — query dispatch decision history */
export async function getDispatchDecisions(
  mode?: string,
  from?: string,
  to?: string,
  limit?: number,
): Promise<DispatchDecisionRecord[]> {
  const conn = await ensureConnection();
  const params = new URLSearchParams();
  if (mode !== undefined) params.set("mode", mode);
  if (from !== undefined) params.set("from", from);
  if (to !== undefined) params.set("to", to);
  if (limit !== undefined) params.set("limit", String(limit));
  const qs = params.toString();

  const response = await fetch(
    `${conn.baseUrl}/v1/orchestrator/decisions${qs ? `?${qs}` : ""}`,
    { headers: { Authorization: `Bearer ${conn.token}` } },
  );

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(
      data.error?.message ||
        `Failed to fetch dispatch decisions: ${response.statusText}`,
    );
  }
  const data = await response.json();
  return data.records;
}
