/**
 * WebSocket client for the daemon's `/v1/events` firehose (API_MAP §4.2).
 *
 * The `ServerEvent` union below is ported verbatim from the retired SvelteKit
 * client (`.daemon-legacy-reference.ts`), which tracks
 * `crates/openalpaca_api/src/events/mod.rs`. Two quirks are preserved because
 * they are real: the six `plugin_*` variants carry no `ts`/`instance_id`
 * (GAP-22), and `task_status.title` / `agent_status.name` arrive empty on
 * updates (GAP-07).
 *
 * **The socket is best-effort.** On `RecvError::Lagged(n)` the server logs and
 * continues, so a slow client silently loses `n` events with no notification,
 * and there is no replay on reconnect. Every consumer therefore needs a REST
 * refetch path: subscribe to `onResync` and invalidate.
 */

import {
  bootstrapConnection,
  refreshConnection,
  wsUrl,
  type ConnectionInfo,
} from "./connection";

// ── Event union ─────────────────────────────────────────────────────────────

/**
 * One `ServerEvent` frame, tagged with a monotonic local `_id` on receipt.
 *
 * `prettier-ignore` keeps one variant per line, so this stays diffable against
 * the Rust enum it mirrors.
 */
// prettier-ignore
export type ServerEvent =
  | { type: "heartbeat"; ts: string; instance_id: string; _id: number }
  | { type: "command_received"; request_id: string; command: string; ts: string; instance_id: string; _id: number }
  | { type: "wake"; wake: unknown; ts: string; instance_id: string; _id: number }
  | { type: "connector_status"; id: string; status: string; ts: string; instance_id: string; _id: number }
  | { type: "task_status"; task_id: string; title: string; status: string; progress_current: number | null; progress_total: number | null; result_summary: string | null; outcome_kind?: string; artifact_count?: number; outcome_summary?: string; ts: string; instance_id: string; _id: number }
  | { type: "agent_status"; agent_id: string; name: string; status: string; current_task_id: string | null; agent_instance_id: string; template_id: string; ts: string; instance_id: string; _id: number }
  | { type: "key_status_changed"; provider: string; key_id: string; status: string; ts: string; instance_id: string; _id: number }
  | { type: "chat_stream_started"; stream_id: string; lane_key: string; ts: string; instance_id: string; _id: number }
  | { type: "chat_stream_ended"; stream_id: string; lane_key: string; status: string; ts: string; instance_id: string; _id: number }
  | { type: "agent_config_changed"; agent_id: string; action: string; config_version: number; ts: string; instance_id: string; _id: number }
  | { type: "orchestrator_config_changed"; model: string; ts: string; instance_id: string; _id: number }
  | { type: "dag_node_status"; task_id: string; node_id: string; node_title: string; agent_id: string; status: string; duration_ms: number | null; output_preview: string | null; ts: string; instance_id: string; _id: number }
  | { type: "security_violation"; agent_id: string; tool_name: string; reason: string; ts: string; instance_id: string; _id: number }
  | { type: "circuit_breaker_tripped"; agent_id: string; tool_name: string; consecutive_failures: number; reset_after_secs: number; ts: string; instance_id: string; _id: number }
  | { type: "tool_executed"; agent_id: string; tool_name: string; success: boolean; duration_ms: number; ts: string; instance_id: string; _id: number }
  | { type: "llm_call_completed"; agent_id: string; model: string; input_tokens: number; output_tokens: number; cost_usd: number; ts: string; instance_id: string; _id: number }
  | { type: "skill_catalog_updated"; skill_name: string; action: string; ts: string; instance_id: string; _id: number }
  | { type: "skill_invocation_started"; request_id: string; skill_id: string; query_preview: string; ts: string; instance_id: string; _id: number }
  | { type: "skill_completed"; request_id: string; skill_id: string; duration_ms: number; output_preview: string; ts: string; instance_id: string; _id: number }
  | { type: "skill_failed"; request_id: string; skill_id: string; error: string; ts: string; instance_id: string; _id: number }
  | { type: "tool_confirmation_requested"; request_id: string; agent_id: string; tool_name: string; tool_arguments: unknown; stream_id: string | null; lane_key: string | null; ts: string; instance_id: string; _id: number }
  | { type: "soul_updated"; actor: string; mode: string; content_sha256: string; backup_path: string | null; ts: string; instance_id: string; _id: number }
  | { type: "daemon_config_changed"; ts: string; instance_id: string; _id: number }
  | { type: "workflow_started"; task_id: string; lane_key: string; title: string; ts: string; instance_id: string; _id: number }
  | { type: "workflow_steered"; task_id: string; lane_key: string; ts: string; instance_id: string; _id: number }
  | { type: "workflow_progress"; task_id: string; lane_key: string; message: string; ts: string; instance_id: string; _id: number }
  | { type: "followup_queued"; lane_key: string; followup_id: number; kind: string; ts: string; instance_id: string; _id: number }
  // GAP-22: the six plugin variants carry neither `ts` nor `instance_id`.
  | { type: "plugin_loaded"; plugin_id: string; tools: string[]; _id: number }
  | { type: "plugin_unloaded"; plugin_id: string; _id: number }
  | { type: "plugin_crashed"; plugin_id: string; error: string; restart_in_secs: number; _id: number }
  | { type: "plugin_disabled"; plugin_id: string; reason: string; _id: number }
  | { type: "plugin_pending_approval"; plugin_id: string; capabilities: string[]; _id: number }
  | { type: "plugin_needs_config"; plugin_id: string; missing_keys: string[]; _id: number };

export type ServerEventType = ServerEvent["type"];

// ── Connection status ───────────────────────────────────────────────────────

export type EventsStatus =
  "idle" | "connecting" | "connected" | "disconnected" | "error";

/**
 * "You may have missed events" — the only honest signal available, since the
 * server never tells a lagged client that it dropped frames. Emitted whenever
 * the socket comes back after having been down, and whenever the daemon
 * identity changed underneath us.
 */
export interface ResyncSignal {
  reason: "reconnected" | "instance_changed";
  /** Wall-clock ms the socket spent disconnected, when known. */
  offlineMs: number | null;
}

// ── Backoff ─────────────────────────────────────────────────────────────────

export const BACKOFF_BASE_MS = 1000;
export const BACKOFF_MAX_MS = 30000;

/**
 * Jittered delay for the next reconnect attempt: `backoff × [0.8, 1.2)`,
 * clamped to `BACKOFF_MAX_MS`. Pure, so the schedule is testable.
 */
export function jitteredDelay(backoffMs: number, random: () => number): number {
  const jittered = backoffMs * (0.8 + random() * 0.4);
  return Math.min(jittered, BACKOFF_MAX_MS);
}

/** Double the backoff, capped. */
export function nextBackoff(backoffMs: number): number {
  return Math.min(backoffMs * 2, BACKOFF_MAX_MS);
}

// ── Client ──────────────────────────────────────────────────────────────────

/** The slice of `WebSocket` this client uses, so tests can supply a double. */
export interface SocketLike {
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  onclose: ((event: unknown) => void) | null;
  close(): void;
}

export interface EventsClientDeps {
  /** Boot path — spawns the daemon if needed. */
  bootstrap: () => Promise<ConnectionInfo>;
  /** Reconnect path — re-reads discovery, re-bootstraps on instance change. */
  refresh: () => Promise<ConnectionInfo>;
  createSocket: (url: string) => SocketLike;
  random: () => number;
  now: () => number;
  /** How many events to retain for the Event log surfaces. */
  ringSize: number;
}

const defaultDeps: EventsClientDeps = {
  bootstrap: bootstrapConnection,
  refresh: refreshConnection,
  createSocket: (url) => new WebSocket(url) as unknown as SocketLike,
  random: Math.random,
  now: Date.now,
  ringSize: 500,
};

export class DaemonEventsClient {
  private readonly deps: EventsClientDeps;

  private socket: SocketLike | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectEnabled = false;
  private backoffMs = BACKOFF_BASE_MS;
  private nextEventId = 0;
  private instanceId: string | null = null;
  private everConnected = false;
  private disconnectedAt: number | null = null;

  private status: EventsStatus = "idle";
  private lastError: string | null = null;
  private ring: ServerEvent[] = [];

  private readonly eventListeners = new Set<(event: ServerEvent) => void>();
  private readonly statusListeners = new Set<(status: EventsStatus) => void>();
  private readonly resyncListeners = new Set<(signal: ResyncSignal) => void>();

  constructor(deps: Partial<EventsClientDeps> = {}) {
    this.deps = { ...defaultDeps, ...deps };
  }

  getStatus(): EventsStatus {
    return this.status;
  }

  getLastError(): string | null {
    return this.lastError;
  }

  /** Newest-first ring of received events. */
  getEvents(): readonly ServerEvent[] {
    return this.ring;
  }

  onEvent(listener: (event: ServerEvent) => void): () => void {
    this.eventListeners.add(listener);
    return () => this.eventListeners.delete(listener);
  }

  onStatus(listener: (status: EventsStatus) => void): () => void {
    this.statusListeners.add(listener);
    return () => this.statusListeners.delete(listener);
  }

  /** Fires when the client may have missed events — refetch on this. */
  onResync(listener: (signal: ResyncSignal) => void): () => void {
    this.resyncListeners.add(listener);
    return () => this.resyncListeners.delete(listener);
  }

  /** Open the socket, bootstrapping the daemon if it is not running. */
  async connect(): Promise<void> {
    this.teardownSocket();
    this.clearTimer();
    this.reconnectEnabled = true;
    this.backoffMs = BACKOFF_BASE_MS;
    this.setStatus("connecting");

    try {
      const info = await this.deps.bootstrap();
      this.adoptInstance(info.instanceId);
      this.openSocket(info);
    } catch (error) {
      this.lastError = error instanceof Error ? error.message : String(error);
      this.setStatus("error");
      this.scheduleReconnect();
    }
  }

  /** Close the socket and stop reconnecting. */
  disconnect(): void {
    this.reconnectEnabled = false;
    this.clearTimer();
    this.teardownSocket();
    this.setStatus("disconnected");
  }

  clearEvents(): void {
    this.ring = [];
  }

  // ── internals ─────────────────────────────────────────────────────────────

  private setStatus(status: EventsStatus): void {
    if (this.status === status) return;
    this.status = status;
    for (const listener of this.statusListeners) listener(status);
  }

  private adoptInstance(instanceId: string): void {
    if (this.instanceId !== null && this.instanceId !== instanceId) {
      this.ring = [];
      this.emitResync("instance_changed");
    }
    this.instanceId = instanceId;
  }

  private emitResync(reason: ResyncSignal["reason"]): void {
    const offlineMs =
      this.disconnectedAt === null
        ? null
        : this.deps.now() - this.disconnectedAt;
    const signal: ResyncSignal = { reason, offlineMs };
    for (const listener of this.resyncListeners) listener(signal);
  }

  private openSocket(info: ConnectionInfo): void {
    const socket = this.deps.createSocket(wsUrl(info, "/v1/events"));
    this.socket = socket;

    socket.onopen = () => {
      this.lastError = null;
      this.backoffMs = BACKOFF_BASE_MS;
      this.setStatus("connected");
      // A first connect cannot have missed anything; every later one can.
      if (this.everConnected) this.emitResync("reconnected");
      this.everConnected = true;
      this.disconnectedAt = null;
    };

    socket.onmessage = (event) => {
      if (typeof event.data !== "string") return;
      let parsed: unknown;
      try {
        parsed = JSON.parse(event.data);
      } catch {
        return;
      }
      if (typeof parsed !== "object" || parsed === null) return;
      if (typeof (parsed as { type?: unknown }).type !== "string") return;

      const tagged = {
        ...(parsed as object),
        _id: this.nextEventId++,
      } as ServerEvent;
      this.ring = [tagged, ...this.ring].slice(0, this.deps.ringSize);
      for (const listener of this.eventListeners) listener(tagged);
    };

    socket.onerror = () => {
      this.lastError = "WebSocket connection error";
      this.setStatus("error");
    };

    socket.onclose = () => {
      if (this.disconnectedAt === null) this.disconnectedAt = this.deps.now();
      this.setStatus("disconnected");
      if (this.reconnectEnabled) this.scheduleReconnect();
    };
  }

  /**
   * Null every handler *before* `close()`, or `onclose` re-enters
   * `scheduleReconnect` and we race two sockets.
   */
  private teardownSocket(): void {
    const socket = this.socket;
    if (!socket) return;
    socket.onopen = null;
    socket.onmessage = null;
    socket.onerror = null;
    socket.onclose = null;
    socket.close();
    this.socket = null;
  }

  private clearTimer(): void {
    if (this.reconnectTimer === null) return;
    clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null || !this.reconnectEnabled) return;
    if (this.disconnectedAt === null) this.disconnectedAt = this.deps.now();

    const delay = jitteredDelay(this.backoffMs, this.deps.random);

    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.backoffMs = nextBackoff(this.backoffMs);
      if (!this.reconnectEnabled) return;

      void this.deps
        .refresh()
        .then((info) => {
          if (!this.reconnectEnabled) return;
          this.adoptInstance(info.instanceId);
          this.teardownSocket();
          this.setStatus("connecting");
          this.openSocket(info);
        })
        .catch(() => {
          this.scheduleReconnect();
        });
    }, delay);
  }
}

/** The app-wide events client. Views read it through `useDaemonEvents`. */
export const daemonEvents = new DaemonEventsClient();
