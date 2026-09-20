/**
 * `ModelPicker` (DESIGN_SPEC §3.17) — the popover above the composer.
 *
 * One honest departure from the design: the **off pill** on a disabled
 * provider is not rendered. The picker is built from `GET /v1/models`, which
 * lists models rather than providers, so a disabled provider has no row here
 * to badge in the first place (turning one off is served — Settings → Models &
 * keys).
 *
 * The footer's one-line scope note used to say that picking here wrote the
 * daemon-wide default (GAP-13). It no longer does: a pick is carried on each
 * turn's `POST /v1/chat` as `model`, so the design's conversation-scoped
 * picker is what this now is, and the note says that instead.
 */

import { Scrim } from "@/components/ui";
import { cn } from "@/lib/cn";
import type { ModelEntry } from "@/lib/api/types";

export interface ModelGroup {
  provider: string;
  models: ModelEntry[];
}

/** Group by provider, keeping the daemon's own ordering within each group. */
export function groupModelsByProvider(
  models: readonly ModelEntry[],
): ModelGroup[] {
  const groups: ModelGroup[] = [];
  const index = new Map<string, ModelGroup>();
  for (const model of models) {
    let group = index.get(model.provider);
    if (group === undefined) {
      group = { provider: model.provider, models: [] };
      index.set(model.provider, group);
      groups.push(group);
    }
    group.models.push(model);
  }
  return groups;
}

export interface ModelPickerProps {
  models: readonly ModelEntry[];
  selected: string | null;
  onPick: (modelId: string) => void;
  onClose: () => void;
  onManageProviders: () => void;
  /** The one-line scope note under the list. */
  note?: string | null;
  /** Set while `/v1/models` is in flight or failed. */
  status?: "ready" | "loading" | "error";
}

export function ModelPicker({
  models,
  selected,
  onPick,
  onClose,
  onManageProviders,
  note = null,
  status = "ready",
}: ModelPickerProps) {
  const groups = groupModelsByProvider(models);

  return (
    <>
      <Scrim zIndex={39} onClose={onClose} />
      <div
        role="dialog"
        aria-label="Chat model"
        className="absolute right-0 bottom-[calc(100%+8px)] z-40 w-[272px] overflow-hidden rounded-2xl border border-line-popover bg-raised shadow-popover"
      >
        <p className="m-0 border-b border-line-hair-2 px-[13px] pt-[9px] pb-[7px] font-mono text-[8.5px] tracking-eyebrow-w text-faint uppercase">
          Chat model
        </p>

        <div className="sc max-h-[300px] overflow-y-auto">
          {status !== "ready" && (
            <p className="m-0 px-[13px] py-[10px] font-mono text-2xs-plus text-faint">
              {status === "loading"
                ? "Loading models…"
                : "Could not read the model registry."}
            </p>
          )}
          {status === "ready" && groups.length === 0 && (
            <p className="m-0 px-[13px] py-[10px] font-mono text-2xs-plus text-faint">
              No models are registered on this daemon.
            </p>
          )}

          {groups.map((group) => (
            <div key={group.provider}>
              <div className="flex items-center gap-[6px] px-[13px] pt-[8px] pb-[3px]">
                <span className="text-sm font-semibold text-ink">
                  {group.provider}
                </span>
              </div>
              {group.models.map((model) => {
                const active = model.id === selected;
                return (
                  <button
                    key={model.id}
                    type="button"
                    aria-current={active ? "true" : undefined}
                    onClick={() => onPick(model.id)}
                    className={cn(
                      "flex w-full cursor-pointer items-center gap-[8px] border-none px-[13px] py-[7px] text-left font-mono text-sm",
                      "focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-blue",
                      active
                        ? "bg-muted-2 text-ink"
                        : "bg-transparent text-secondary hover:bg-line-hair-3",
                    )}
                  >
                    <span aria-hidden className="w-[11px] shrink-0 text-green">
                      {active ? "✓" : ""}
                    </span>
                    {model.id}
                  </button>
                );
              })}
            </div>
          ))}
        </div>

        <div className="border-t border-line-hair-2 bg-sunken px-[13px] py-[8px]">
          {note !== null && note !== "" && (
            <p className="mt-0 mb-[5px] font-mono text-2xs text-faint">
              {note}
            </p>
          )}
          <button
            type="button"
            onClick={onManageProviders}
            className="cursor-pointer border-none bg-transparent p-0 font-mono text-2xs-plus text-muted-fg hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue"
          >
            Manage providers &amp; keys ↗
          </button>
        </div>
      </div>
    </>
  );
}
