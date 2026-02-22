/**
 * REST API client for orchestrator latency endpoints.
 */

import { get } from "svelte/store";
import { connectionInfo, type ConnectionInfo } from "../daemon";
import type { OrchestratorLatencyRecord, LatencyAggregate } from "../types";

async function ensureConnection(): Promise<ConnectionInfo> {
  const conn = get(connectionInfo);
  if (!conn) throw new Error("Not connected to daemon");
  return conn;
}

/** GET /v1/orchestrator/latency — query latency records */
export async function getLatencyRecords(
  mode?: string,
  from?: string,
  to?: string,
  limit?: number,
): Promise<OrchestratorLatencyRecord[]> {
  const conn = await ensureConnection();
  const params = new URLSearchParams();
  if (mode !== undefined) params.set("mode", mode);
  if (from !== undefined) params.set("from", from);
  if (to !== undefined) params.set("to", to);
  if (limit !== undefined) params.set("limit", String(limit));
  const qs = params.toString();

  const response = await fetch(
    `${conn.baseUrl}/v1/orchestrator/latency${qs ? `?${qs}` : ""}`,
    { headers: { Authorization: `Bearer ${conn.token}` } },
  );

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(
      data.error?.message ||
        `Failed to fetch latency records: ${response.statusText}`,
    );
  }
  const data = await response.json();
  return data.records;
}

/** GET /v1/orchestrator/latency/aggregate — aggregated P50/P95/P99 by mode */
export async function getLatencyAggregates(
  from?: string,
  to?: string,
): Promise<LatencyAggregate[]> {
  const conn = await ensureConnection();
  const params = new URLSearchParams();
  if (from !== undefined) params.set("from", from);
  if (to !== undefined) params.set("to", to);
  const qs = params.toString();

  const response = await fetch(
    `${conn.baseUrl}/v1/orchestrator/latency/aggregate${qs ? `?${qs}` : ""}`,
    { headers: { Authorization: `Bearer ${conn.token}` } },
  );

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(
      data.error?.message ||
        `Failed to fetch latency aggregates: ${response.statusText}`,
    );
  }
  const data = await response.json();
  return data.aggregates;
}
