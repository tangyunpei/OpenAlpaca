/**
 * Settings → Skills (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * The design's rows are really *tools* — `shell_execute`, `file_edit`,
 * `web_fetch` — and nothing lists them: neither the tool registry nor the skill
 * catalog is exposed over HTTP (GAP-18). So the name, description, `asks` badge
 * and enabled switch the design draws have no source, and none of them are
 * faked here.
 *
 * What the daemon *does* serve is `GET /v1/skills/health`: per-skill invocation
 * counts and success rates, keyed by `skill_id`. Those are real rows and they
 * are what this section shows, with the catalog gap named above them.
 */

import { useSkillHealth, useToolCatalog } from "@/hooks/useSkills";
import { gapDetail } from "@/lib/unavailable";

import { GapNote, ListCard, ListRow, ListState } from "./primitives";
import { percent } from "./format";

export function SkillsSection() {
  const health = useSkillHealth();
  const catalog = useToolCatalog();
  const rows = health.data ?? [];

  return (
    <>
      <ListCard>
        <ListState
          pending={health.isPending}
          error={health.error}
          empty={rows.length === 0}
          emptyCopy="No skill has been invoked yet."
        >
          {rows.map((skill) => (
            <ListRow
              key={skill.skill_id}
              name={skill.skill_id}
              description={`${skill.total_invocations} invocations · ${percent(
                skill.clean_success_rate,
              )} clean · ${percent(skill.repair_rate)} repaired`}
              meta={`${Math.round(skill.avg_duration_ms)} ms avg`}
            />
          ))}
        </ListState>
      </ListCard>

      {!catalog.available && <GapNote>{gapDetail(catalog)}.</GapNote>}
      <GapNote>
        Invocation counts are lifetime totals; the daemon serves no daily
        breakdown.
      </GapNote>
    </>
  );
}
