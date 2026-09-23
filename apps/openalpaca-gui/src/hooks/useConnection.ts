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

import { getDaemonStatus } from "@/lib/api/status";
import { getHealth } from "@/lib/api/telemetry";
import type { DaemonStatus, HealthResponse } from "@/lib/api/types";
import {
  getCachedConnection,
  shortInstanceId,
  subscribeConnection,
  subscribeInstanceChange,
  type ConnectionInfo,
} from "@/lib/connection";
import { daemonEvents, type EventsStatus } from "@/lib/events";
import { qk } from "@/lib/query-keys";
import { useProjectStore } from "@/stores/project";

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

/**
 * `GET /v1/status` — the daemon's own numbers (uptime, schema version, log
 * path, store sizes) plus the canonical root it resolves this window's project
 * path to (R50).
 *
 * Asked even with no project. It used to be disabled there, because the only
 * field that moved was `project_root` and `null` is what "no project" already
 * means; now the route carries the whole of GAP-14, so a projectless window
 * still has every reason to ask. The path stays in the query key: the answer
 * genuinely differs per project.
 *
 * `uptime_secs` is a snapshot, so the panel re-asks on a timer rather than
 * counting up locally — a client-side clock would keep ticking through a
 * restart the daemon would report as a fresh `started_at`.
 */
export function useDaemonStatus(
  workspacePath: string | null,
): UseQueryResult<DaemonStatus> {
  return useQuery({
    queryKey: qk.status(workspacePath),
    queryFn: ({ signal }) => getDaemonStatus(workspacePath, signal),
    refetchInterval: 30_000,
    staleTime: 10_000,
  });
}

/**
 * Whether this daemon would honour §5.6c's replay resume on an `interrupted`
 * run — `GET /v1/status`'s `routing.resume_enabled`.
 *
 * It reads the *same* query the Connection panel does, keyed by this window's
 * project, so asking it from a run card costs nothing beyond a cache read.
 *
 * The default is `false` on every uncertainty — still loading, the request
 * failed, or a daemon too old to send the field — because the failure modes
 * are not symmetric: a hidden control that should have been there is a
 * missing affordance, while a shown one that should not have been is a button
 * whose only possible answer is `409 RESUME_DISABLED`.
 */
export function useResumeEnabled(): boolean {
  const projectPath = useProjectStore((s) => s.path);
  const status = useDaemonStatus(projectPath);
  return status.data?.routing?.resume_enabled === true;
}

/**
 * The daemon's own home-store root — `GET /v1/status`'s `home_root` — or
 * `null` until it has answered (G7, S11).
 *
 * Copy that names a file under the store reads this rather than printing
 * `~/.openalpaca`: `OPENALPACA_HOME_STORE` moves the root, and a path written
 * from this side would then be a confident lie about where the owner's files
 * are. It reads the same query the Connection panel does, keyed by this
 * window's project, so asking from anywhere else costs a cache read.
 */
export function useHomeRoot(): string | null {
  const projectPath = useProjectStore((s) => s.path);
  return useDaemonStatus(projectPath).data?.home_root ?? null;
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
  /**
   * Why the last attempt to reach the daemon failed, verbatim, or `null`.
   *
   * For a daemon that would not start this is `ensure_daemon_running`'s
   * rejection, which ends with the daemon's own log lines — a legacy-schema
   * database, an older install's data, another daemon holding the lock. It
   * used to reach nobody: the sidecar's output went to `/dev/null`.
   */
  lastError: string | null;
  reconnect: () => Promise<void>;
}

export function useConnectionStatus(): ConnectionStatus {
  const info = useConnectionInfo();
  const health = useHealth();
  const [socket, setSocket] = useState<EventsStatus>(() =>
    daemonEvents.getStatus(),
  );
  const [lastError, setLastError] = useState<string | null>(() =>
    daemonEvents.getLastError(),
  );
  const client = useQueryClient();

  // The client records the error before it announces the status it causes,
  // so reading it on every status change is reading it at the right time.
  useEffect(
    () =>
      daemonEvents.onStatus((next) => {
        setSocket(next);
        setLastError(daemonEvents.getLastError());
      }),
    [],
  );

  // A daemon restart invalidates every cached id, not just the socket.
  useEffect(
    () =>
      subscribeInstanceChange(() => {
        client.clear();
      }),
    [client],
  );

  // `connect()` bootstraps (`ensure_daemon_running`) itself and records a
  // failure where `lastError` reads it. Bootstrapping here first, outside the
  // client, turned a daemon that would not start into an unhandled rejection
  // nobody saw.
  const reconnect = useCallback(async () => {
    daemonEvents.disconnect();
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
    lastError,
    reconnect,
  };
}
