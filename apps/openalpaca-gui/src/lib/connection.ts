/**
 * Daemon discovery and auth (API_MAP §1).
 *
 * The webview never reads `discovery.json`. Two Tauri commands do — the names
 * are read from `src-tauri/src/lib.rs`'s `tauri::generate_handler!`:
 *
 *   `ensure_daemon_running` — probes liveness, spawns the sidecar if dead,
 *                             polls up to ~5 s. Use on boot.
 *   `get_connection_info`   — reads discovery + expiry check. Use on reconnect.
 *
 * Both return `ConnectionInfo`, serialized to the webview in camelCase.
 *
 * `instanceId` is the identity guard: a change means the daemon restarted, so
 * every `task_id`, `stream_id` and `request_id` the client holds is dead and
 * the app must fully re-bootstrap rather than merely reopen its socket.
 */

import { invoke } from "@tauri-apps/api/core";

export interface ConnectionInfo {
  baseUrl: string;
  token: string;
  instanceId: string;
}

/** Thrown when discovery is missing/expired or the Tauri bridge is absent. */
export class ConnectionError extends Error {
  override readonly name = "ConnectionError";

  constructor(
    message: string,
    /** The underlying Tauri rejection or malformed payload. */
    readonly detail: unknown = null,
  ) {
    super(message);
  }
}

export type ConnectionListener = (info: ConnectionInfo | null) => void;
export type InstanceChangeListener = (
  next: ConnectionInfo,
  previous: ConnectionInfo,
) => void;

let current: ConnectionInfo | null = null;
let bootstrapInFlight: Promise<ConnectionInfo> | null = null;

const connectionListeners = new Set<ConnectionListener>();
const instanceListeners = new Set<InstanceChangeListener>();

function isConnectionInfo(value: unknown): value is ConnectionInfo {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.baseUrl === "string" &&
    typeof v.token === "string" &&
    typeof v.instanceId === "string"
  );
}

function publish(info: ConnectionInfo | null): void {
  current = info;
  for (const listener of connectionListeners) listener(info);
}

async function invokeConnection(
  command: "get_connection_info" | "ensure_daemon_running",
) {
  let raw: unknown;
  try {
    raw = await invoke(command);
  } catch (cause) {
    throw new ConnectionError(
      typeof cause === "string" ? cause : `Tauri command \`${command}\` failed`,
      cause,
    );
  }
  if (!isConnectionInfo(raw)) {
    throw new ConnectionError(
      `\`${command}\` returned an unexpected payload`,
      raw,
    );
  }
  return raw;
}

/** The cached connection, or `null` before the first successful bootstrap. */
export function getCachedConnection(): ConnectionInfo | null {
  return current;
}

/** Subscribe to connection changes. Returns an unsubscribe function. */
export function subscribeConnection(listener: ConnectionListener): () => void {
  connectionListeners.add(listener);
  return () => connectionListeners.delete(listener);
}

/**
 * Subscribe to daemon-identity changes. Fired when a refresh observes a
 * different `instanceId` — consumers must drop all server-derived state.
 */
export function subscribeInstanceChange(
  listener: InstanceChangeListener,
): () => void {
  instanceListeners.add(listener);
  return () => instanceListeners.delete(listener);
}

/**
 * Boot path: ensure a daemon exists (spawning the sidecar if needed) and cache
 * the result. Concurrent callers share one in-flight invocation.
 */
export function bootstrapConnection(): Promise<ConnectionInfo> {
  if (bootstrapInFlight) return bootstrapInFlight;

  bootstrapInFlight = invokeConnection("ensure_daemon_running")
    .then((info) => {
      const previous = current;
      publish(info);
      if (previous && previous.instanceId !== info.instanceId) {
        for (const listener of instanceListeners) listener(info, previous);
      }
      return info;
    })
    .finally(() => {
      bootstrapInFlight = null;
    });

  return bootstrapInFlight;
}

/**
 * Reconnect path: re-read discovery. On an `instanceId` mismatch this performs
 * a **full re-bootstrap** rather than handing back a connection to a daemon the
 * caller's state no longer matches.
 */
export async function refreshConnection(): Promise<ConnectionInfo> {
  const previous = current;
  const info = await invokeConnection("get_connection_info");

  if (previous && previous.instanceId !== info.instanceId) {
    publish(null);
    const rebooted = await bootstrapConnection();
    for (const listener of instanceListeners) listener(rebooted, previous);
    return rebooted;
  }

  publish(info);
  return info;
}

/** The cached connection, bootstrapping on first use. */
export async function ensureConnection(): Promise<ConnectionInfo> {
  return current ?? (await bootstrapConnection());
}

/** Drop the cached connection (tests, and an explicit user-driven reconnect). */
export function resetConnection(): void {
  publish(null);
}

/** `http(s)://host:port` + path. */
export function httpUrl(info: ConnectionInfo, path: string): string {
  return `${info.baseUrl}${path}`;
}

/**
 * `ws(s)://host:port` + path + `?token=` — browsers cannot set WebSocket
 * headers, so `/v1/events` validates the token inline from the query string.
 */
export function wsUrl(info: ConnectionInfo, path: string): string {
  const base = info.baseUrl.replace(/^http/, "ws");
  return `${base}${path}?token=${encodeURIComponent(info.token)}`;
}

/**
 * SSE chat stream URL. `EventSource` cannot set headers either, which is why
 * this route is merged outside the auth middleware and checks `?token=` itself.
 */
export function sseUrl(info: ConnectionInfo, streamId: string): string {
  return `${info.baseUrl}/v1/chat/stream/${encodeURIComponent(streamId)}?token=${encodeURIComponent(info.token)}`;
}

/** The design's `connected · 7f3a` chip. */
export function shortInstanceId(instanceId: string): string {
  return instanceId.replace(/-/g, "").slice(0, 4);
}
