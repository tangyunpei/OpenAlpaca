/**
 * Reactive settings store — combines REST fetches with real-time WebSocket updates.
 */

import { writable, derived, type Readable } from "svelte/store";
import { events, type ServerEvent } from "../daemon";
import { getLlmSettings } from "../api/settings";
import type { LlmSettingsResponse, ProviderInfo } from "../types";

/** Current LLM settings */
export const llmSettings = writable<LlmSettingsResponse | null>(null);

/** Loading state */
export const settingsLoading = writable(false);

/** Error state */
export const settingsError = writable<string | null>(null);

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
