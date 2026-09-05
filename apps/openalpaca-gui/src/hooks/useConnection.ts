/**
 * Connection identity for the Settings → Connection panel and the header chip.
 *
 * The `instanceId` guard lives here: when `/v1/health` reports a different
 * instance than the cached `ConnectionInfo`, the daemon restarted and every
 * server-derived id the app holds is dead — so the whole cache is dropped.
 */

import {
  useQuery,
  useQueryClient,
  type UseQueryResult,
} from "@tanstack/react-query";
import { useCallback, useEffect, useState } from "react";

import { getHealth } from "@/lib/api/telemetry";
import type { HealthResponse } from "@/lib/api/types";
import {
  bootstrapConnection,
  getCachedConnection,
  shortInstanceId,
  subscribeConnection,
  subscribeInstanceChange,
  type ConnectionInfo,
} from "@/lib/connection";
import { daemonEvents, type EventsStatus } from "@/lib/events";
import { qk } from "@/lib/query-keys";

/** The cached `ConnectionInfo`, kept in sync with the connection module. */
export function useConnectionInfo(): ConnectionInfo | null {
  const [info, setInfo] = useState<ConnectionInfo | null>(getCachedConnection);
  useEffect(() => subscribeConnection(setInfo), []);
  return info;
}

/** `GET /v1/health` — unauthenticated liveness plus the instance id. */
export function useHealth(): UseQueryResult<HealthResponse> {
  return useQuery({
    queryKey: qk.health(),
    queryFn: ({ signal }) => getHealth(signal),
    refetchInterval: 30_000,
    staleTime: 10_000,
  });
}

export interface ConnectionStatus {
  info: ConnectionInfo | null;
  health: HealthResponse | undefined;
  socket: EventsStatus;
  /** `true` once the socket is up and health agrees on the instance. */
  connected: boolean;
  /** The design's `connected · 7f3a` chip. */
  instanceChip: string | null;
  endpoint: string | null;
  reconnect: () => Promise<void>;
}

export function useConnectionStatus(): ConnectionStatus {
  const info = useConnectionInfo();
  const health = useHealth();
  const [socket, setSocket] = useState<EventsStatus>(() =>
    daemonEvents.getStatus(),
  );
  const client = useQueryClient();

  useEffect(() => daemonEvents.onStatus(setSocket), []);

  // A daemon restart invalidates every cached id, not just the socket.
  useEffect(
    () =>
      subscribeInstanceChange(() => {
        client.clear();
      }),
    [client],
  );

  const reconnect = useCallback(async () => {
    daemonEvents.disconnect();
    await bootstrapConnection();
    await daemonEvents.connect();
    await client.invalidateQueries();
  }, [client]);

  const instanceId = health.data?.instance_id ?? info?.instanceId ?? null;

  return {
    info,
    health: health.data,
    socket,
    connected: socket === "connected" && health.isSuccess,
    instanceChip: instanceId === null ? null : shortInstanceId(instanceId),
    endpoint: info === null ? null : info.baseUrl.replace(/^https?:\/\//, ""),
    reconnect,
  };
}
