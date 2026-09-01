/**
 * Settings → Agents (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Real: the template list (`GET /v1/agent-templates`) and, separately, which
 * instances are running right now (`GET /v1/agent-instances`) — the design's
 * rows are templates, so the running count is shown as meta rather than
 * conflated with them.
 *
 * Unavailable: `12 runs 7d` and the per-template on/off switch. `AgentMetrics`
 * is lifetime, keyed by instance, and only returned by `GET /v1/agents/{id}`;
 * there is no enabled flag at all (GAP-20).
 */

import { Button, Tag, chipVariant } from "@/components/ui";
import {
  TEMPLATE_METRICS_NOTE,
  useAgentInstances,
  useAgentTemplates,
} from "@/hooks/useAgents";

import { GapNote, ListCard, ListRow, ListState, Toggle } from "./primitives";

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
                meta={running === 0 ? undefined : `${running} running`}
                control={
                  <Toggle
                    checked
                    label={`Enable ${template.name}`}
                    disabled
                    disabledReason={TEMPLATE_METRICS_NOTE}
                  />
                }
              />
            );
          })}
        </ListState>
      </ListCard>

      <GapNote>{TEMPLATE_METRICS_NOTE}.</GapNote>
    </>
  );
}
