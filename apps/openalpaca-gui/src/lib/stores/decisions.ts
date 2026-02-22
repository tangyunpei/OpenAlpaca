/**
 * Dispatch Decisions store — reactive state for dispatch decision history.
 */

import { writable } from "svelte/store";
import { getDispatchDecisions } from "../api/decisions";
import type { DispatchDecisionRecord } from "../types";

/** Recent dispatch decision records */
export const decisionRecords = writable<DispatchDecisionRecord[]>([]);

/** Loading state */
export const decisionsLoading = writable(false);

/** Error state */
export const decisionsError = writable<string | null>(null);

/** Load dispatch decision history */
export async function loadDecisions(
  mode?: string,
  limit?: number,
): Promise<void> {
  decisionsLoading.set(true);
  decisionsError.set(null);
  try {
    const records = await getDispatchDecisions(
      mode,
      undefined,
      undefined,
      limit ?? 100,
    );
    decisionRecords.set(records);
  } catch (e) {
    console.error("[decisions-store] Failed to load decisions:", e);
    decisionsError.set(e instanceof Error ? e.message : String(e));
  } finally {
    decisionsLoading.set(false);
  }
}
