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
  | { type: "heartbeat"; ts: string; instance_id: string }
  | { type: "log"; level: string; message: string; ts: string; instance_id: string }
  | { type: "command_received"; request_id: string; command: string; ts: string; instance_id: string }
  | { type: "wake"; wake: unknown; ts: string; instance_id: string }
  | { type: "connector_status"; id: string; status: string; ts: string; instance_id: string }
  | { type: "task_status"; task_id: string; title: string; status: string; progress_current: number | null; progress_total: number | null; result_summary: string | null; ts: string; instance_id: string }
  | { type: "agent_status"; agent_id: string; name: string; status: string; current_task_id: string | null; ts: string; instance_id: string };

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

/**
 * Connect to daemon's WebSocket events endpoint
 */
export async function connectToDaemon(): Promise<void> {
  connectionState.set("connecting");
  errorMessage.set(null);

  try {
    // Ensure daemon is running and get connection info
    const info: ConnectionInfo = await invoke("ensure_daemon_running");
    connectionInfo.set(info);
    currentInstanceId = info.instanceId;

    // Connect to WebSocket
    const wsUrl = info.baseUrl.replace("http", "ws");
    ws = new WebSocket(`${wsUrl}/v1/events?token=${encodeURIComponent(info.token)}`);

    ws.onopen = () => {
      connectionState.set("connected");
      errorMessage.set(null);
      console.log("[daemon] WebSocket connected");
    };

    ws.onmessage = (event) => {
      try {
        const data: ServerEvent = JSON.parse(event.data);
        events.update((list) => {
          const updated = [data, ...list];
          // Keep last 100 events
          return updated.slice(0, 100);
        });
      } catch (e) {
        console.error("[daemon] Failed to parse event:", e);
      }
    };

    ws.onerror = () => {
      connectionState.set("error");
      errorMessage.set("WebSocket connection error");
    };

    ws.onclose = async () => {
      connectionState.set("disconnected");
      console.log("[daemon] WebSocket closed");
      scheduleReconnect();
    };
  } catch (e) {
    connectionState.set("error");
    errorMessage.set(String(e));
    scheduleReconnect();
  }
}

/**
 * Disconnect from daemon
 */
export function disconnect(): void {
  if (reconnectTimeout) {
    clearTimeout(reconnectTimeout);
    reconnectTimeout = null;
  }
  if (ws) {
    ws.close();
    ws = null;
  }
  connectionState.set("disconnected");
}

/**
 * Schedule reconnection attempt
 */
function scheduleReconnect(): void {
  if (reconnectTimeout) return;

  reconnectTimeout = setTimeout(async () => {
    reconnectTimeout = null;

    try {
      // Check if instance changed
      const info: ConnectionInfo = await invoke("get_connection_info");
      if (info.instanceId !== currentInstanceId) {
        console.log("[daemon] Instance changed, re-bootstrapping");
        await connectToDaemon();
      } else {
        // Same instance, just reconnect WS
        const wsUrl = info.baseUrl.replace("http", "ws");
        ws = new WebSocket(`${wsUrl}/v1/events?token=${encodeURIComponent(info.token)}`);
        ws.onopen = () => connectionState.set("connected");
        ws.onmessage = (event) => {
          const data: ServerEvent = JSON.parse(event.data);
          events.update((list) => [data, ...list].slice(0, 100));
        };
        ws.onclose = () => {
          connectionState.set("disconnected");
          scheduleReconnect();
        };
      }
    } catch {
      scheduleReconnect();
    }
  }, 1000);
}

/**
 * Clear all events
 */
export function clearEvents(): void {
  events.set([]);
}
