/**
 * Reactive instance store — combines REST fetches with real-time WebSocket updates.
 *
 * Instances are ephemeral runtime objects. The store tracks them via:
 * 1. Initial REST fetch from GET /v1/agent-instances
 * 2. Real-time WebSocket agent_status events for spawned/busy/idle/destroyed transitions
 */

import { writable, derived, type Readable } from "svelte/store";
import { events, type ServerEvent } from "../daemon";
import { getInstances } from "../api/instances";
import type { AgentInstance } from "../types";

/** Internal Map store for O(1) lookup by instance_id */
const instanceMap = writable<Map<string, AgentInstance>>(new Map());

/** Sorted instance list (alphabetical by name) */
export const instanceList: Readable<AgentInstance[]> = derived(instanceMap, ($map) =>
  Array.from($map.values()).sort((a, b) => a.name.localeCompare(b.name)),
);

/** Instances grouped by template_id */
export const instancesByTemplate: Readable<Map<string, AgentInstance[]>> = derived(
  instanceMap,
  ($map) => {
    const grouped = new Map<string, AgentInstance[]>();
    for (const inst of $map.values()) {
      const list = grouped.get(inst.template_id) || [];
      list.push(inst);
      grouped.set(inst.template_id, list);
    }
    return grouped;
  },
);

/** Loading state */
export const instancesLoading = writable(false);

/** Fetch all active instances from REST and populate the map */
export async function loadInstances(): Promise<void> {
  instancesLoading.set(true);
  try {
    const instances = await getInstances();
    instanceMap.set(new Map(instances.map((i) => [i.id, i])));
  } catch (e) {
    console.error("[instances-store] Failed to load instances:", e);
  } finally {
    instancesLoading.set(false);
  }
}

/**
 * Subscribe to WebSocket agent_status events and update instance map.
 *
 * - "spawned" / "busy" / "idle" / "waiting" / "error" → upsert instance
 * - "destroyed" → remove instance from map
 *
 * Returns an unsubscribe function.
 */
export function subscribeToInstanceEvents(): () => void {
  return events.subscribe(($events) => {
    if ($events.length === 0) return;
    const latest = $events[0] as ServerEvent;
    if (latest.type !== "agent_status") return;

    // Use agent_instance_id + template_id from the WebSocket event
    const instanceId = latest.agent_instance_id;
    const templateId = latest.template_id;

    // Skip events without instance identity (shouldn't happen, but guard)
    if (!instanceId || !templateId) return;

    instanceMap.update((map) => {
      if (latest.status === "destroyed") {
        // Remove destroyed instance
        map.delete(instanceId);
      } else {
        // Upsert instance
        const existing = map.get(instanceId);
        if (existing) {
          map.set(instanceId, {
            ...existing,
            status: latest.status,
            current_task: latest.current_task_id,
          });
        } else {
          // Create new instance entry
          map.set(instanceId, {
            id: instanceId,
            template_id: templateId,
            name: latest.name || templateId,
            status: latest.status,
            current_task: latest.current_task_id,
          });
        }
      }
      return new Map(map);
    });
  });
}
