/**
 * Reactive plugin store — combines REST fetches with real-time WebSocket updates.
 */

import { writable, derived, type Readable } from "svelte/store";
import { events, type ServerEvent } from "../daemon";
import { getPlugins } from "../api/plugins";
import type { PluginInfo } from "../api/plugins";

/** Internal Map store for O(1) lookup by plugin name */
const pluginMap = writable<Map<string, PluginInfo>>(new Map());

/** Sorted plugin list (alphabetical by name) */
export const pluginList: Readable<PluginInfo[]> = derived(pluginMap, ($map) =>
  Array.from($map.values()).sort((a, b) => a.name.localeCompare(b.name)),
);

/** Loading state */
export const pluginsLoading = writable(false);

/** Shared in-flight guard for loadPlugins — prevents concurrent REST fetches. */
let loadInFlight = false;
let loadPending = false;

/** Fetch all plugins from REST and populate the map. */
export async function loadPlugins(): Promise<void> {
  if (loadInFlight) {
    loadPending = true;
    return;
  }
  loadInFlight = true;
  pluginsLoading.set(true);
  try {
    const plugins = await getPlugins();
    pluginMap.set(new Map(plugins.map((p) => [p.name, p] as [string, PluginInfo])));
  } catch (e) {
    console.error("[plugins-store] Failed to load plugins:", e);
  } finally {
    pluginsLoading.set(false);
    loadInFlight = false;
    if (loadPending) {
      loadPending = false;
      loadPlugins();
    }
  }
}

/** Subscribe to WebSocket plugin events and refresh the list.
 *  Returns an unsubscribe function. */
export function subscribeToPluginEvents(): () => void {
  return events.subscribe(($events) => {
    if ($events.length === 0) return;
    const latest = $events[0] as ServerEvent;
    if (
      latest.type !== "plugin_loaded" &&
      latest.type !== "plugin_unloaded" &&
      latest.type !== "plugin_crashed" &&
      latest.type !== "plugin_disabled" &&
      latest.type !== "plugin_pending_approval" &&
      latest.type !== "plugin_needs_config"
    ) {
      return;
    }
    // Any plugin event triggers a full refresh to stay in sync
    loadPlugins();
  });
}
