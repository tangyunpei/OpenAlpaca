/** `/v1/orchestrator/*` — config plus the latency/decision telemetry. */

import { apiFetch } from "../http";
import type {
  DispatchDecisionRecord,
  LatencyAggregate,
  OrchestratorConfigResponse,
  OrchestratorLatencyRecord,
  UpdateOrchestratorRequest,
} from "./types";

/** `GET /v1/orchestrator/config` */
export async function getOrchestratorConfig(
  signal?: AbortSignal,
): Promise<OrchestratorConfigResponse> {
  return await apiFetch<OrchestratorConfigResponse>("/v1/orchestrator/config", {
    signal,
  });
}

/**
 * `PUT /v1/orchestrator/config` — GAP-13: this rewrites `llm.toml` and
 * broadcasts to every client and connector. It is the daemon *default*, not a
 * per-chat setting; label it that way in the UI.
 */
export async function updateOrchestratorConfig(
  req: UpdateOrchestratorRequest,
): Promise<void> {
  await apiFetch<void>("/v1/orchestrator/config", { method: "PUT", body: req });
}

export interface TelemetryQuery {
  mode?: string;
  from?: string;
  to?: string;
  limit?: number;
}

/** `GET /v1/orchestrator/latency` — envelope `{ records }`. */
export async function getLatencyRecords(
  query: TelemetryQuery = {},
  signal?: AbortSignal,
): Promise<OrchestratorLatencyRecord[]> {
  const data = await apiFetch<{ records: OrchestratorLatencyRecord[] }>(
    "/v1/orchestrator/latency",
    { query: { ...query }, signal },
  );
  return data.records;
}

/** `GET /v1/orchestrator/latency/aggregate` — envelope `{ aggregates }`. */
export async function getLatencyAggregates(
  query: { from?: string; to?: string } = {},
  signal?: AbortSignal,
): Promise<LatencyAggregate[]> {
  const data = await apiFetch<{ aggregates: LatencyAggregate[] }>(
    "/v1/orchestrator/latency/aggregate",
    { query: { ...query }, signal },
  );
  return data.aggregates;
}

/** `GET /v1/orchestrator/decisions` — envelope `{ records }`. */
export async function getDispatchDecisions(
  query: TelemetryQuery = {},
  signal?: AbortSignal,
): Promise<DispatchDecisionRecord[]> {
  const data = await apiFetch<{ records: DispatchDecisionRecord[] }>(
    "/v1/orchestrator/decisions",
    { query: { ...query }, signal },
  );
  return data.records;
}
