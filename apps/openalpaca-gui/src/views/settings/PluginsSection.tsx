/**
 * Settings → Plugins (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * The best-backed section in the design: `GET /v1/plugins` is real and so are
 * approve / deny / enable / disable. The status strings are the daemon's own
 * (`PluginStatus::fmt` in `crates/openalpaca_plugins/src/manager.rs`):
 * `loading`, `waiting-approval`, `needs-config (…)`, `running`, `crashed: …`,
 * `disabled`, `denied`, `stopped` — matched here rather than guessed at.
 * `denied` is the consent refusal (the plugin is unloaded and stays out until
 * it is approved again); `disabled` is only the toggle position.
 *
 * A plugin waiting for first-load approval gets Approve/Deny instead of a
 * toggle: that is the gate the daemon actually enforces, and a switch would
 * misrepresent it.
 *
 * Unavailable: `Install plugin` (GAP-19) — plugins are copied into
 * `~/Library/Application Support/OpenAlpaca/plugins/` by hand.
 */

import { Button, Tag, type TagTone } from "@/components/ui";
import { useUnwiredConnectors } from "@/hooks/useConnectors";
import {
  usePluginAction,
  usePluginInstall,
  usePlugins,
} from "@/hooks/usePlugins";
import { gapDetail } from "@/lib/unavailable";
import { useUiStore } from "@/stores/ui";

import { GapNote, ListCard, ListRow, ListState, Toggle } from "./primitives";

/** `crashed: boom` and `needs-config (token)` carry detail; the tag wants a word. */
export function statusWord(status: string): string {
  const word = status.split(/[:(]/)[0];
  return (word ?? status).trim();
}

function statusTone(status: string): TagTone {
  const word = statusWord(status);
  if (word === "running") return "live";
  if (word === "crashed") return "warn";
  if (word === "waiting-approval" || word === "needs-config") return "asks";
  return "neutral";
}

export function PluginsSection() {
  const plugins = usePlugins();
  const action = usePluginAction();
  const install = usePluginInstall();
  const unwired = useUnwiredConnectors();
  const showToast = useUiStore((s) => s.showToast);

  const unwiredIds = new Set(unwired.map((entry) => entry.connectorId));
  const rows = plugins.data ?? [];
  // `usePluginInstall` is typed as an `Availability`, so narrow rather than
  // assume — the day an install route lands this branch simply stops firing.
  const installNote = install.available ? null : gapDetail(install);

  const run = (name: string, verb: "approve" | "deny" | "enable" | "disable") =>
    action.mutate(
      { name, action: verb },
      {
        onSuccess: () => showToast(`${name} ${verb}d`),
        onError: (error) => showToast(`Could not ${verb} — ${error.message}`),
      },
    );

  return (
    <>
      <ListCard
        addLabel="Install plugin"
        onAdd={() =>
          showToast(installNote ?? "Installing a plugin is not wired up yet")
        }
      >
        <ListState
          pending={plugins.isPending}
          error={plugins.error}
          empty={rows.length === 0}
          emptyCopy="No plugins installed."
        >
          {rows.map((plugin) => {
            const word = statusWord(plugin.status);
            const awaiting = word === "waiting-approval";
            const declaresUnwired =
              plugin.connector !== null && unwiredIds.has(plugin.connector);
            const parts = [
              `${plugin.tools.length} ${plugin.tools.length === 1 ? "tool" : "tools"}`,
            ];
            if (plugin.connector !== null)
              parts.push(`connector ${plugin.connector}`);
            if (plugin.provider !== null)
              parts.push(`provider ${plugin.provider}`);

            return (
              <ListRow
                key={plugin.name}
                name={plugin.name}
                tags={
                  <>
                    <Tag value={word} tone={statusTone(plugin.status)} />
                    {declaresUnwired && <Tag value="unwired" />}
                  </>
                }
                description={parts.join(" · ")}
                meta={`v${plugin.version}`}
                control={
                  awaiting ? (
                    <span className="flex shrink-0 gap-[6px]">
                      <Button
                        variant="primarySm"
                        disabled={action.isPending}
                        onClick={() => run(plugin.name, "approve")}
                      >
                        Approve
                      </Button>
                      <Button
                        variant="dangerGhost"
                        disabled={action.isPending}
                        onClick={() => run(plugin.name, "deny")}
                      >
                        Deny
                      </Button>
                    </span>
                  ) : (
                    <Toggle
                      checked={word === "running"}
                      label={`Enable ${plugin.name}`}
                      disabled={action.isPending}
                      onChange={(next) =>
                        run(plugin.name, next ? "enable" : "disable")
                      }
                    />
                  )
                }
              />
            );
          })}
        </ListState>
      </ListCard>

      {installNote !== null && <GapNote>{installNote}.</GapNote>}
    </>
  );
}
