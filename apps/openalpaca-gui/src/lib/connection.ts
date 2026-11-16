/**
 * Daemon discovery and auth (API_MAP §1).
 *
 * The webview never reads `discovery.json`. Tauri commands do — the names are
 * read from `src-tauri/src/lib.rs`'s `tauri::generate_handler!`:
 *
 *   `ensure_daemon_running` — asks the daemon `discovery.json` names
 *                             `GET /v1/health` and judges it (Q04): reuses one
 *                             this app speaks to, refuses one it cannot use
 *                             (`mismatch`, never parked on a spawn), answers
 *                             `starting` at once for one that is up but not
 *                             answering yet (this window asks again on its own
 *                             timer, `lib/events.ts`), and only with no live
 *                             daemon spawns the sidecar and waits ~5 s for the
 *                             file to name a daemon, answering `starting` at
 *                             once for one that is up but silent, as above
 *                             (Q04-VERDICT). Use on boot. When the daemon does
 *                             not come up, the rejection carries the end of
 *                             what it wrote to `daemon.log` — its own reason,
 *                             verbatim.
 *   `get_connection_info`   — reads discovery + expiry check, and refuses
 *                             (`mismatch`) a live daemon whose file states a
 *                             protocol this app does not speak, so the
 *                             reconnect ladder parks on it too (Q04). Use on
 *                             reconnect, when a request is refused `401`, and
 *                             once the cache is an hour old by the wall clock
 *                             (`keepConnectionFresh`): the daemon rotates its
 *                             token while it runs (T21).
 *   `read_daemon_log_tail`  — the end of `daemon.log`, read straight from the
 *                             file, so it answers with no daemon serving.
 *   `await_daemon_stopped`  — after `POST /v1/command {"command":"shutdown"}`,
 *                             waits until the process and its lock are gone.
 *
 * The first two return `ConnectionInfo`, serialized to the webview in
 * camelCase, with the daemon's `version` and `protocol` and the shell's
 * `note` on a daemon of an older protocol or another version (Q04), and
 * `daemonPath` (Q21): where the `PATH` of a daemon this run
 * of the app spawned came from — the login shell's, captured at the app's
 * start (read again without `-i` when, run interactively, it ended before
 * it printed its environment: CAPTURE-RETRY), or the fallback list — and
 * how long the capture took, never the
 * `PATH` itself, with the names of the variables passed beside it because
 * `daemon.toml`'s `[daemon.env] passthrough_keys` lists them (N-DAEMON-2),
 * never their values; `null` for a daemon the app did not spawn this run. A
 * refusal of the shell's is typed, `{ kind, message }`
 * (`lib/shell-error.ts`, Q13): it becomes a `ConnectionError` carrying that
 * `kind` and the shell's sentence as it is.
 *
 * `instanceId` is the identity guard: a change means the daemon restarted, so
 * every `task_id`, `stream_id` and `request_id` the client holds is dead and
 * the app must fully re-bootstrap rather than merely reopen its socket.
 */

import { invoke } from "@tauri-apps/api/core";

import { shellErrorOf, type ShellErrorKind } from "./shell-error";

export interface ConnectionInfo {
  baseUrl: string;
  token: string;
  instanceId: string;
  /**
   * Where the `PATH` of the daemon this app spawned came from (Q21); `null`
   * for a daemon this run of the app did not spawn. Absent only in a test's
   * hand-written `ConnectionInfo`.
   */
  daemonPath?: DaemonPath | null;
  /**
   * The daemon's `build.version` and `build.protocol` (Q04); `null` from a
   * shell that does not send them. Absent only in a test's hand-written
   * `ConnectionInfo`.
   */
  version?: string | null;
  protocol?: number | null;
  /**
   * The shell's sentence on a daemon it uses of an older protocol or another
   * version — both named, and Stop then Start, never a restart — `null` for
   * one of this app's (Q04's seventh row, A-20).
   */
  note?: string | null;
}

/**
 * The shell's `daemonPath` (`src-tauri/src/login_path.rs`'s
 * `DaemonPathReport`), pinned from both sides by `daemon-path.json`.
 */
export interface DaemonPath {
  /**
   * `login-shell`: the login shell's; `login-shell-noninteractive`: the
   * login shell's, read again without `-i` because, run interactively, it
   * ended before it printed its environment (CAPTURE-RETRY);
   * `fallback-list`: the fixed list.
   */
  source: "login-shell" | "login-shell-noninteractive" | "fallback-list";
  /** How long the capture took, from the app's start. */
  captureMs: number;
  /** Why the capture failed, the shell's sentence; `null` but for `fallback-list`. */
  reason: string | null;
  /**
   * The variables passed beside `PATH` from the login shell, by name, as
   * `[daemon.env] passthrough_keys` lists them (N-DAEMON-2); empty for none.
   */
  passedKeys: string[];
}

/** `value` as a `DaemonPath`, or `null` when it is not one. */
export function daemonPathOf(value: unknown): DaemonPath | null {
  if (typeof value !== "object" || value === null) return null;
  const v = value as Record<string, unknown>;
  if (
    v.source !== "login-shell" &&
    v.source !== "login-shell-noninteractive" &&
    v.source !== "fallback-list"
  ) {
    return null;
  }
  if (typeof v.captureMs !== "number" || !Number.isFinite(v.captureMs)) {
    return null;
  }
  if (v.reason !== null && typeof v.reason !== "string") return null;
  if (
    !Array.isArray(v.passedKeys) ||
    !v.passedKeys.every((key) => typeof key === "string")
  ) {
    return null;
  }
  return {
    source: v.source,
    captureMs: v.captureMs,
    reason: v.reason,
    passedKeys: [...(v.passedKeys as string[])],
  };
}

/** Thrown when discovery is missing/expired or the Tauri bridge is absent. */
export class ConnectionError extends Error {
  override readonly name = "ConnectionError";

  constructor(
    message: string,
    /** The underlying Tauri rejection or malformed payload. */
    readonly detail: unknown = null,
    /**
     * What refused, when the shell did (its typed refusal's `kind`); `null`
     * for Tauri's own refusal, an absent bridge or a malformed payload.
     */
    readonly kind: ShellErrorKind | null = null,
  ) {
    super(message);
  }
}

/**
 * A command's rejection as a `ConnectionError`: the shell's typed refusal
 * keeps its `kind` and its sentence; Tauri's own refusal is a bare string,
 * kept as it is with no `kind`; anything else is named by the command.
 */
function refusalOf(command: string, cause: unknown): ConnectionError {
  const shell = shellErrorOf(cause);
  if (shell) return new ConnectionError(shell.message, cause, shell.kind);
  return new ConnectionError(
    typeof cause === "string" ? cause : `Tauri command \`${command}\` failed`,
    cause,
  );
}

export type ConnectionListener = (info: ConnectionInfo | null) => void;
export type InstanceChangeListener = (
  next: ConnectionInfo,
  previous: ConnectionInfo,
) => void;

let current: ConnectionInfo | null = null;
/**
 * The connection a read on reconnect found replaced, kept while no
 * connection is cached until a read lands, so that read announces the
 * change: the re-bootstrap the read starts can fail first — since Q04 the
 * shell answers `starting` at once while the new daemon boots — and the
 * window then asks again through a boot or a rung of its own, which would
 * otherwise see no previous connection and announce nothing.
 */
let replaced: ConnectionInfo | null = null;
/** When `current` was read, by the wall clock (`Date.now()`); `null` with it. */
let readAt: number | null = null;
let bootstrapInFlight: Promise<ConnectionInfo> | null = null;
let refreshInFlight: Promise<ConnectionInfo> | null = null;

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
  readAt = info === null ? null : Date.now();
  for (const listener of connectionListeners) listener(info);
}

async function invokeConnection(
  command: "get_connection_info" | "ensure_daemon_running",
) {
  let raw: unknown;
  try {
    raw = await invoke(command);
  } catch (cause) {
    throw refusalOf(command, cause);
  }
  if (!isConnectionInfo(raw)) {
    throw new ConnectionError(
      `\`${command}\` returned an unexpected payload`,
      raw,
    );
  }
  const v = raw as unknown as Record<string, unknown>;
  return {
    baseUrl: raw.baseUrl,
    token: raw.token,
    instanceId: raw.instanceId,
    daemonPath: daemonPathOf(raw.daemonPath),
    version: typeof v.version === "string" ? v.version : null,
    protocol:
      typeof v.protocol === "number" && Number.isFinite(v.protocol)
        ? v.protocol
        : null,
    note: typeof v.note === "string" ? v.note : null,
  };
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
    throw refusalOf("read_daemon_log_tail", cause);
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
  // A phase belongs to one stop; a new intent (or none) starts from nothing.
  setStopPhase(null);
  for (const listener of stopIntentListeners) listener(next);
}

export function subscribeStopIntent(
  listener: (intent: StopIntent) => void,
): () => void {
  stopIntentListeners.add(listener);
  return () => stopIntentListeners.delete(listener);
}

/**
 * How far this window's own stop has got — meaningful only while the intent
 * is `"stopped_here"`, and reset whenever the intent changes.
 *
 * `"stopping"` — from the shutdown POST until `await_daemon_stopped`
 *                answers. The process may still hold its lock for up to 15 s,
 *                so nothing may start a daemon yet: a replacement spawned now
 *                loses the singleton-lock race and exits.
 * the rest     — what that wait concluded (`StopResult["kind"]` for a stop
 *                the daemon accepted). `"stopped"` covers `not_running` too.
 */
export type StopPhase =
  | "stopping"
  | "stopped"
  | "still_alive"
  | "lock_still_held"
  | "unconfirmed"
  | null;

let stopPhase: StopPhase = null;
const stopPhaseListeners = new Set<(phase: StopPhase) => void>();

export function getStopPhase(): StopPhase {
  return stopPhase;
}

export function setStopPhase(next: StopPhase): void {
  if (next === stopPhase) return;
  stopPhase = next;
  for (const listener of stopPhaseListeners) listener(next);
}

export function subscribeStopPhase(
  listener: (phase: StopPhase) => void,
): () => void {
  stopPhaseListeners.add(listener);
  return () => stopPhaseListeners.delete(listener);
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
    throw refusalOf("await_daemon_stopped", cause);
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
 * Subscribe to daemon-identity changes. Fired once when a read observes a
 * different `instanceId` than the connection it replaces — on reconnect, a
 * read that lands after the re-bootstrap failed included — so consumers must
 * drop all server-derived state.
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
      adopt(info);
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
 *
 * Also the path a request refused `401` takes (`lib/http.ts`): the daemon
 * rotates the token while it runs (T21). Concurrent callers share one read.
 */
export function refreshConnection(): Promise<ConnectionInfo> {
  if (refreshInFlight) return refreshInFlight;
  refreshInFlight = rereadConnection().finally(() => {
    refreshInFlight = null;
  });
  return refreshInFlight;
}

async function rereadConnection(): Promise<ConnectionInfo> {
  const previous = current ?? replaced;
  const info = await invokeConnection("get_connection_info");

  if (previous && previous.instanceId !== info.instanceId) {
    // Kept until a read lands: the boot below, or a later one if it fails.
    replaced = previous;
    publish(null);
    return bootstrapConnection();
  }

  adopt(info);
  return info;
}

/**
 * Cache `info`, and announce the change when it is another daemon than the
 * connection it replaces: the cached one, or the one a read on reconnect
 * found replaced (`replaced`).
 */
function adopt(info: ConnectionInfo): void {
  const previous = current ?? replaced;
  replaced = null;
  publish(info);
  if (previous && previous.instanceId !== info.instanceId) {
    for (const listener of instanceListeners) listener(info, previous);
  }
}

/** The cached connection, bootstrapping on first use. */
export async function ensureConnection(): Promise<ConnectionInfo> {
  return current ?? (await bootstrapConnection());
}

/**
 * Drop the cached connection, and a change not yet announced with it (tests,
 * and an explicit user-driven reconnect).
 */
export function resetConnection(): void {
  replaced = null;
  publish(null);
}

// ── Keeping the token fresh (TOKEN-REREAD) ──────────────────────────────────

/**
 * How old the cached connection may grow, by the wall clock, before the
 * window reads it again: one hour.
 *
 * The daemon renews its token twelve hours after its issue and honours the
 * one it replaced until that one's `expires_at`, a day after its issue (T21),
 * so a token is good for at least twelve hours after it was read. A URL that
 * carries the token in its query — a content URL an `<img>` loads, the chat
 * stream's `EventSource`, the events socket's — cannot answer a `401` by
 * reading the connection again as `apiRequest` does; it carries whatever the
 * cache held when it was built. Reading the cache again well inside those
 * twelve hours keeps every such URL on a token the daemon honours
 * (`tasks/pending-decisions.md` §1 _Rev 93_ (b)).
 */
export const CONNECTION_REREAD_AFTER_MS = 60 * 60 * 1000;

/** How often the window looks at the cache's age: once a minute. */
export const CONNECTION_CHECK_EVERY_MS = 60 * 1000;

/**
 * Read the connection again if the cache is due: `CONNECTION_REREAD_AFTER_MS`
 * old or more by the wall clock, or read in the future (the clock was set
 * back). Nothing while there is no cache (the first read is the boot's),
 * while a boot is in flight, or while the window has no daemon by intent: a
 * read that finds another instance re-bootstraps, and
 * `ensure_daemon_running` would start a daemon the user stopped. A failed
 * read keeps the cache and its stamp, so the next look tries again; the
 * refusal itself is the socket's and `apiRequest`'s to report.
 */
function rereadIfDue(): void {
  if (current === null || readAt === null) return;
  if (bootstrapInFlight !== null) return;
  if (getStopIntent() !== null) return;
  const age = Date.now() - readAt;
  if (age >= 0 && age < CONNECTION_REREAD_AFTER_MS) return;
  refreshConnection().catch(() => undefined);
}

/**
 * Keep the cached connection's token fresh while the window is open: look
 * at its age every `CONNECTION_CHECK_EVERY_MS`, and at once when the window
 * is shown (never while it is hidden) or focused again. Returns the function
 * that stops it.
 *
 * Judged on the wall clock, not by a timer of hours: a Mac's sleep stops the
 * webview's timers but not `Date.now()`, so a night's sleep is counted at the
 * first look after the wake — at most one interval, plus the read, during
 * which a URL built from the cache may still carry a lapsed token.
 */
export function keepConnectionFresh(): () => void {
  // Listeners of this start's own, so one start's stop never removes
  // another's (the DOM keeps one copy of a listener added twice).
  const onTick = () => rereadIfDue();
  const onFocus = () => rereadIfDue();
  const onVisible = () => {
    if (document.visibilityState !== "hidden") rereadIfDue();
  };
  const timer = setInterval(onTick, CONNECTION_CHECK_EVERY_MS);
  document.addEventListener("visibilitychange", onVisible);
  window.addEventListener("focus", onFocus);
  return () => {
    clearInterval(timer);
    document.removeEventListener("visibilitychange", onVisible);
    window.removeEventListener("focus", onFocus);
  };
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
