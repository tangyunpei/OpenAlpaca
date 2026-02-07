/**
 * REST API client for LLM usage endpoints.
 */

import { get } from "svelte/store";
import { connectionInfo, type ConnectionInfo } from "../daemon";
import type { LlmCallLog, LlmUsageDaily } from "../types";

async function ensureConnection(): Promise<ConnectionInfo> {
  const conn = get(connectionInfo);
  if (!conn) throw new Error("Not connected to daemon");
  return conn;
}

/** GET /v1/llm/usage — query LLM call logs */
export async function getLlmUsage(
  agentId?: string,
  keyId?: string,
  limit?: number,
): Promise<LlmCallLog[]> {
  const conn = await ensureConnection();
  const params = new URLSearchParams();
  if (agentId !== undefined) params.set("agent_id", agentId);
  if (keyId !== undefined) params.set("key_id", keyId);
  if (limit !== undefined) params.set("limit", String(limit));
  const qs = params.toString();

  const response = await fetch(
    `${conn.baseUrl}/v1/llm/usage${qs ? `?${qs}` : ""}`,
    { headers: { Authorization: `Bearer ${conn.token}` } },
  );

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch usage: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/llm/usage/daily — query daily usage aggregates */
export async function getLlmUsageDaily(
  agentId?: string,
  date?: string,
  limit?: number,
): Promise<LlmUsageDaily[]> {
  const conn = await ensureConnection();
  const params = new URLSearchParams();
  if (agentId !== undefined) params.set("agent_id", agentId);
  if (date !== undefined) params.set("date", date);
  if (limit !== undefined) params.set("limit", String(limit));
  const qs = params.toString();

  const response = await fetch(
    `${conn.baseUrl}/v1/llm/usage/daily${qs ? `?${qs}` : ""}`,
    { headers: { Authorization: `Bearer ${conn.token}` } },
  );

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch daily usage: ${response.statusText}`);
  }
  return await response.json();
}
