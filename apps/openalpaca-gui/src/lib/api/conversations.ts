/**
 * REST API client for cross-platform conversation endpoints.
 */

import { get } from "svelte/store";
import { connectionInfo, type ConnectionInfo } from "../daemon";
import type { ConversationsResponse, ConversationMessagesResponse } from "../types";

async function ensureConnection(): Promise<ConnectionInfo> {
  const conn = get(connectionInfo);
  if (!conn) throw new Error("Not connected to daemon");
  return conn;
}

/** GET /v1/conversations — list conversations across all sources */
export async function listConversations(
  source?: string,
  limit?: number,
  offset?: number,
): Promise<ConversationsResponse> {
  const conn = await ensureConnection();
  const params = new URLSearchParams();
  if (source !== undefined) params.set("source", source);
  if (limit !== undefined) params.set("limit", String(limit));
  if (offset !== undefined) params.set("offset", String(offset));
  const qs = params.toString();

  const response = await fetch(
    `${conn.baseUrl}/v1/conversations${qs ? `?${qs}` : ""}`,
    { headers: { Authorization: `Bearer ${conn.token}` } },
  );

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch conversations: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/conversations/{id}/messages — get messages for a conversation */
export async function getConversationMessages(
  conversationId: string,
  limit?: number,
  offset?: number,
): Promise<ConversationMessagesResponse> {
  const conn = await ensureConnection();
  const params = new URLSearchParams();
  if (limit !== undefined) params.set("limit", String(limit));
  if (offset !== undefined) params.set("offset", String(offset));
  const qs = params.toString();

  const response = await fetch(
    `${conn.baseUrl}/v1/conversations/${encodeURIComponent(conversationId)}/messages${qs ? `?${qs}` : ""}`,
    { headers: { Authorization: `Bearer ${conn.token}` } },
  );

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch messages: ${response.statusText}`);
  }
  return await response.json();
}
