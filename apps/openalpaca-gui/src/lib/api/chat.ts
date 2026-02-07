/**
 * REST API client for chat endpoints.
 */

import { get } from "svelte/store";
import { connectionInfo, type ConnectionInfo } from "../daemon";
import type {
  ChatSendRequest,
  ChatSendResponse,
  ChatHistoryResponse,
  ChatDeleteResponse,
} from "../types";

async function ensureConnection(): Promise<ConnectionInfo> {
  const conn = get(connectionInfo);
  if (!conn) throw new Error("Not connected to daemon");
  return conn;
}

/** POST /v1/chat — send a message */
export async function sendMessage(req: ChatSendRequest): Promise<ChatSendResponse> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/chat`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${conn.token}`,
    },
    body: JSON.stringify(req),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to send message: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/chat/history — fetch conversation history */
export async function getChatHistory(limit?: number, offset?: number, laneKey?: string): Promise<ChatHistoryResponse> {
  const conn = await ensureConnection();
  const params = new URLSearchParams();
  if (limit !== undefined) params.set("limit", String(limit));
  if (offset !== undefined) params.set("offset", String(offset));
  if (laneKey !== undefined) params.set("lane_key", laneKey);
  const qs = params.toString();

  const response = await fetch(
    `${conn.baseUrl}/v1/chat/history${qs ? `?${qs}` : ""}`,
    { headers: { Authorization: `Bearer ${conn.token}` } },
  );

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch history: ${response.statusText}`);
  }
  return await response.json();
}

/** DELETE /v1/chat/history — clear conversation history */
export async function clearChatHistory(): Promise<ChatDeleteResponse> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/chat/history`, {
    method: "DELETE",
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to clear history: ${response.statusText}`);
  }
  return await response.json();
}

/** Create an EventSource for SSE chat streaming */
export function createChatStream(streamId: string): EventSource | null {
  const conn = get(connectionInfo);
  if (!conn) return null;

  const url = `${conn.baseUrl}/v1/chat/stream/${encodeURIComponent(streamId)}?token=${encodeURIComponent(conn.token)}`;
  return new EventSource(url);
}
