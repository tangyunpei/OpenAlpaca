/**
 * `GET /v1/tools` — the tool catalog (GAP-18's tool half, ADR-030 §8).
 *
 * Read-only by construction: there is no `PUT` and no per-tool toggle (S1).
 * Availability is *derived* — (the agent's capabilities) ∩ (its extension being
 * enabled) — never asserted per tool, which is why a row carries `origin` and
 * no `enabled` of its own.
 */

import { apiFetch } from "../http";
import type { ToolCatalogEntry } from "./types";

/** Bare array, sorted by name. */
export async function listTools(
  signal?: AbortSignal,
): Promise<ToolCatalogEntry[]> {
  return await apiFetch<ToolCatalogEntry[]>("/v1/tools", { signal });
}
