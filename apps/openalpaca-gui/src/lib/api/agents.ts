/** `/v1/agents*`, `/v1/agent-templates*`, `/v1/agent-instances`. */

import { ApiError, apiFetch } from "../http";
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

/**
 * `DELETE /v1/agent-templates/{id}` — archives the file that holds the
 * template. A built-in template no file holds and a plugin's template are
 * refused, nothing changed: `409 {error: "bundled" | "plugin", message}`,
 * a plugin's naming it in `plugin` and in the sentence (subagent design
 * §2.10 T-2). The shared parser hands a string `error` through as the
 * message, so the refusal is read here: the sentence becomes the message
 * and the word the `code`.
 */
export async function deleteAgentTemplate(
  id: string,
): Promise<{ id: string; status: string }> {
  try {
    return await apiFetch<{ id: string; status: string }>(
      `/v1/agent-templates/${encodeURIComponent(id)}`,
      { method: "DELETE" },
    );
  } catch (error) {
    throw templateRefusal(error);
  }
}

/** A `409 {error: "<word>", message}` refusal read as its sentence. */
function templateRefusal(error: unknown): unknown {
  if (!(error instanceof ApiError) || error.status !== 409) return error;
  const body = error.detail as Record<string, unknown> | null;
  if (
    typeof body?.error !== "string" ||
    typeof body.message !== "string" ||
    body.message.length === 0
  ) {
    return error;
  }
  return new ApiError(body.message, error.status, body.error, error.detail);
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
