/**
 * Settings → Models & keys (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Real: the provider list and its keys (`GET /v1/settings/llm`), the model
 * catalogue (`GET /v1/models`), and picking a model — which writes
 * `PUT /v1/orchestrator/config`.
 *
 * One note rides with that write: it is the **daemon-wide** default. The chat
 * composer's own picker no longer touches it — GAP-13 closed, and a pick there
 * is carried on that turn's `POST /v1/chat` as `model` and persisted nowhere.
 *
 * The per-provider figure is real since T50 (GAP-08c, closed). It is today's,
 * off `GET /v1/usage/summary`'s `by_provider` — that day's `llm_call_log`
 * rows — where the design's `41k tok today` used to be
 * `ProviderUsageSummary.total_tokens`, a *lifetime* total under a heading
 * that said today. A provider with no calls today says so rather than
 * borrowing its lifetime number.
 *
 * The per-provider switch is real (GAP-15 closed):
 * `PUT /v1/settings/llm/providers/{provider}/enabled` writes the bit to
 * `llm.toml` and unloads or reloads the provider live. It moves optimistically
 * and is put back if the daemon refuses — the refusal an owner will meet is
 * the `409` on the provider that serves the chat model.
 *
 * A `200` is not always a load: turning on a provider with no usable key
 * writes the bit and leaves the router with nothing (`loaded: false`). The
 * switch still moves — the file says on — so the row carries the daemon's
 * `warning` beneath it, because a switch that says on beside a provider that
 * cannot answer is the silent degradation the rules reject. That `warning`
 * arrives on the toggle's own response and on nothing else, so after a reload
 * the same provider had nothing but the word `active` beside it; the row now
 * reads the catalogue instead — enabled, `GET /v1/models` answered, and not a
 * model from this provider in it is "on, but no models loaded", every time the
 * page is drawn.
 *
 * The local-model story (L1/L2/L3) is three more facts on the same screen:
 * a provider that needs no key says so instead of counting zero keys; the
 * enable's `discovered_models` is what the toast reports, so switching Ollama
 * on answers "found 3 models" or names why it could not ask; and `Refresh`
 * (`POST /v1/models/refresh`) re-asks every enabled provider, which is how a
 * model pulled after boot reaches the picker. Above the card, when the
 * configured default is not routable, the daemon's own `effective_default_model`
 * is shown — a picker that disagrees with every reply is the worst kind of
 * silent substitution.
 *
 * Keys are added here (D-F, DESIGN_SPEC §5.4a): `Add key` in the card header
 * and on each row opens `AddKeyForm`, which refuses a switched-off provider
 * before any typing and offers this section's own `toggleProvider` as the way
 * out — the same mutation the row's switch calls.
 */

import { useState } from "react";

import { Button, Tag, chipVariant } from "@/components/ui";
import { useDaemonStatus } from "@/hooks/useConnection";
import {
  useOrchestratorConfig,
  useUpdateOrchestratorConfig,
} from "@/hooks/useOrchestrator";
import {
  useLlmSettings,
  useModels,
  useRefreshModels,
  useSetProviderEnabled,
} from "@/hooks/useSettings";
import { formatSpend, useUsageSummary } from "@/hooks/useUsage";
import { effectiveModelNote } from "@/lib/model-availability";
import { useProjectStore } from "@/stores/project";
import { useUiStore } from "@/stores/ui";

import { AddKeyForm } from "./AddKeyForm";
import { GapNote, ListCard, ListRow, ListState, Toggle } from "./primitives";
import { compactCount } from "./format";
import { providerKeyLine, providerToggleToast } from "./models-copy";
import { providerToggleErrorCopy } from "./provider-toggle";

export function ModelsSection() {
  const llm = useLlmSettings();
  const models = useModels();
  const refreshModels = useRefreshModels();
  const usage = useUsageSummary();
  const orchestrator = useOrchestratorConfig();
  const updateOrchestrator = useUpdateOrchestratorConfig();
  const setProviderEnabled = useSetProviderEnabled();
  const projectPath = useProjectStore((s) => s.path);
  // `GET /v1/status` is where the daemon reports the pair (L3); it is already
  // this window's polling query, so reading it here costs a cache hit.
  const daemonStatus = useDaemonStatus(projectPath);
  const setModel = useUiStore((s) => s.setModel);
  const showToast = useUiStore((s) => s.showToast);
  // Per provider, the daemon's reason for a write that did not load. It comes
  // back on the toggle's own response and on nothing else — `GET
  // /v1/settings/llm` reports the file, which by then says `enabled = true` —
  // so it is kept here until the next toggle of that row answers.
  const [notLoaded, setNotLoaded] = useState<Record<string, string>>({});
  // The provider the `Add key` form is open on, or `null` when it is shut.
  const [addingFor, setAddingFor] = useState<string | null>(null);

  const providers = Object.entries(llm.data?.providers ?? {});
  const activeModel = orchestrator.data?.model ?? llm.data?.orchestrator.model;
  const substitution = effectiveModelNote(daemonStatus.data?.llm);

  const refresh = () => {
    refreshModels.mutate(undefined, {
      onSuccess: (rows) =>
        showToast(
          rows.length === 0
            ? "No models — every provider is off, or none could be asked"
            : `${rows.length} ${rows.length === 1 ? "model" : "models"} in the catalogue`,
        ),
      onError: (error) => showToast(`Could not refresh — ${error.message}`),
    });
  };

  const toggleProvider = (provider: string, next: boolean) => {
    setProviderEnabled.mutate(
      { provider, enabled: next },
      {
        onSuccess: (row) => {
          setNotLoaded((current) => {
            const { [row.id]: _dropped, ...rest } = current;
            return row.loaded || row.warning === null
              ? rest
              : { ...rest, [row.id]: row.warning };
          });
          showToast(providerToggleToast(row));
        },
        // `activeModel` is the daemon's `[orchestrator] model` — the very field
        // the 409 guard resolves — so the unresolved-default refusal can name
        // the id the owner has to fix.
        onError: (error) =>
          showToast(providerToggleErrorCopy(provider, error, activeModel)),
      },
    );
  };

  const pickModel = (modelId: string, provider: string) => {
    updateOrchestrator.mutate(
      {
        model: modelId,
        // `PUT /v1/orchestrator/config` requires the whole pair, and an
        // omitted chain is an *erased* chain (`[]` → `None` on the daemon).
        // `GET /v1/orchestrator/config` is the one read here that touches the
        // DB under the single mutex, so it can still be in flight while the
        // chips — drawn off `GET /v1/models` — are already clickable. The
        // same field rides on `GET /v1/settings/llm`, off the same file read,
        // and a chip cannot exist without it, so the second source is the one
        // that makes the `?? []` tail unreachable rather than destructive.
        fallback_models:
          orchestrator.data?.fallback_models ??
          llm.data?.orchestrator.fallback_models ??
          [],
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
      {substitution !== null && (
        <p
          role="status"
          className="mt-0 mb-[10px] rounded-2xl border border-amber-line bg-amber-surface px-[12px] py-[9px] text-base leading-[1.5] text-amber-ink"
        >
          {substitution}
        </p>
      )}

      {/* `Add key`, not `Add provider`: the providers are compiled in and
          `GET /v1/settings/llm` enumerates every one of them, so there is no
          provider to add — what an owner can add is a key (D-F, §5.4a). */}
      <ListCard
        addLabel="Add key"
        onAdd={() =>
          setAddingFor(
            addingFor === null ? (providers[0]?.[0] ?? "anthropic") : null,
          )
        }
        actions={
          <Button
            variant="ghostSm"
            disabled={refreshModels.isPending}
            onClick={refresh}
          >
            {refreshModels.isPending ? "Refreshing…" : "Refresh models"}
          </Button>
        }
      >
        {addingFor !== null && (
          <AddKeyForm
            provider={addingFor}
            onChangeProvider={setAddingFor}
            onDone={showToast}
            onCancel={() => setAddingFor(null)}
            onEnableProvider={(provider) => toggleProvider(provider, true)}
            enableBusy={setProviderEnabled.isPending}
          />
        )}
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
            const today = (usage.data?.by_provider ?? []).find(
              (row) => row.provider === provider,
            );
            // The file says on and the catalogue has arrived carrying nothing
            // for this provider: the router holds no model from it, whatever
            // the bit says. Only once `models.data` is defined — an empty
            // catalogue that has not loaded yet would otherwise accuse every
            // provider of the same thing.
            const unloaded =
              info.enabled &&
              models.data !== undefined &&
              providerModels.length === 0;
            return (
              <ListRow
                key={provider}
                name={provider}
                tags={
                  <Tag
                    value={info.enabled ? (unloaded ? "on" : "active") : "off"}
                    {...(unloaded ? { tone: "warn" as const } : {})}
                  />
                }
                description={
                  <>
                    {providerKeyLine(info)}
                    {notLoaded[provider] !== undefined && (
                      <span className="mt-[3px] block text-red-ink">
                        On, but not loaded — {notLoaded[provider]}
                      </span>
                    )}
                    {unloaded && notLoaded[provider] === undefined && (
                      <span className="mt-[3px] block text-amber-ink">
                        On, but no models loaded
                      </span>
                    )}
                  </>
                }
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
                  usage.data === undefined
                    ? undefined
                    : today === undefined
                      ? "No calls today"
                      : `${compactCount(today.tokens)} tok today · ${formatSpend(today.usd)}`
                }
                control={
                  <span className="flex shrink-0 items-center gap-[8px]">
                    {/* The shortest path from "On, but not loaded — No keys
                        for …" to the field that fixes it. Kept for a keyless
                        provider too: the form then says there is nothing to
                        type, which is the answer rather than a missing
                        control. */}
                    <Button
                      variant="ghostXs"
                      aria-label={`Add key for ${provider}`}
                      onClick={() => setAddingFor(provider)}
                    >
                      Add key
                    </Button>
                    <Toggle
                      checked={info.enabled}
                      label={`Enable ${provider}`}
                      disabled={setProviderEnabled.isPending}
                      disabledReason="switching…"
                      onChange={(next) => toggleProvider(provider, next)}
                    />
                  </span>
                }
              />
            );
          })}
        </ListState>
      </ListCard>

      <GapNote>
        This is the daemon-wide default — every client that names no model of
        its own gets it. The chat composer&apos;s picker is per-conversation and
        writes nothing here.
      </GapNote>
    </>
  );
}
