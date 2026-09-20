/** `/v1/agents*`, `/v1/agent-templates*`, `/v1/agent-instances`. */

import { apiFetch } from "../http";
import type {
  AgentActionResponse,
  AgentConfigFile,
  AgentConfigResponse,
  AgentDetailResponse,
  AgentInstance,
  AgentTemplate,
} from "./types";

/** `GET /v1/agent-templates` */
export async function listAgentTemplates(
  signal?: AbortSignal,
): Promise<AgentTemplate[]> {
  return await apiFetch<AgentTemplate[]>("/v1/agent-templates", { signal });
}

/** `GET /v1/agent-templates/{id}` */
export async function getAgentTemplate(
  id: string,
  signal?: AbortSignal,
): Promise<AgentTemplate> {
  return await apiFetch<AgentTemplate>(
    `/v1/agent-templates/${encodeURIComponent(id)}`,
    { signal },
  );
}

/** `POST /v1/agent-templates` */
export async function createAgentTemplate(
  config: AgentConfigFile,
): Promise<{ id: string; status: string }> {
  return await apiFetch<{ id: string; status: string }>("/v1/agent-templates", {
    method: "POST",
    body: { config },
  });
}

/** `PUT /v1/agent-templates/{id}` */
export async function updateAgentTemplate(
  id: string,
  config: AgentConfigFile,
): Promise<{ id: string; status: string }> {
  return await apiFetch<{ id: string; status: string }>(
    `/v1/agent-templates/${encodeURIComponent(id)}`,
    { method: "PUT", body: { config } },
  );
}

/** `DELETE /v1/agent-templates/{id}` — archives the template. */
export async function deleteAgentTemplate(
  id: string,
): Promise<{ id: string; status: string }> {
  return await apiFetch<{ id: string; status: string }>(
    `/v1/agent-templates/${encodeURIComponent(id)}`,
    { method: "DELETE" },
  );
}

/** `GET /v1/agent-instances` */
export async function listAgentInstances(
  signal?: AbortSignal,
): Promise<AgentInstance[]> {
  return await apiFetch<AgentInstance[]>("/v1/agent-instances", { signal });
}

/** `GET /v1/agents/{id}` — includes lifetime `metrics`. */
export async function getAgent(
  id: string,
  signal?: AbortSignal,
): Promise<AgentDetailResponse> {
  return await apiFetch<AgentDetailResponse>(
    `/v1/agents/${encodeURIComponent(id)}`,
    { signal },
  );
}

/** `GET /v1/agents/{id}/config` — carries the optimistic-lock version. */
export async function getAgentConfig(
  id: string,
  signal?: AbortSignal,
): Promise<AgentConfigResponse> {
  return await apiFetch<AgentConfigResponse>(
    `/v1/agents/${encodeURIComponent(id)}/config`,
    {
      signal,
    },
  );
}

/** `PUT /v1/agents/{id}/config` — 409 when `config_version` is stale. */
export async function updateAgentConfig(
  id: string,
  config: AgentConfigFile,
  configVersion: number,
): Promise<void> {
  await apiFetch<void>(`/v1/agents/${encodeURIComponent(id)}/config`, {
    method: "PUT",
    body: { config, config_version: configVersion },
  });
}

/** `POST /v1/agents/{id}/action` — pauses/resumes a running *instance*. */
export async function performAgentAction(
  id: string,
  action: "pause" | "resume",
): Promise<AgentActionResponse> {
  return await apiFetch<AgentActionResponse>(
    `/v1/agents/${encodeURIComponent(id)}/action`,
    {
      method: "POST",
      body: { action },
    },
  );
}
