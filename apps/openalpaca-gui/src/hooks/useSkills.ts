/**
 * Settings → Tools.
 *
 * `GET /v1/tools` is the tool catalog (ADR-030 §8) — name, description,
 * `origin`, `requires_confirmation`, `invocations_today`. There is no per-tool
 * enable state to read, because none exists (S1).
 *
 * `GET /v1/skills` is the skill catalog and `GET /v1/skills/health` the
 * lifetime metrics; the two join on the skill id, which is how a health row
 * gets a name to show instead of the id the log keys on.
 */

import { useQuery, type UseQueryResult } from "@tanstack/react-query";

import { getSkillHealth, listSkills } from "@/lib/api/skills";
import { listTools } from "@/lib/api/tools";
import type {
  SkillCatalogEntry,
  SkillHealthMetrics,
  ToolCatalogEntry,
} from "@/lib/api/types";
import { qk } from "@/lib/query-keys";

export function useSkillHealth(): UseQueryResult<SkillHealthMetrics[]> {
  return useQuery({
    queryKey: qk.skills.health(),
    queryFn: ({ signal }) => getSkillHealth(signal),
  });
}

export function useSkillCatalog(): UseQueryResult<SkillCatalogEntry[]> {
  return useQuery({
    queryKey: qk.skills.catalog(),
    queryFn: ({ signal }) => listSkills(signal),
  });
}

export function useTools(): UseQueryResult<ToolCatalogEntry[]> {
  return useQuery({
    queryKey: qk.tools.list(),
    queryFn: ({ signal }) => listTools(signal),
  });
}
