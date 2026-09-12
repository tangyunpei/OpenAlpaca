/**
 * Settings → Agents (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Real: the template list (`GET /v1/agent-templates`), its per-template
 * *completed*-run count and the window it was counted over (GAP-20's counts
 * half, closed by T48: `?window=7d|30d|all` against `subagent_span`), and,
 * separately, which instances are running right now
 * (`GET /v1/agent-instances`) — the design's rows are templates, so the
 * running count is shown as meta rather than conflated with them.
 *
 * Unavailable: the per-template on/off switch. There is no enabled flag on a
 * template and nothing would enforce one in the spawn path (GAP-20's remaining
 * half).
 */

import { Button, Tag, chipVariant } from "@/components/ui";
import {
  TEMPLATE_TOGGLE_NOTE,
  useAgentInstances,
  useAgentTemplates,
} from "@/hooks/useAgents";
import type { AgentTemplate } from "@/lib/api/types";

import { GapNote, ListCard, ListRow, ListState, Toggle } from "./primitives";

/**
 * `1 running · 12 runs · 7d`. A template with no completed run in the window
 * says so in words: `0 runs` reads like a metric that failed to load.
 *
 * The falsy test on `run_count` is deliberate. It is typed as always present,
 * but `apiFetch` casts the response rather than validating it, so a daemon
 * older than the field serves a row without one — `=== 0` would let that row
 * through to render the literal string `undefined runs`. Absent and zero mean
 * the same thing here, and read the same. `window` gets the same treatment:
 * a pre-T48 daemon sends `run_count` with no `window` at all, and that count
 * was lifetime-and-in-flight, not this window's completed runs — so the label
 * is dropped rather than guessing `7d` for a number that was never scoped to
 * one.
 */
function templateMeta(template: AgentTemplate, running: number): string {
  const parts: string[] = [];
  if (running > 0) parts.push(`${running} running`);
  if (!template.run_count) {
    // A windowed zero only says "none in this window"; only an unwindowed or
    // all-time zero can honestly claim the template has never run.
    const scoped = template.window && template.window !== "all";
    parts.push(scoped ? `No runs · ${template.window}` : "No runs yet");
  } else {
    const noun = template.run_count === 1 ? "run" : "runs";
    const windowed = template.window
      ? `${template.run_count} ${noun} · ${template.window}`
      : `${template.run_count} ${noun}`;
    parts.push(windowed);
  }
  return parts.join(" · ");
}

export function AgentsSection() {
  const templates = useAgentTemplates();
  const instances = useAgentInstances();
  const rows = templates.data ?? [];

  const runningByTemplate = new Map<string, number>();
  for (const instance of instances.data ?? []) {
    runningByTemplate.set(
      instance.template_id,
      (runningByTemplate.get(instance.template_id) ?? 0) + 1,
    );
  }

  return (
    <>
      <ListCard>
        <ListState
          pending={templates.isPending}
          error={templates.error}
          empty={rows.length === 0}
          emptyCopy="No agent templates defined."
        >
          {rows.map((template) => {
            const running = runningByTemplate.get(template.id) ?? 0;
            return (
              <ListRow
                key={template.id}
                name={template.name}
                tags={
                  template.singleton ? <Tag value="singleton" /> : undefined
                }
                description={template.description}
                chips={
                  template.model === undefined ? undefined : (
                    <Button variant={chipVariant(false)} disabled>
                      {template.model}
                    </Button>
                  )
                }
                meta={templateMeta(template, running)}
                control={
                  <Toggle
                    checked
                    label={`Enable ${template.name}`}
                    disabled
                    disabledReason={TEMPLATE_TOGGLE_NOTE}
                  />
                }
              />
            );
          })}
        </ListState>
      </ListCard>

      <GapNote>{TEMPLATE_TOGGLE_NOTE}.</GapNote>
    </>
  );
}
