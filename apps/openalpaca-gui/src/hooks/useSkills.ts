/**
 * Settings → Tools.
 *
 * `GET /v1/tools` is the tool catalog (ADR-030 §8) — name, description,
 * `origin`, `requires_confirmation`, `invocations_today`. There is no per-tool
 * enable state to read, because none exists (S1).
 *
 * `GET /v1/skills/health` is still the only *skill* route: metrics keyed by
 * `skill_id`, with no name and no description (GAP-18's remaining half), which
 * is why the health rows read as ids.
 */

import { useQuery, type UseQueryResult } from "@tanstack/react-query";

import { getSkillHealth } from "@/lib/api/skills";
import { listTools } from "@/lib/api/tools";
import type { SkillHealthMetrics, ToolCatalogEntry } from "@/lib/api/types";
import { qk } from "@/lib/query-keys";

export function useSkillHealth(): UseQueryResult<SkillHealthMetrics[]> {
  return useQuery({
    queryKey: qk.skills.health(),
    queryFn: ({ signal }) => getSkillHealth(signal),
  });
}

export function useTools(): UseQueryResult<ToolCatalogEntry[]> {
  return useQuery({
    queryKey: qk.tools.list(),
    queryFn: ({ signal }) => listTools(signal),
  });
}
