/**
 * REST API client for message feedback endpoints.
 */

import { ensureConnection } from "./connection";

export interface FeedbackResponse {
  message_id: number;
  feedback: "positive" | "negative";
  comment: string | null;
}

export async function submitFeedback(
  messageId: number,
  feedback: "positive" | "negative",
  comment?: string,
): Promise<FeedbackResponse> {
  const conn = await ensureConnection();
  const res = await fetch(`${conn.baseUrl}/v1/chat/messages/${messageId}/feedback`, {
    method: "PUT",
    headers: {
      Authorization: `Bearer ${conn.token}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ feedback, comment: comment ?? null }),
  });
  if (!res.ok) throw new Error(`Failed to submit feedback: ${res.statusText}`);
  return await res.json();
}

export async function getFeedback(messageId: number): Promise<FeedbackResponse | null> {
  const conn = await ensureConnection();
  const res = await fetch(`${conn.baseUrl}/v1/chat/messages/${messageId}/feedback`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`Failed to get feedback: ${res.statusText}`);
  return await res.json();
}

export async function deleteFeedback(messageId: number): Promise<void> {
  const conn = await ensureConnection();
  const res = await fetch(`${conn.baseUrl}/v1/chat/messages/${messageId}/feedback`, {
    method: "DELETE",
    headers: { Authorization: `Bearer ${conn.token}` },
  });
  if (!res.ok) throw new Error(`Failed to delete feedback: ${res.statusText}`);
}
