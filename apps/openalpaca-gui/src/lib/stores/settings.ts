/**
 * Reactive settings store — combines REST fetches with real-time WebSocket updates.
 */

import { writable, derived, type Readable } from "svelte/store";
import { events, type ServerEvent } from "../daemon";
import { getLlmSettings, getAvailableModels, refreshAvailableModels } from "../api/settings";
import { getOrchestratorConfig, updateOrchestratorConfig } from "../api/orchestrator";
import type { LlmSettingsResponse, ProviderInfo, OrchestratorConfigResponse, ModelEntry } from "../types";

/** Current LLM settings */
export const llmSettings = writable<LlmSettingsResponse | null>(null);

/** Loading state */
export const settingsLoading = writable(false);

/** Error state */
export const settingsError = writable<string | null>(null);

/** Orchestrator config */
export const orchestratorConfig = writable<OrchestratorConfigResponse | null>(null);

/** Available models from all providers */
export const availableModels = writable<ModelEntry[]>([]);

/** Models refreshing state */
export const modelsRefreshing = writable(false);

/** Derived: sorted list of [providerName, providerInfo] entries */
export const providerList: Readable<[string, ProviderInfo][]> = derived(
  llmSettings,
  ($settings) => {
    if (!$settings) return [];
    return Object.entries($settings.providers).sort(([a], [b]) =>
      a.localeCompare(b),
    );
  },
);

/** Fetch settings from REST API and populate the store */
export async function loadSettings(): Promise<void> {
  settingsLoading.set(true);
  settingsError.set(null);
  try {
    const settings = await getLlmSettings();
    llmSettings.set(settings);
  } catch (e) {
    console.error("[settings-store] Failed to load settings:", e);
    settingsError.set(e instanceof Error ? e.message : String(e));
  } finally {
    settingsLoading.set(false);
  }
}

/** Fetch orchestrator config */
export async function loadOrchestratorConfig(): Promise<void> {
  try {
    const config = await getOrchestratorConfig();
    orchestratorConfig.set(config);
  } catch (e) {
    console.error("[settings-store] Failed to load orchestrator config:", e);
  }
}

/** Save orchestrator config */
export async function saveOrchestratorConfig(
  model: string,
  fallbackModels: string[],
): Promise<void> {
  await updateOrchestratorConfig({ model, fallback_models: fallbackModels });
  await loadOrchestratorConfig();
}

/** Fetch available models from the API */
export async function loadAvailableModels(): Promise<void> {
  try {
    const models = await getAvailableModels();
    availableModels.set(models);
  } catch (e) {
    console.error("[settings-store] Failed to load available models:", e);
  }
}

/** Refresh models from provider APIs (re-queries each provider) */
export async function refreshModels(): Promise<void> {
  modelsRefreshing.set(true);
  try {
    const models = await refreshAvailableModels();
    availableModels.set(models);
  } catch (e) {
    console.error("[settings-store] Failed to refresh models:", e);
  } finally {
    modelsRefreshing.set(false);
  }
}

/** Subscribe to WebSocket events for key_status_changed.
 *  On change, refresh the full settings.
 *  Returns an unsubscribe function. */
export function subscribeToKeyEvents(): () => void {
  return events.subscribe(($events) => {
    if ($events.length === 0) return;
    const latest = $events[0] as ServerEvent;
    if (latest.type !== "key_status_changed") return;

    // Refresh settings when a key changes
    loadSettings();
  });
}

/** Subscribe to orchestrator_config_changed events.
 *  Returns an unsubscribe function. */
export function subscribeToOrchestratorEvents(): () => void {
  return events.subscribe(($events) => {
    if ($events.length === 0) return;
    const latest = $events[0] as ServerEvent;
    if (latest.type !== "orchestrator_config_changed") return;

    loadOrchestratorConfig();
  });
}
