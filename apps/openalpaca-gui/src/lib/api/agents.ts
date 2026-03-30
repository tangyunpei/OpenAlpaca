/**
 * REST API client for agent endpoints.
 */

import { ensureConnection } from "./connection";
import type {
  Agent,
  AgentDetailResponse,
  AgentActionResponse,
  AgentConfigResponse,
  UpdateAgentConfigRequest,
  CreateAgentRequest,
  CreateAgentFromTomlRequest,
} from "../types";

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

/** GET /v1/agents/{id}/config — returns config + config_version */
export async function getAgentConfig(id: string): Promise<AgentConfigResponse> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agents/${id}/config`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error || `Failed to fetch agent config: ${response.statusText}`);
  }
  return await response.json();
}

/** PUT /v1/agents/{id}/config — optimistic locking update */
export async function updateAgentConfig(
  id: string,
  req: UpdateAgentConfigRequest,
): Promise<{ agent_id: string; config_version: number; status: string }> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agents/${id}/config`, {
    method: "PUT",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify(req),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    if (response.status === 409) {
      throw new Error("CONFIG_CONFLICT");
    }
    throw new Error(data.error || `Failed to update agent config: ${response.statusText}`);
  }
  return await response.json();
}

/** POST /v1/agents — create new agent */
export async function createAgent(
  req: CreateAgentRequest,
): Promise<{ agent_id: string; status: string }> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agents`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify(req),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error || `Failed to create agent: ${response.statusText}`);
  }
  return await response.json();
}

/** POST /v1/agents/from-toml — create from raw TOML */
export async function createAgentFromToml(
  req: CreateAgentFromTomlRequest,
): Promise<{ agent_id: string; status: string }> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agents/from-toml`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify(req),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error || `Failed to create agent from TOML: ${response.statusText}`);
  }
  return await response.json();
}

/** DELETE /v1/agents/{id} — archive agent */
export async function deleteAgent(
  id: string,
): Promise<{ agent_id: string; status: string }> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agents/${id}`, {
    method: "DELETE",
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error || `Failed to delete agent: ${response.statusText}`);
  }
  return await response.json();
}
