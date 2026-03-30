/**
 * REST API client for agent template endpoints.
 */

import { ensureConnection } from "./connection";
import type { AgentTemplate, AgentConfigFile } from "../types";

/** GET /v1/agent-templates */
export async function getTemplates(): Promise<AgentTemplate[]> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agent-templates`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    throw new Error(`Failed to fetch templates: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/agent-templates/{id} */
export async function getTemplate(id: string): Promise<AgentTemplate> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agent-templates/${id}`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    throw new Error(`Failed to fetch template ${id}: ${response.statusText}`);
  }
  return await response.json();
}

/** POST /v1/agent-templates — create template from config */
export async function createTemplate(
  config: AgentConfigFile,
): Promise<{ id: string; status: string }> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agent-templates`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify({ config }),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error || `Failed to create template: ${response.statusText}`);
  }
  return await response.json();
}

/** PUT /v1/agent-templates/{id} — update template */
export async function updateTemplate(
  id: string,
  config: AgentConfigFile,
): Promise<{ id: string; status: string }> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agent-templates/${id}`, {
    method: "PUT",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify({ config }),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error || `Failed to update template: ${response.statusText}`);
  }
  return await response.json();
}

/** DELETE /v1/agent-templates/{id} — archive template */
export async function deleteTemplate(
  id: string,
): Promise<{ id: string; status: string }> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/agent-templates/${id}`, {
    method: "DELETE",
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error || `Failed to delete template: ${response.statusText}`);
  }
  return await response.json();
}
