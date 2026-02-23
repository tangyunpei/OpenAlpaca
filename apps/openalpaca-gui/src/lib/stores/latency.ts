/**
 * Orchestrator Latency store — reactive state for latency records and aggregates.
 */

import { writable } from "svelte/store";
import { getLatencyRecords, getLatencyAggregates } from "../api/latency";
import type { OrchestratorLatencyRecord, LatencyAggregate } from "../types";

/** Recent latency records */
export const latencyRecords = writable<OrchestratorLatencyRecord[]>([]);

/** Aggregated latency stats by mode */
export const latencyAggregates = writable<LatencyAggregate[]>([]);

/** Loading states */
export const latencyRecordsLoading = writable(false);
export const latencyAggregatesLoading = writable(false);

/** Error state */
export const latencyError = writable<string | null>(null);

/** Load recent latency records */
export async function loadLatencyRecords(
  mode?: string,
  limit?: number,
): Promise<void> {
  latencyRecordsLoading.set(true);
  latencyError.set(null);
  try {
    const records = await getLatencyRecords(
      mode,
      undefined,
      undefined,
      limit ?? 100,
    );
    latencyRecords.set(records);
  } catch (e) {
    console.error("[latency-store] Failed to load records:", e);
    latencyError.set(e instanceof Error ? e.message : String(e));
  } finally {
    latencyRecordsLoading.set(false);
  }
}

/** Load aggregated latency stats */
export async function loadLatencyAggregates(
  from?: string,
  to?: string,
): Promise<void> {
  latencyAggregatesLoading.set(true);
  latencyError.set(null);
  try {
    const aggregates = await getLatencyAggregates(from, to);
    latencyAggregates.set(aggregates);
  } catch (e) {
    console.error("[latency-store] Failed to load aggregates:", e);
    latencyError.set(e instanceof Error ? e.message : String(e));
  } finally {
    latencyAggregatesLoading.set(false);
  }
}
