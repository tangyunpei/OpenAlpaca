/**
 * REST API client for agent endpoints.
 */

import { get } from "svelte/store";
import { connectionInfo, type ConnectionInfo } from "../daemon";
import type { Agent, AgentDetailResponse, AgentActionResponse } from "../types";

async function ensureConnection(): Promise<ConnectionInfo> {
  const conn = get(connectionInfo);
  if (!conn) throw new Error("Not connected to daemon");
  return conn;
}

export interface ListAgentsQuery {
  status?: string;
  skill?: string;
  limit?: number;
}

/** GET /v1/agents */
export async function getAgents(query?: ListAgentsQuery): Promise<Agent[]> {
  const conn = await ensureConnection();
  const params = new URLSearchParams();
  if (query?.status) params.set("status", query.status);
  if (query?.skill) params.set("skill", query.skill);
  if (query?.limit) params.set("limit", String(query.limit));
  const qs = params.toString();

  const response = await fetch(
    `${conn.baseUrl}/v1/agents${qs ? `?${qs}` : ""}`,
    { headers: { Authorization: `Bearer ${conn.token}` } },
  );

  if (!response.ok) {
    throw new Error(`Failed to fetch agents: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/agents/{id} — returns agent + metrics */
export async function getAgent(id: string): Promise<AgentDetailResponse> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agents/${id}`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    throw new Error(`Failed to fetch agent ${id}: ${response.statusText}`);
  }
  return await response.json();
}

/** POST /v1/agents/{id}/action — action: "pause" | "resume" */
export async function performAgentAction(
  id: string,
  action: "pause" | "resume",
): Promise<AgentActionResponse> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agents/${id}/action`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify({ action }),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error || `Agent action ${action} failed: ${response.statusText}`);
  }
  return await response.json();
}
