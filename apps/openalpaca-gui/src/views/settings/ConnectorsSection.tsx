/**
 * Settings → Connectors (DESIGN_SPEC §5.4, API_MAP §2.4).
 *
 * Real: the connector list and the enable/disable toggle
 * (`POST /v1/connectors/{id}/action`). The `unwired` tag is a genuine
 * client-side join — a plugin declaring a connector that never appears in
 * `GET /v1/connectors` is exactly what the design's badge means.
 *
 * Unavailable: the `184 calls 7d` figure and the `Connect service` flow
 * (GAP-17). The route returns four fields and nothing counts calls.
 */

import { Tag } from "@/components/ui";
import {
  CONNECTOR_DETAIL_NOTE,
  useConnectorAction,
  useConnectors,
  useUnwiredConnectors,
} from "@/hooks/useConnectors";
import { useUiStore } from "@/stores/ui";

import { GapNote, ListCard, ListRow, ListState, Toggle } from "./primitives";

/** The daemon reports free-form status strings; these read as "on". */
function isEnabled(status: string): boolean {
  return /^(connected|running|active|enabled|live)$/i.test(status.trim());
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
                description={
                  connector.configured
                    ? `${connector.id} · configured`
                    : `${connector.id} · not configured`
                }
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
      <GapNote>{CONNECTOR_DETAIL_NOTE}.</GapNote>
    </>
  );
}
