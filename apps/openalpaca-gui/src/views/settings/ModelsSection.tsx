/**
 * Settings → Models & keys (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Real: the provider list and its keys (`GET /v1/settings/llm`), the model
 * catalogue (`GET /v1/models`), and picking a model — which writes
 * `PUT /v1/orchestrator/config`.
 *
 * Two honesty notes ride with that write. It is **daemon-wide**, not
 * per-conversation (GAP-13), and the per-provider token figure the design shows
 * as "today" is lifetime (`ProviderUsageSummary.total_tokens`, GAP-08c).
 *
 * The per-provider switch is real (GAP-15 closed):
 * `PUT /v1/settings/llm/providers/{provider}/enabled` writes the bit to
 * `llm.toml` and unloads or reloads the provider live. It moves optimistically
 * and is put back if the daemon refuses — the refusal an owner will meet is
 * the `409` on the provider that serves the chat model.
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
  useSetProviderEnabled,
} from "@/hooks/useSettings";
import { useUiStore } from "@/stores/ui";

import { GapNote, ListCard, ListRow, ListState, Toggle } from "./primitives";
import { compactCount } from "./format";
import { providerToggleErrorCopy } from "./provider-toggle";

export function ModelsSection() {
  const llm = useLlmSettings();
  const models = useModels();
  const usage = useProviderUsage();
  const orchestrator = useOrchestratorConfig();
  const updateOrchestrator = useUpdateOrchestratorConfig();
  const setProviderEnabled = useSetProviderEnabled();
  const setModel = useUiStore((s) => s.setModel);
  const showToast = useUiStore((s) => s.showToast);

  const providers = Object.entries(llm.data?.providers ?? {});
  const activeModel = orchestrator.data?.model ?? llm.data?.orchestrator.model;

  const toggleProvider = (provider: string, next: boolean) => {
    setProviderEnabled.mutate(
      { provider, enabled: next },
      {
        onSuccess: (row) =>
          showToast(`${row.id} ${row.enabled ? "on" : "off"}`),
        onError: (error) => showToast(providerToggleErrorCopy(provider, error)),
      },
    );
  };

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
                    disabled={setProviderEnabled.isPending}
                    disabledReason="switching…"
                    onChange={(next) => toggleProvider(provider, next)}
                  />
                }
              />
            );
          })}
        </ListState>
      </ListCard>

      <GapNote>{MODEL_SCOPE_NOTE}.</GapNote>
      <GapNote>
        Per-provider token counts are lifetime totals; the daemon serves no
        per-day breakdown.
      </GapNote>
    </>
  );
}
