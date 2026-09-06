/**
 * Settings → Tools (ADR-030 §9.3; replaces the misnamed Skills section).
 *
 * The design's "Skills" rows are in fact **tools** — `shell_execute`,
 * `file_edit`, `github__create_issue` — and `GET /v1/tools` now serves them:
 * name, description, provenance, whether the tool asks first, and how often it
 * ran today.
 *
 * Two deliberate, documented departures from `DESIGN_SPEC`, both of which draw
 * a control the settled model says must not exist:
 *
 *  * **A builtin row renders NO control at all** — not a disabled toggle, not
 *    a checked-and-disabled one. A greyed-out switch implies a switch exists
 *    and is merely unavailable; the truth is that builtins are governed by
 *    agent configuration, which is not a per-tool switch.
 *  * **An extension tool row renders a read-only provenance chip** that leads
 *    to its Extensions row. Availability is derived — (the agent's
 *    capabilities) ∩ (its extension being enabled) — never asserted per tool
 *    (S1). There is no per-tool toggle anywhere, and no route that would
 *    accept one.
 *
 * Skill health keeps its own subsection, fed by `GET /v1/skills/health` and
 * named from `GET /v1/skills` — the two join on the skill id. A health row for
 * a skill the catalog no longer holds (a deleted directory, a disabled
 * plugin's withdrawn contribution) keeps showing its id: that is the truth the
 * log has, and it is never replaced by a placeholder name.
 */

import { Eyebrow, Tag } from "@/components/ui";
import { useSkillCatalog, useSkillHealth, useTools } from "@/hooks/useSkills";
import type { ToolCatalogEntry } from "@/lib/api/types";
import { useUiStore } from "@/stores/ui";

import { GapNote, ListCard, ListRow, ListState } from "./primitives";
import { percent } from "./format";

/** "via MCP `github` — enabled" (§9.3). */
export function originLabel(entry: ToolCatalogEntry): string | null {
  if (entry.origin === null) return null;
  const kind = entry.origin.kind === "mcp" ? "MCP" : "plugin";
  return `via ${kind} ${entry.origin.id} — ${entry.origin.state}`;
}

export function ToolsSection() {
  const tools = useTools();
  const health = useSkillHealth();
  const catalog = useSkillCatalog();
  const setSection = useUiStore((s) => s.setSettingsSection);

  const toolRows = tools.data ?? [];
  const healthRows = health.data ?? [];
  const namesById = new Map(
    (catalog.data ?? []).map((skill) => [skill.id, skill.name]),
  );

  return (
    <>
      <ListCard>
        <ListState
          pending={tools.isPending}
          error={tools.error}
          empty={toolRows.length === 0}
          emptyCopy="No tools are registered."
        >
          {toolRows.map((entry) => {
            const origin = originLabel(entry);
            return (
              <ListRow
                key={entry.name}
                name={entry.name}
                tags={
                  entry.requires_confirmation ? (
                    <Tag value="asks" tone="asks" />
                  ) : undefined
                }
                description={entry.description}
                chips={
                  origin === null ? undefined : (
                    <button
                      type="button"
                      onClick={() => setSection("extensions")}
                      className="cursor-pointer rounded-sm border border-line bg-muted px-[6px] py-[2px] font-mono text-2xs tracking-label text-secondary uppercase hover:text-ink"
                    >
                      {origin}
                    </button>
                  )
                }
                meta={`${entry.invocations_today} today`}
              />
            );
          })}
        </ListState>
      </ListCard>

      <GapNote>
        Tools have no individual on/off switch: a tool is available when an
        agent's capabilities allow it and the extension serving it is enabled.
      </GapNote>

      <Eyebrow tracking="narrow" tone="faint" className="mt-[22px] mb-[8px]">
        Skill health
      </Eyebrow>
      <ListCard>
        <ListState
          pending={health.isPending}
          error={health.error}
          empty={healthRows.length === 0}
          emptyCopy="No skill has been invoked yet."
        >
          {healthRows.map((skill) => {
            const name = namesById.get(skill.skill_id);
            const metrics = `${skill.total_invocations} invocations · ${percent(
              skill.clean_success_rate,
            )} clean · ${percent(skill.repair_rate)} repaired`;
            return (
              <ListRow
                key={skill.skill_id}
                name={name ?? skill.skill_id}
                // The id stays visible when the name replaces it in the title:
                // it is what the log keys on and what `/slash` resolves.
                description={
                  name === undefined
                    ? metrics
                    : `${skill.skill_id} · ${metrics}`
                }
                meta={`${Math.round(skill.avg_duration_ms)} ms avg`}
              />
            );
          })}
        </ListState>
      </ListCard>

      <GapNote>
        Skill health metrics are lifetime totals; the daemon serves no per-day
        breakdown of them.
      </GapNote>
    </>
  );
}
