/**
 * Settings → Skills.
 *
 * `GET /v1/skills/health` is the only skill route that exists, and it returns
 * metrics keyed by `skill_id` with no name, description, `asks` badge, or
 * enabled flag. The rows the design draws are really *tools*, and no tool
 * listing is exposed at all — so the catalog side is `Unavailable` (GAP-18)
 * and the health side is real.
 */

import { useQuery, type UseQueryResult } from "@tanstack/react-query";

import { getSkillHealth } from "@/lib/api/skills";
import { listToolCatalog, type ToolCatalogEntry } from "@/lib/api/unbacked";
import type { SkillHealthMetrics } from "@/lib/api/types";
import { qk } from "@/lib/query-keys";
import type { Availability } from "@/lib/unavailable";

export function useSkillHealth(): UseQueryResult<SkillHealthMetrics[]> {
  return useQuery({
    queryKey: qk.skills.health(),
    queryFn: ({ signal }) => getSkillHealth(signal),
  });
}

/** GAP-18 — no `/v1/tools`, no `/v1/skills` catalog. */
export function useToolCatalog(): Availability<ToolCatalogEntry[]> {
  return listToolCatalog();
}
