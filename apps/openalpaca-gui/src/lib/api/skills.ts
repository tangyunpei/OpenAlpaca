/**
 * The two skill routes.
 *
 * `GET /v1/skills` is the catalog — id, name, description, triggers, schedule
 * and provenance. `GET /v1/skills/health` is the metrics, keyed by the same
 * `skill_id`. The catalog is read-only by construction: there is no per-skill
 * enable state, so a row carries `origin` (the extension serving it, or `null`
 * for a file skill) and nothing that could be toggled.
 */

import { apiFetch } from "../http";
import type { SkillCatalogEntry, SkillHealthMetrics } from "./types";

/** `GET /v1/skills` — bare array, sorted by id. */
export async function listSkills(
  signal?: AbortSignal,
): Promise<SkillCatalogEntry[]> {
  return await apiFetch<SkillCatalogEntry[]>("/v1/skills", { signal });
}

/** `GET /v1/skills/health` — bare array keyed by `skill_id`. */
export async function getSkillHealth(
  signal?: AbortSignal,
): Promise<SkillHealthMetrics[]> {
  return await apiFetch<SkillHealthMetrics[]>("/v1/skills/health", { signal });
}
