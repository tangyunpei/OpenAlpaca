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
 * named from `GET /v1/skills`. The join is not a plain id match:
 * `skill_execution_log.skill_id` is **not** the catalog id — every invocation
 * path resolves the entry and then logs `frontmatter.name` (`/slash` and
 * router selection through `Intent::SkillInvocation`, and the model's
 * `invoke_skill`) — so a health row is resolved by the daemon's rule, which
 * `resolve_skill_key` owns and `skillsByLogKey` below mirrors: lowercased, id
 * first, then frontmatter name.
 *
 * A health row for a skill the catalog no longer holds (a deleted directory, a
 * disabled plugin's withdrawn contribution) keeps showing the id it has: that
 * is the truth the log has, and it is never replaced by a placeholder name.
 */

import { Eyebrow, Tag } from "@/components/ui";
import { useSkillCatalog, useSkillHealth, useTools } from "@/hooks/useSkills";
import type { SkillCatalogEntry, ToolCatalogEntry } from "@/lib/api/types";
import { useUiStore } from "@/stores/ui";

import { GapNote, ListCard, ListRow, ListState } from "./primitives";
import { percent } from "./format";

/**
 * The daemon's rule, mirrored on the client — it cannot be imported, so it is
 * pinned by test instead. The rule and the record of *why* the column needs one
 * live in `resolve_skill_key`
 * (`crates/openalpaca_storage/src/repository/skill_execution/mod.rs`), which
 * both `SkillCatalog::get` and `GET /v1/skills` use:
 *
 *  * lowercase both sides;
 *  * an **id** hit wins outright over any entry's frontmatter name;
 *  * a frontmatter name shared by two entries resolves to the **lowest id**, so
 *    two reads of an unchanged catalog agree.
 *
 * Ids are therefore inserted last (they overwrite), names first and only where
 * the key is still free, walking the catalog in id order.
 */
export function skillsByLogKey(
  catalog: SkillCatalogEntry[],
): Map<string, SkillCatalogEntry> {
  // Code-unit order, not `localeCompare` — the daemon compares `&str` bytes,
  // and a locale collation would disagree with it on `-` versus `_`.
  const byId = [...catalog].sort((a, b) =>
    a.id < b.id ? -1 : a.id > b.id ? 1 : 0,
  );
  const byKey = new Map<string, SkillCatalogEntry>();
  for (const skill of byId) {
    const name = skill.name.toLowerCase();
    if (!byKey.has(name)) byKey.set(name, skill);
  }
  for (const skill of byId) byKey.set(skill.id.toLowerCase(), skill);
  return byKey;
}

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
  const skillsByKey = skillsByLogKey(catalog.data ?? []);

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
          {healthRows.map((row) => {
            const skill = skillsByKey.get(row.skill_id.toLowerCase());
            const metrics = `${row.total_invocations} invocations · ${percent(
              row.clean_success_rate,
            )} clean · ${percent(row.repair_rate)} repaired`;
            return (
              <ListRow
                key={row.skill_id}
                name={skill?.name ?? row.skill_id}
                // The catalog id stays visible when the name takes the title:
                // it is what `/slash` resolves and what the catalog is keyed
                // by, whichever spelling the log happened to record.
                description={
                  skill === undefined ? metrics : `${skill.id} · ${metrics}`
                }
                meta={`${Math.round(row.avg_duration_ms)} ms avg`}
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
