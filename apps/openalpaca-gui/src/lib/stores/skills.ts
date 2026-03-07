/**
 * Store for skill health metrics.
 */

import { writable } from "svelte/store";
import { getSkillHealth } from "../api/skills";
import type { SkillHealthMetrics } from "../types";

export const skillHealthList = writable<SkillHealthMetrics[]>([]);
export const skillsLoading = writable(false);

let loadInFlight = false;

export async function loadSkillHealth(): Promise<void> {
  if (loadInFlight) return;
  loadInFlight = true;
  skillsLoading.set(true);
  try {
    const metrics = await getSkillHealth();
    skillHealthList.set(metrics);
  } catch (e) {
    console.error("Failed to load skill health:", e);
  } finally {
    skillsLoading.set(false);
    loadInFlight = false;
  }
}
