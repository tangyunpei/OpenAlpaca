/**
 * `/v1/skills/health` — the only skill route that exists.
 *
 * The Settings → Skills rows the design draws are really *tools*, and neither
 * the tool registry nor the skill catalog is exposed over HTTP (GAP-18). This
 * module serves health metrics only; the catalog side goes through
 * `unbacked.ts`.
 */

import { apiFetch } from "../http";
import type { SkillHealthMetrics } from "./types";

/** `GET /v1/skills/health` — bare array keyed by `skill_id`. */
export async function getSkillHealth(
  signal?: AbortSignal,
): Promise<SkillHealthMetrics[]> {
  return await apiFetch<SkillHealthMetrics[]>("/v1/skills/health", { signal });
}
