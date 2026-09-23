/**
 * Daemon discovery and auth (API_MAP §1).
 *
 * The webview never reads `discovery.json`. Tauri commands do — the names are
 * read from `src-tauri/src/lib.rs`'s `tauri::generate_handler!`:
 *
 *   `ensure_daemon_running` — probes liveness, spawns the sidecar if dead,
 *                             polls up to ~5 s. Use on boot. When the daemon
 *                             does not come up, the rejection carries the end
 *                             of what it wrote to `daemon.log` — its own
 *                             reason, verbatim.
 *   `get_connection_info`   — reads discovery + expiry check. Use on reconnect.
 *   `read_daemon_log_tail`  — the end of `daemon.log`, read straight from the
 *                             file, so it answers with no daemon serving.
 *   `await_daemon_stopped`  — after `POST /v1/command {"command":"shutdown"}`,
 *                             waits until the process and its lock are gone.
 *
 * The first two return `ConnectionInfo`, serialized to the webview in
 * camelCase.
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

/**
 * The last `lines` lines of the daemon log, newest last — `""` when there is
 * no log yet.
 *
 * Read by the shell straight from `daemon.log`, not through the daemon, so it
 * answers for a daemon that would not start as well as one that is running.
 * Colour escapes are gone and nothing else is changed: render it verbatim.
 */
export async function readDaemonLogTail(lines: number): Promise<string> {
  let raw: unknown;
  try {
    raw = await invoke("read_daemon_log_tail", { lines });
  } catch (cause) {
    throw new ConnectionError(
      typeof cause === "string"
        ? cause
        : "Tauri command `read_daemon_log_tail` failed",
      cause,
    );
  }
  if (typeof raw !== "string") {
    throw new ConnectionError(
      "`read_daemon_log_tail` returned an unexpected payload",
      raw,
    );
  }
  return raw;
}

// ── Stop intent ─────────────────────────────────────────────────────────────

/**
 * Why this window has no daemon.
 *
 * `null`                — it should have one; a closed socket is a failure and
 *                         the reconnect ladder is right to climb.
 * `"stopped_here"`      — this window stopped it. Set before the shutdown
 *                         POST, cleared by `Start daemon`.
 * `"stopped_elsewhere"` — the daemon said it was going away (the
 *                         `daemon_shutting_down` frame) and this window did
 *                         not ask. The CLI, or another window, stopped it.
 *
 * While it is non-null nothing climbs a ladder and nothing respawns a daemon:
 * auto-restarting a daemon the user just stopped is exactly wrong, so the way
 * back is an explicit `Start daemon`. Window-level and never persisted —
 * reopening the app is a fresh intent to use it, and `ensure_daemon_running`
 * spawning a daemon on that boot is correct.
 */
export type StopIntent = "stopped_here" | "stopped_elsewhere" | null;

let stopIntent: StopIntent = null;
let stoppedAt: number | null = null;
const stopIntentListeners = new Set<(intent: StopIntent) => void>();

export function getStopIntent(): StopIntent {
  return stopIntent;
}

/** Wall-clock ms the current stop intent was set; `null` while there is none. */
export function getStoppedAt(): number | null {
  return stoppedAt;
}

export function setStopIntent(
  next: StopIntent,
  now: number = Date.now(),
): void {
  if (next === stopIntent) return;
  stopIntent = next;
  stoppedAt = next === null ? null : now;
  for (const listener of stopIntentListeners) listener(next);
}

export function subscribeStopIntent(
  listener: (intent: StopIntent) => void,
): () => void {
  stopIntentListeners.add(listener);
  return () => stopIntentListeners.delete(listener);
}

/**
 * What `await_daemon_stopped` concluded. Only `not_running` and `stopped`
 * mean a daemon may be started now; `pid` is set only for `still_alive`.
 */
export interface DaemonStopReport {
  outcome: "not_running" | "stopped" | "lock_still_held" | "still_alive";
  pid: number | null;
  waitedMs: number;
}

const STOP_OUTCOMES: ReadonlySet<string> = new Set([
  "not_running",
  "stopped",
  "lock_still_held",
  "still_alive",
]);

/**
 * Wait — up to 15 s, in the shell — until the daemon's process has exited and
 * its single-instance lock is free. The shutdown route's `200` says only that
 * the daemon was asked; this is what says it is gone.
 */
export async function awaitDaemonStopped(): Promise<DaemonStopReport> {
  let raw: unknown;
  try {
    raw = await invoke("await_daemon_stopped");
  } catch (cause) {
    throw new ConnectionError(
      typeof cause === "string"
        ? cause
        : "Tauri command `await_daemon_stopped` failed",
      cause,
    );
  }
  const report = raw as Partial<DaemonStopReport> | null;
  if (
    typeof report !== "object" ||
    report === null ||
    typeof report.outcome !== "string" ||
    !STOP_OUTCOMES.has(report.outcome)
  ) {
    throw new ConnectionError(
      "`await_daemon_stopped` returned an unexpected payload",
      raw,
    );
  }
  return {
    outcome: report.outcome,
    pid: typeof report.pid === "number" ? report.pid : null,
    waitedMs: typeof report.waitedMs === "number" ? report.waitedMs : 0,
  };
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
