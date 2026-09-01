/** `/v1/plugins*`. */

import { apiFetch } from "../http";
import type { PluginAction, PluginInfo } from "./types";

/** `GET /v1/plugins` — bare array. */
export async function listPlugins(signal?: AbortSignal): Promise<PluginInfo[]> {
  return await apiFetch<PluginInfo[]>("/v1/plugins", { signal });
}

/** `POST /v1/plugins/{name}/{approve|deny|enable|disable}` */
export async function performPluginAction(
  name: string,
  action: PluginAction,
): Promise<void> {
  await apiFetch<void>(`/v1/plugins/${encodeURIComponent(name)}/${action}`, {
    method: "POST",
  });
}

/** `POST /v1/plugins/{name}/config` */
export async function setPluginConfig(
  name: string,
  key: string,
  value: string,
): Promise<void> {
  await apiFetch<void>(`/v1/plugins/${encodeURIComponent(name)}/config`, {
    method: "POST",
    body: { key, value },
  });
}
