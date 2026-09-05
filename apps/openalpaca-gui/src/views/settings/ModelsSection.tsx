/**
 * Settings → Models & keys (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Real: the provider list and its keys (`GET /v1/settings/llm`), the model
 * catalogue (`GET /v1/models`), and picking a model — which writes
 * `PUT /v1/orchestrator/config`.
 *
 * Two honesty notes ride with that write. It is **daemon-wide**, not
 * per-conversation (GAP-13), and the per-provider token figure the design shows
 * as "today" is lifetime (`ProviderUsageSummary.total_tokens`, GAP-08c). The
 * per-provider on/off switch has no route at all (GAP-15), so it is rendered as
 * the design draws it and disabled.
 */

import { Button, Tag, chipVariant } from "@/components/ui";
import {
  MODEL_SCOPE_NOTE,
  useOrchestratorConfig,
  useUpdateOrchestratorConfig,
} from "@/hooks/useOrchestrator";
import {
  useLlmSettings,
  useModels,
  useProviderUsage,
} from "@/hooks/useSettings";
import { GAPS, gapNote } from "@/lib/unavailable";
import { useUiStore } from "@/stores/ui";

import { GapNote, ListCard, ListRow, ListState, Toggle } from "./primitives";
import { compactCount } from "./format";

const PROVIDER_TOGGLE_NOTE = gapNote(GAPS["GAP-15"]);

export function ModelsSection() {
  const llm = useLlmSettings();
  const models = useModels();
  const usage = useProviderUsage();
  const orchestrator = useOrchestratorConfig();
  const updateOrchestrator = useUpdateOrchestratorConfig();
  const setModel = useUiStore((s) => s.setModel);
  const showToast = useUiStore((s) => s.showToast);

  const providers = Object.entries(llm.data?.providers ?? {});
  const activeModel = orchestrator.data?.model ?? llm.data?.orchestrator.model;

  const pickModel = (modelId: string, provider: string) => {
    updateOrchestrator.mutate(
      {
        model: modelId,
        fallback_models: orchestrator.data?.fallback_models ?? [],
      },
      {
        onSuccess: () => {
          setModel(modelId);
          showToast(`Chat model → ${modelId} (${provider})`);
        },
        onError: (error) => showToast(`Could not switch — ${error.message}`),
      },
    );
  };

  return (
    <>
      <ListCard
        addLabel="Add provider"
        onAdd={() =>
          showToast(
            "Adding a provider needs the key editor, which is not built yet",
          )
        }
      >
        <ListState
          pending={llm.isPending}
          error={llm.error}
          empty={providers.length === 0}
          emptyCopy="No providers configured."
        >
          {providers.map(([provider, info]) => {
            const providerModels = (models.data ?? []).filter(
              (model) => model.provider === provider,
            );
            const summary = (usage.data ?? []).find(
              (row) => row.provider === provider,
            );
            return (
              <ListRow
                key={provider}
                name={provider}
                tags={<Tag value={info.enabled ? "active" : "off"} />}
                description={`${info.keys.length} ${
                  info.keys.length === 1 ? "key" : "keys"
                } · ${info.key_selection_strategy}`}
                chips={
                  providerModels.length === 0
                    ? undefined
                    : providerModels.map((model) => (
                        <Button
                          key={model.id}
                          variant={chipVariant(model.id === activeModel)}
                          disabled={updateOrchestrator.isPending}
                          onClick={() => pickModel(model.id, provider)}
                        >
                          {model.id === activeModel
                            ? `✓ ${model.id}`
                            : model.id}
                        </Button>
                      ))
                }
                meta={
                  summary === undefined
                    ? undefined
                    : `${compactCount(summary.total_tokens)} tok lifetime`
                }
                control={
                  <Toggle
                    checked={info.enabled}
                    label={`Enable ${provider}`}
                    disabled
                    disabledReason={PROVIDER_TOGGLE_NOTE}
                  />
                }
              />
            );
          })}
        </ListState>
      </ListCard>

      <GapNote>{MODEL_SCOPE_NOTE}.</GapNote>
      <GapNote>{PROVIDER_TOGGLE_NOTE}.</GapNote>
      <GapNote>
        Per-provider token counts are lifetime totals; the daemon serves no
        per-day breakdown.
      </GapNote>
    </>
  );
}
