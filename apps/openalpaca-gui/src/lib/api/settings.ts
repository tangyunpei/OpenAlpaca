/**
 * REST API client for LLM settings endpoints.
 */

import { get } from "svelte/store";
import { connectionInfo, type ConnectionInfo } from "../daemon";
import type {
  LlmSettingsResponse,
  AddKeyRequest,
  ReorderKeysRequest,
  SetKeyPriorityRequest,
  ValidateKeyRequest,
  KeyValidationResult,
  KeyStatusMap,
  ModelEntry,
  DiscoveredCredentialInfo,
  CliBackendStatus,
  ProviderUsageSummary,
} from "../types";

async function ensureConnection(): Promise<ConnectionInfo> {
  const conn = get(connectionInfo);
  if (!conn) throw new Error("Not connected to daemon");
  return conn;
}

/** GET /v1/settings/llm */
export async function getLlmSettings(): Promise<LlmSettingsResponse> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/settings/llm`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch settings: ${response.statusText}`);
  }
  return await response.json();
}

/** PUT /v1/settings/llm — add/update key */
export async function addOrUpdateKey(req: AddKeyRequest): Promise<void> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/settings/llm`, {
    method: "PUT",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify(req),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to add key: ${response.statusText}`);
  }
}

/** DELETE /v1/settings/llm/keys/{provider}/{keyId} */
export async function removeKey(provider: string, keyId: string): Promise<void> {
  const conn = await ensureConnection();
  const response = await fetch(
    `${conn.baseUrl}/v1/settings/llm/keys/${encodeURIComponent(provider)}/${encodeURIComponent(keyId)}`,
    {
      method: "DELETE",
      headers: { Authorization: `Bearer ${conn.token}` },
    },
  );

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to remove key: ${response.statusText}`);
  }
}

/** PUT /v1/settings/llm/keys/reorder */
export async function reorderKeys(req: ReorderKeysRequest): Promise<void> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/settings/llm/keys/reorder`, {
    method: "PUT",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify(req),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to reorder keys: ${response.statusText}`);
  }
}

/** PUT /v1/settings/llm/keys/priority — update a key's priority */
export async function setKeyPriority(req: SetKeyPriorityRequest): Promise<void> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/settings/llm/keys/priority`, {
    method: "PUT",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify(req),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(
      data.error?.message || `Failed to set key priority: ${response.statusText}`,
    );
  }
}

/** POST /v1/settings/llm/validate */
export async function validateKey(req: ValidateKeyRequest): Promise<KeyValidationResult> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/settings/llm/validate`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify(req),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Validation failed: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/settings/llm/status */
export async function getKeyStatus(): Promise<KeyStatusMap> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/settings/llm/status`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch status: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/models — list all available models */
export async function getAvailableModels(): Promise<ModelEntry[]> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/models`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch models: ${response.statusText}`);
  }
  return await response.json();
}

/** POST /v1/models/refresh — refresh models from provider APIs */
export async function refreshAvailableModels(): Promise<ModelEntry[]> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/models/refresh`, {
    method: "POST",
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to refresh models: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/settings/llm/credentials — list discovered OAuth credentials */
export async function getDiscoveredCredentials(): Promise<DiscoveredCredentialInfo[]> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/settings/llm/credentials`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch credentials: ${response.statusText}`);
  }
  return await response.json();
}

/** POST /v1/settings/llm/credentials/rescan — rescan for OAuth credentials */
export async function rescanCredentials(): Promise<DiscoveredCredentialInfo[]> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/settings/llm/credentials/rescan`, {
    method: "POST",
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to rescan credentials: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/settings/llm/cli-backends — list CLI backend status */
export async function getCliBackends(): Promise<CliBackendStatus[]> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/settings/llm/cli-backends`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch CLI backends: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/settings/llm/providers/usage — provider-level usage summaries */
export async function getProviderUsage(): Promise<ProviderUsageSummary[]> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/settings/llm/providers/usage`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch provider usage: ${response.statusText}`);
  }
  return await response.json();
}
