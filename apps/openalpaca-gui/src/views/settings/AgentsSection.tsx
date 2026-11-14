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
 * A template a file in `config/agents` switches off (`disabled: true`,
 * subagent design §2.4 L-4a) is listed with its file and a "Switched off"
 * chip, its toggle off.
 *
 * Where each template comes from (subagent design §2.7 R-4): one chip per
 * row — "Built in", "Yours", "Yours · replaces built-in", "Plugin · <id>",
 * "Withheld" (a file holds the id and cannot be used, named with its error)
 * or "Switched off" — and a line for every definition of the id that does
 * not run, from the row's `not_used`, and for each of its `warnings`.
 *
 * Unavailable: the toggle as a control. The switch is the person's file and
 * no route writes it (GAP-20's remaining half).
 */

import { Button, Tag, chipVariant } from "@/components/ui";
import {
  TEMPLATE_TOGGLE_NOTE,
  useAgentInstances,
  useAgentTemplates,
} from "@/hooks/useAgents";
import type { AgentTemplate, NotUsedEntry } from "@/lib/api/types";

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

/**
 * The row's one source chip (R-4), or none for a daemon older than
 * `source`. A file that switches the id off or withholds it says so over
 * where the id would come from: nothing of the id runs.
 */
function sourceChip(
  template: AgentTemplate,
): { label: string; tone: "warn" | "neutral" } | undefined {
  if (template.state === "disabled")
    return { label: "Switched off", tone: "neutral" };
  if (template.state === "withheld") return { label: "Withheld", tone: "warn" };
  switch (template.source) {
    case "bundled":
      return { label: "Built in", tone: "neutral" };
    case "file":
      return {
        label: template.overrides_bundled
          ? "Yours · replaces built-in"
          : "Yours",
        tone: "neutral",
      };
    case "plugin":
      return {
        label: template.plugin ? `Plugin · ${template.plugin}` : "Plugin",
        tone: "neutral",
      };
    default:
      return undefined;
  }
}

/** What a definition of the id that does not run is, in a sentence. */
function notUsedLine(entry: NotUsedEntry): string {
  const at = entry.path ? `, ${entry.path},` : "";
  switch (entry.reason) {
    case "unedited_copy":
      if (entry.edit === "manual_step_7" || entry.edit === "manual_step_8") {
        const step = entry.edit === "manual_step_7" ? 7 : 8;
        return `An earlier build's copy as the Installation Manual's step ${step} left it${at} is not used`;
      }
      return entry.version === "current"
        ? `An unedited copy of the built-in template${at} is not used`
        : `An earlier build's unedited copy${at} is not used`;
    case "overridden":
      return "The built-in template is not used";
    case "duplicate":
      return `${entry.path ?? "Another file"} also holds this id and is not used`;
    case "does_not_parse":
      return `${entry.path ?? "A file"} holds this id, does not parse and is not used`;
    case "plugin_refused":
      return `The template the plugin ${entry.plugin ?? "?"} offers for this id is not used`;
    default:
      return `${entry.path ?? entry.source} is not used (${String(entry.reason)})`;
  }
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
            const switchedOff = template.state === "disabled";
            const chip = sourceChip(template);
            const said =
              switchedOff && template.path
                ? `Switched off by ${template.path}`
                : template.state === "withheld" && template.path
                  ? `Withheld by ${template.path}`
                  : template.description;
            const warnings = template.warnings ?? [];
            const notUsed = template.not_used ?? [];
            return (
              <ListRow
                key={template.id}
                name={template.name}
                tags={
                  chip || template.singleton ? (
                    <>
                      {template.singleton && <Tag value="singleton" />}
                      {chip && <Tag value={chip.label} tone={chip.tone} />}
                    </>
                  ) : undefined
                }
                description={
                  warnings.length === 0 && notUsed.length === 0 ? (
                    said
                  ) : (
                    <>
                      <span className="block">{said}</span>
                      {warnings.map((warning) => (
                        <span
                          key={warning}
                          className="block text-sm wrap-anywhere text-amber-ink"
                        >
                          {warning}
                        </span>
                      ))}
                      {notUsed.map((entry, i) => (
                        <span
                          key={`${entry.reason}:${entry.path ?? entry.plugin ?? i}`}
                          className="block text-sm wrap-anywhere text-faint"
                        >
                          {notUsedLine(entry)}
                        </span>
                      ))}
                    </>
                  )
                }
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
                    checked={!switchedOff}
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
