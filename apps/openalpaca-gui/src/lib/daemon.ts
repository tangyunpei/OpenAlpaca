/**
 * Daemon connection utilities for OpenAlpaca GUI
 *
 * Handles WebSocket connection to daemon's /v1/events endpoint
 * with automatic reconnection support.
 */

import { invoke } from "@tauri-apps/api/core";
import { writable, type Writable } from "svelte/store";

/** Connection info returned from daemon discovery (camelCase from Rust) */
export interface ConnectionInfo {
  baseUrl: string;
  token: string;
  instanceId: string;
}

/** Server event types from daemon — discriminated union matching Rust ServerEvent enum */
export type ServerEvent =
  | { type: "heartbeat"; ts: string; instance_id: string; _id: number }
  | { type: "log"; level: string; message: string; ts: string; instance_id: string; _id: number }
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
  | { type: "soul_updated"; actor: string; mode: string; content_sha256: string; backup_path: string | null; ts: string; instance_id: string; _id: number }
  | { type: "context_package_built"; agent_id: string; sections: [string, number][]; total_tokens: number; budget: number; sub_agent_window: number; ts: string; instance_id: string; _id: number }
  | { type: "daemon_config_changed"; ts: string; instance_id: string; _id: number }
  | { type: "workflow_started"; task_id: string; lane_key: string; title: string; ts: string; instance_id: string; _id: number }
  | { type: "workflow_steered"; task_id: string; lane_key: string; ts: string; instance_id: string; _id: number }
  | { type: "workflow_progress"; task_id: string; lane_key: string; message: string; ts: string; instance_id: string; _id: number }
  | { type: "followup_queued"; lane_key: string; followup_id: number; kind: string; ts: string; instance_id: string; _id: number }
  | { type: "plugin_loaded"; plugin_id: string; tools: string[]; _id: number }
  | { type: "plugin_unloaded"; plugin_id: string; _id: number }
  | { type: "plugin_crashed"; plugin_id: string; error: string; restart_in_secs: number; _id: number }
  | { type: "plugin_disabled"; plugin_id: string; reason: string; _id: number }
  | { type: "plugin_pending_approval"; plugin_id: string; capabilities: string[]; _id: number }
  | { type: "plugin_needs_config"; plugin_id: string; missing_keys: string[]; _id: number };

/** Connection state */
export type ConnectionState = "disconnected" | "connecting" | "connected" | "error";

/** Store for connection state */
export const connectionState: Writable<ConnectionState> = writable("disconnected");

/** Store for received events (last 100) */
export const events: Writable<ServerEvent[]> = writable([]);

/** Store for current connection info */
export const connectionInfo: Writable<ConnectionInfo | null> = writable(null);

/** Store for error messages */
export const errorMessage: Writable<string | null> = writable(null);

let ws: WebSocket | null = null;
let reconnectTimeout: ReturnType<typeof setTimeout> | null = null;
let currentInstanceId: string | null = null;
let reconnectEnabled = true;
let backoffMs = 1000;
let nextEventId = 0;

const BACKOFF_BASE = 1000;
const BACKOFF_MAX = 30000;

/**
 * Tear down the current WebSocket connection.
 * Nulls handlers BEFORE calling close() to prevent onclose → scheduleReconnect.
 */
function teardownWebSocket(): void {
  if (ws) {
    ws.onopen = null;
    ws.onmessage = null;
    ws.onerror = null;
    ws.onclose = null;
    ws.close();
    ws = null;
  }
}

/**
 * Set up standard WS event handlers on the given WebSocket.
 */
function setupWsHandlers(socket: WebSocket): void {
  socket.onopen = () => {
    connectionState.set("connected");
    errorMessage.set(null);
    backoffMs = BACKOFF_BASE; // reset backoff on successful connect
    console.log("[daemon] WebSocket connected");
  };

  socket.onmessage = (event) => {
    try {
      const data = JSON.parse(event.data);
      data._id = nextEventId++;
      events.update((list) => {
        const updated = [data as ServerEvent, ...list];
        return updated.slice(0, 100);
      });
    } catch (e) {
      console.error("[daemon] Failed to parse event:", e);
    }
  };

  socket.onerror = () => {
    connectionState.set("error");
    errorMessage.set("WebSocket connection error");
  };

  socket.onclose = () => {
    connectionState.set("disconnected");
    console.log("[daemon] WebSocket closed");
    if (reconnectEnabled) {
      scheduleReconnect();
    }
  };
}

/**
 * Connect to daemon's WebSocket events endpoint
 */
export async function connectToDaemon(): Promise<void> {
  teardownWebSocket();
  if (reconnectTimeout) {
    clearTimeout(reconnectTimeout);
    reconnectTimeout = null;
  }
  reconnectEnabled = true;
  backoffMs = BACKOFF_BASE;
  connectionState.set("connecting");
  errorMessage.set(null);

  try {
    const info: ConnectionInfo = await invoke("ensure_daemon_running");
    connectionInfo.set(info);
    currentInstanceId = info.instanceId;

    const wsUrl = info.baseUrl.replace("http", "ws");
    ws = new WebSocket(`${wsUrl}/v1/events?token=${encodeURIComponent(info.token)}`);
    setupWsHandlers(ws);
  } catch (e) {
    connectionState.set("error");
    errorMessage.set(String(e));
    if (reconnectEnabled) {
      scheduleReconnect();
    }
  }
}

/**
 * Disconnect from daemon. Prevents reconnection.
 */
export function disconnect(): void {
  reconnectEnabled = false;
  if (reconnectTimeout) {
    clearTimeout(reconnectTimeout);
    reconnectTimeout = null;
  }
  teardownWebSocket();
  connectionState.set("disconnected");
}

/**
 * Schedule reconnection attempt with exponential backoff and jitter.
 * Base 1s, max 30s, +/-20% jitter. Resets on successful onopen.
 */
function scheduleReconnect(): void {
  if (reconnectTimeout) return;
  if (!reconnectEnabled) return;

  // Apply jitter: +/-20%
  const jitter = backoffMs * (0.8 + Math.random() * 0.4);
  const delay = Math.min(jitter, BACKOFF_MAX);

  reconnectTimeout = setTimeout(async () => {
    reconnectTimeout = null;
    if (!reconnectEnabled) return;

    try {
      const info: ConnectionInfo = await invoke("get_connection_info");
      if (info.instanceId !== currentInstanceId) {
        console.log("[daemon] Instance changed, re-bootstrapping");
        await connectToDaemon();
      } else {
        // Same instance, just reconnect WS
        teardownWebSocket();
        const wsUrl = info.baseUrl.replace("http", "ws");
        ws = new WebSocket(`${wsUrl}/v1/events?token=${encodeURIComponent(info.token)}`);
        setupWsHandlers(ws);
      }
    } catch {
      if (reconnectEnabled) {
        scheduleReconnect();
      }
    }

    // Increase backoff for next attempt
    backoffMs = Math.min(backoffMs * 2, BACKOFF_MAX);
  }, delay);
}

/**
 * Clear all events
 */
export function clearEvents(): void {
  events.set([]);
}
