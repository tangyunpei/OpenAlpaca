/**
 * Settings → Connectors (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Real: the connector list and the enable/disable toggle
 * (`POST /v1/connectors/{id}/action`). The `unwired` tag is a genuine
 * client-side join — a plugin declaring a connector that never appears in
 * `GET /v1/connectors` is exactly what the design's badge means.
 *
 * Real since T49: the row detail. The design's `184 calls 7d` is served as
 * `messages_7d` — the messages the daemon actually attributed to that
 * connector over the last seven UTC days — and the name is the connector's
 * own, so Discord is no longer printed as `discord`. `registered` answers the
 * one question `status` cannot: an `error` row is either a connector that
 * started and exited or one that never started.
 *
 * Unavailable: the `Connect service` flow (GAP-17, narrowed). Connectors are
 * compiled into the daemon; no route adds one.
 */

import { Tag } from "@/components/ui";
import {
  CONNECTOR_ADD_NOTE,
  useConnectorAction,
  useConnectors,
  useUnwiredConnectors,
} from "@/hooks/useConnectors";
import type { Connector } from "@/lib/api/types";
import { useUiStore } from "@/stores/ui";

import { GapNote, ListCard, ListRow, ListState, Toggle } from "./primitives";

/** The daemon reports free-form status strings; these read as "on". */
function isEnabled(status: string): boolean {
  return /^(connected|running|active|enabled|live)$/i.test(status.trim());
}

/**
 * `184 messages · 7d`. A connector nobody messaged this week says so in words —
 * `0 messages` reads like a metric that failed to load, and the window is named
 * because a bare zero would otherwise claim the connector has never been used.
 */
function connectorMeta(connector: Connector): string {
  if (!connector.messages_7d) return "No messages · 7d";
  const noun = connector.messages_7d === 1 ? "message" : "messages";
  return `${connector.messages_7d} ${noun} · 7d`;
}

/**
 * `telegram · configured`, plus what `registered` adds where it adds anything.
 *
 * The daemon reports `error` both for a connector whose task exited and for one
 * that was never spawned at all; `registered` — the manager's handle registry —
 * is the only thing that tells them apart, so it is spoken only there. On a
 * healthy row it would be noise: `active` already implies a live handle.
 */
function connectorDescription(connector: Connector): string {
  const parts = [
    connector.source,
    connector.configured ? "configured" : "not configured",
  ];
  if (connector.status.trim().toLowerCase() === "error") {
    parts.push(connector.registered ? "started, then exited" : "never started");
  }
  return parts.join(" · ");
}

export function ConnectorsSection() {
  const connectors = useConnectors();
  const unwired = useUnwiredConnectors();
  const action = useConnectorAction();
  const showToast = useUiStore((s) => s.showToast);

  const unwiredIds = new Set(unwired.map((entry) => entry.connectorId));
  const rows = connectors.data ?? [];

  return (
    <>
      <ListCard
        addLabel="Connect service"
        onAdd={() =>
          showToast("Adding a connector has no daemon route yet — see GAP-17")
        }
      >
        <ListState
          pending={connectors.isPending}
          error={connectors.error}
          empty={rows.length === 0}
          emptyCopy="No connectors registered."
        >
          {rows.map((connector) => {
            const on = isEnabled(connector.status);
            return (
              <ListRow
                key={connector.id}
                name={connector.name}
                tags={
                  <>
                    <Tag value={connector.status} />
                    {unwiredIds.has(connector.id) && <Tag value="unwired" />}
                  </>
                }
                description={connectorDescription(connector)}
                meta={connectorMeta(connector)}
                control={
                  <Toggle
                    checked={on}
                    label={`Enable ${connector.name}`}
                    disabled={action.isPending}
                    onChange={(next) =>
                      action.mutate(
                        {
                          id: connector.id,
                          action: next ? "enable" : "disable",
                        },
                        {
                          onSuccess: () =>
                            showToast(
                              `${connector.name} ${next ? "enabled" : "disabled"}`,
                            ),
                          onError: (error) =>
                            showToast(`Could not change — ${error.message}`),
                        },
                      )
                    }
                  />
                }
              />
            );
          })}
        </ListState>
      </ListCard>

      {unwired.length > 0 && (
        <GapNote>
          {unwired
            .map(
              (entry) =>
                `${entry.declaredBy} declares ${entry.connectorId}, which is not registered`,
            )
            .join(" · ")}
          .
        </GapNote>
      )}
      <GapNote>{CONNECTOR_ADD_NOTE}.</GapNote>
    </>
  );
}
