/**
 * Reactive template store — fetches agent templates from REST.
 */

import { writable, derived, type Readable } from "svelte/store";
import { getTemplates } from "../api/templates";
import type { AgentTemplate } from "../types";

/** Internal Map store for O(1) lookup by template_id */
const templateMap = writable<Map<string, AgentTemplate>>(new Map());

/** Sorted template list (alphabetical by name) */
export const templateList: Readable<AgentTemplate[]> = derived(templateMap, ($map) =>
  Array.from($map.values()).sort((a, b) => a.name.localeCompare(b.name)),
);

/** Loading state */
export const templatesLoading = writable(false);

/** Fetch all templates from REST and populate the map */
export async function loadTemplates(): Promise<void> {
  templatesLoading.set(true);
  try {
    const templates = await getTemplates();
    templateMap.set(new Map(templates.map((t) => [t.id, t])));
  } catch (e) {
    console.error("[templates-store] Failed to load templates:", e);
  } finally {
    templatesLoading.set(false);
  }
}

/** Get a template from the store by id */
export function getTemplateFromStore(id: string): AgentTemplate | undefined {
  let result: AgentTemplate | undefined;
  templateMap.subscribe(($map) => {
    result = $map.get(id);
  })();
  return result;
}
