/** `/v1/settings/llm*` and `/v1/models*`. */

import { apiFetch } from "../http";
import type {
  AddKeyRequest,
  CliBackendStatus,
  DiscoveredCredentialInfo,
  KeyStatusMap,
  KeyValidationResult,
  LlmSettingsResponse,
  ModelEntry,
  ProviderUsageSummary,
  ReorderKeysRequest,
  SetKeyPriorityRequest,
  ValidateKeyRequest,
} from "./types";

/** `GET /v1/settings/llm` — the full (redacted) LlmConfig. */
export async function getLlmSettings(
  signal?: AbortSignal,
): Promise<LlmSettingsResponse> {
  return await apiFetch<LlmSettingsResponse>("/v1/settings/llm", { signal });
}

/** `PUT /v1/settings/llm` — upsert one key. */
export async function upsertKey(req: AddKeyRequest): Promise<void> {
  await apiFetch<void>("/v1/settings/llm", { method: "PUT", body: req });
}

/** `DELETE /v1/settings/llm/keys/{provider}/{keyId}` */
export async function removeKey(
  provider: string,
  keyId: string,
): Promise<void> {
  await apiFetch<void>(
    `/v1/settings/llm/keys/${encodeURIComponent(provider)}/${encodeURIComponent(keyId)}`,
    { method: "DELETE" },
  );
}

/** `PUT /v1/settings/llm/keys/reorder` */
export async function reorderKeys(req: ReorderKeysRequest): Promise<void> {
  await apiFetch<void>("/v1/settings/llm/keys/reorder", {
    method: "PUT",
    body: req,
  });
}

/** `PUT /v1/settings/llm/keys/priority` */
export async function setKeyPriority(
  req: SetKeyPriorityRequest,
): Promise<void> {
  await apiFetch<void>("/v1/settings/llm/keys/priority", {
    method: "PUT",
    body: req,
  });
}

/** `POST /v1/settings/llm/validate` */
export async function validateKey(
  req: ValidateKeyRequest,
): Promise<KeyValidationResult> {
  return await apiFetch<KeyValidationResult>("/v1/settings/llm/validate", {
    method: "POST",
    body: req,
  });
}

/** `GET /v1/settings/llm/status` — per-key health. */
export async function getKeyStatus(
  signal?: AbortSignal,
): Promise<KeyStatusMap> {
  return await apiFetch<KeyStatusMap>("/v1/settings/llm/status", { signal });
}

/** `GET /v1/settings/llm/credentials` */
export async function getDiscoveredCredentials(
  signal?: AbortSignal,
): Promise<DiscoveredCredentialInfo[]> {
  return await apiFetch<DiscoveredCredentialInfo[]>(
    "/v1/settings/llm/credentials",
    { signal },
  );
}

/** `POST /v1/settings/llm/credentials/rescan` */
export async function rescanCredentials(): Promise<DiscoveredCredentialInfo[]> {
  return await apiFetch<DiscoveredCredentialInfo[]>(
    "/v1/settings/llm/credentials/rescan",
    {
      method: "POST",
    },
  );
}

/** `GET /v1/settings/llm/cli-backends` */
export async function getCliBackends(
  signal?: AbortSignal,
): Promise<CliBackendStatus[]> {
  return await apiFetch<CliBackendStatus[]>("/v1/settings/llm/cli-backends", {
    signal,
  });
}

/** `GET /v1/settings/llm/providers/usage` — `health` is hardcoded, `total_tokens` is lifetime. */
export async function getProviderUsage(
  signal?: AbortSignal,
): Promise<ProviderUsageSummary[]> {
  return await apiFetch<ProviderUsageSummary[]>(
    "/v1/settings/llm/providers/usage",
    { signal },
  );
}

/** `GET /v1/models` — API-discovered models only. */
export async function listModels(signal?: AbortSignal): Promise<ModelEntry[]> {
  return await apiFetch<ModelEntry[]>("/v1/models", { signal });
}

/** `POST /v1/models/refresh` */
export async function refreshModels(): Promise<ModelEntry[]> {
  return await apiFetch<ModelEntry[]>("/v1/models/refresh", { method: "POST" });
}
