/**
 * Reactive agent store — combines REST fetches with real-time WebSocket updates.
 */

import { writable, derived, type Readable } from "svelte/store";
import { events, type ServerEvent } from "../daemon";
import { getAgents, getAgent } from "../api/agents";
import type { Agent, AgentDetailResponse } from "../types";

/** Internal Map store for O(1) lookup by agent_id */
const agentMap = writable<Map<string, Agent>>(new Map());

/** Sorted agent list (alphabetical by name) */
export const agentList: Readable<Agent[]> = derived(agentMap, ($map) =>
  Array.from($map.values()).sort((a, b) => a.name.localeCompare(b.name)),
);

/** Currently selected agent detail (fetched on demand) */
export const selectedAgentDetail = writable<AgentDetailResponse | null>(null);

/** Loading state */
export const agentsLoading = writable(false);

/** Fetch all agents from REST and populate the map */
export async function loadAgents(): Promise<void> {
  agentsLoading.set(true);
  try {
    const agents = await getAgents();
    agentMap.set(new Map(agents.map((a) => [a.id, a])));
  } catch (e) {
    console.error("[agents-store] Failed to load agents:", e);
  } finally {
    agentsLoading.set(false);
  }
}

/** Fetch full agent detail + metrics */
export async function loadAgentDetail(id: string): Promise<void> {
  try {
    const detail = await getAgent(id);
    selectedAgentDetail.set(detail);
    // Also update the agent in the map
    agentMap.update((map) => {
      map.set(detail.agent.id, detail.agent);
      return new Map(map);
    });
  } catch (e) {
    console.error(`[agents-store] Failed to load agent detail ${id}:`, e);
  }
}

/** Subscribe to WebSocket events and merge agent_status into the map.
 *  Returns an unsubscribe function. */
export function subscribeToAgentEvents(): () => void {
  return events.subscribe(($events) => {
    if ($events.length === 0) return;
    const latest = $events[0] as ServerEvent;
    if (latest.type !== "agent_status") return;

    agentMap.update((map) => {
      const existing = map.get(latest.agent_id);
      if (existing) {
        // Merge — skip overwriting name with empty string
        map.set(latest.agent_id, {
          ...existing,
          status: latest.status,
          current_task_id: latest.current_task_id,
          ...(latest.name ? { name: latest.name } : {}),
        });
      } else {
        // Create placeholder for unknown agent
        map.set(latest.agent_id, {
          id: latest.agent_id,
          name: latest.name || "Unknown Agent",
          description: null,
          icon: null,
          status: latest.status,
          current_task_id: latest.current_task_id,
          skills_json: "[]",
          preset_json: "{}",
          constraints_json: null,
          llm_config_json: null,
          persona: null,
          created_at: latest.ts,
          updated_at: latest.ts,
        });
      }
      return new Map(map);
    });
  });
}
