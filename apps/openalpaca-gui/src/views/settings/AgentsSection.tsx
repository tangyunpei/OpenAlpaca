/**
 * Settings → Agents (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Real: the template list (`GET /v1/agent-templates`), its per-template run
 * count and last-run stamp (GAP-20's counts half, from `subagent_span`), and,
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

import { shortDate } from "./format";
import { GapNote, ListCard, ListRow, ListState, Toggle } from "./primitives";

/**
 * `1 running · 12 runs · last 4 Sep`. A template nothing has ever spawned says
 * so in words: `0 runs` reads like a metric that failed to load.
 */
function templateMeta(template: AgentTemplate, running: number): string {
  const parts: string[] = [];
  if (running > 0) parts.push(`${running} running`);
  if (template.run_count === 0) {
    parts.push("No runs yet");
  } else {
    const last =
      template.last_run_at === undefined
        ? ""
        : ` · last ${shortDate(template.last_run_at)}`;
    parts.push(`${template.run_count} runs${last}`);
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
