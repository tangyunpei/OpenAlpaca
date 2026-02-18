/**
 * Reactive agent store — combines REST fetches with real-time WebSocket updates.
 */

import { writable, derived, type Readable } from "svelte/store";
import { events, type ServerEvent } from "../daemon";
import {
  getAgents,
  getAgent,
  getAgentConfig,
  updateAgentConfig,
  createAgent,
  createAgentFromToml,
  deleteAgent,
} from "../api/agents";
import type { Agent, AgentDetailResponse, AgentConfigResponse, AgentConfigFile } from "../types";

/** Internal Map store for O(1) lookup by agent_id */
const agentMap = writable<Map<string, Agent>>(new Map());

/** Sorted agent list (alphabetical by name) */
export const agentList: Readable<Agent[]> = derived(agentMap, ($map) =>
  Array.from($map.values()).sort((a, b) => a.name.localeCompare(b.name)),
);

/** Currently selected agent detail (fetched on demand) */
export const selectedAgentDetail = writable<AgentDetailResponse | null>(null);

/** Currently selected agent config (for config editor) */
export const selectedAgentConfig = writable<AgentConfigResponse | null>(null);

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

/** Fetch agent config + version for editing */
export async function loadAgentConfig(id: string): Promise<void> {
  try {
    const config = await getAgentConfig(id);
    selectedAgentConfig.set(config);
  } catch (e) {
    console.error(`[agents-store] Failed to load agent config ${id}:`, e);
  }
}

/** Save agent config with optimistic locking.
 *  Throws "CONFIG_CONFLICT" on 409. */
export async function saveAgentConfig(
  id: string,
  config: AgentConfigFile,
  version: number,
): Promise<number> {
  const result = await updateAgentConfig(id, { config, config_version: version });
  // Reload into store
  await loadAgentConfig(id);
  await loadAgents();
  return result.config_version;
}

/** Create a new agent from config */
export async function createNewAgent(
  config: AgentConfigFile,
): Promise<string> {
  const result = await createAgent({ config });
  await loadAgents();
  return result.agent_id;
}

/** Create a new agent from raw TOML */
export async function createAgentFromTomlContent(toml: string): Promise<string> {
  const result = await createAgentFromToml({ toml_content: toml });
  await loadAgents();
  return result.agent_id;
}

/** Delete (archive) an agent */
export async function removeAgent(id: string): Promise<void> {
  await deleteAgent(id);
  agentMap.update((map) => {
    map.delete(id);
    return new Map(map);
  });
  selectedAgentDetail.set(null);
  selectedAgentConfig.set(null);
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
          ...(latest.template_id ? { template_id: latest.template_id } : {}),
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
          template_id: latest.template_id || undefined,
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

/** Subscribe to agent_config_changed events.
 *  On change, auto-reload agents list.
 *  Returns an unsubscribe function. */
export function subscribeToAgentConfigEvents(): () => void {
  return events.subscribe(($events) => {
    if ($events.length === 0) return;
    const latest = $events[0] as ServerEvent;
    if (latest.type !== "agent_config_changed") return;

    // Reload the agents list
    loadAgents();
  });
}
