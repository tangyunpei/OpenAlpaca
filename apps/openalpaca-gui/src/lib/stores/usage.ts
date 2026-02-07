/**
 * LLM Usage store — reactive state for usage logs and daily aggregates.
 */

import { writable } from "svelte/store";
import { getLlmUsage, getLlmUsageDaily } from "../api/usage";
import type { LlmCallLog, LlmUsageDaily } from "../types";

/** Recent LLM call logs */
export const usageLogs = writable<LlmCallLog[]>([]);

/** Daily usage aggregates */
export const usageDaily = writable<LlmUsageDaily[]>([]);

/** Loading states */
export const usageLogsLoading = writable(false);
export const usageDailyLoading = writable(false);

/** Error states */
export const usageError = writable<string | null>(null);

/** Load recent usage logs */
export async function loadUsageLogs(
  agentId?: string,
  keyId?: string,
  limit?: number,
): Promise<void> {
  usageLogsLoading.set(true);
  usageError.set(null);
  try {
    const logs = await getLlmUsage(agentId, keyId, limit ?? 50);
    usageLogs.set(logs);
  } catch (e) {
    console.error("[usage-store] Failed to load logs:", e);
    usageError.set(e instanceof Error ? e.message : String(e));
  } finally {
    usageLogsLoading.set(false);
  }
}

/** Load daily usage aggregates */
export async function loadUsageDaily(
  agentId?: string,
  date?: string,
  limit?: number,
): Promise<void> {
  usageDailyLoading.set(true);
  usageError.set(null);
  try {
    const daily = await getLlmUsageDaily(agentId, date, limit ?? 30);
    usageDaily.set(daily);
  } catch (e) {
    console.error("[usage-store] Failed to load daily usage:", e);
    usageError.set(e instanceof Error ? e.message : String(e));
  } finally {
    usageDailyLoading.set(false);
  }
}
