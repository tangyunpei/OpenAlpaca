/** `/v1/chat*` — history, confirmations, feedback. Sending lives in `chat-stream.ts`. */

import { ApiError, apiFetch } from "../http";
import type {
  ApprovalScope,
  ChatDeleteResponse,
  ChatHistoryResponse,
  FeedbackResponse,
  FeedbackValue,
} from "./types";

export interface ChatHistoryQuery {
  limit?: number;
  offset?: number;
  /**
   * Omit to let the daemon answer for `state.default_lane_key` and echo it
   * back — one round trip, no separate identity lookup needed. `GET /v1/me`
   * (GAP-16, closed) serves the same `default_lane_key` up front for a
   * caller that wants it before sending anything.
   */
  laneKey?: string;
}

/** `GET /v1/chat/history` */
export async function getChatHistory(
  query: ChatHistoryQuery = {},
  signal?: AbortSignal,
): Promise<ChatHistoryResponse> {
  return await apiFetch<ChatHistoryResponse>("/v1/chat/history", {
    query: {
      limit: query.limit,
      offset: query.offset,
      lane_key: query.laneKey,
    },
    signal,
  });
}

/** `DELETE /v1/chat/history` — clears messages and the conversation summary. */
export async function clearChatHistory(
  laneKey?: string,
): Promise<ChatDeleteResponse> {
  return await apiFetch<ChatDeleteResponse>("/v1/chat/history", {
    method: "DELETE",
    query: { lane_key: laneKey },
  });
}

export interface RespondToConfirmationInput {
  requestId: string;
  approved: boolean;
  /**
   * `entire_tool` caches the approval for the rest of the session; an
   * omitted scope (or `these_args`) approves this call only (GAP-01, closed
   * — the daemon's `ConfirmationBody` forwards this to the sandbox instead of
   * discarding it).
   */
  approvalScope?: ApprovalScope;
}

/** `POST /v1/chat/confirmations/{request_id}` — 200 with an empty body. */
export async function respondToConfirmation(
  input: RespondToConfirmationInput,
): Promise<void> {
  await apiFetch<void>(
    `/v1/chat/confirmations/${encodeURIComponent(input.requestId)}`,
    {
      method: "POST",
      body: {
        approved: input.approved,
        ...(input.approvalScope === undefined
          ? {}
          : { approval_scope: input.approvalScope }),
      },
    },
  );
}

/** `PUT /v1/chat/messages/{id}/feedback` */
export async function setMessageFeedback(
  messageId: number,
  feedback: FeedbackValue,
  comment?: string,
): Promise<FeedbackResponse> {
  return await apiFetch<FeedbackResponse>(
    `/v1/chat/messages/${messageId}/feedback`,
    {
      method: "PUT",
      body: { feedback, comment: comment ?? null },
    },
  );
}

/** `GET /v1/chat/messages/{id}/feedback` — `null` when none is recorded. */
export async function getMessageFeedback(
  messageId: number,
): Promise<FeedbackResponse | null> {
  try {
    return await apiFetch<FeedbackResponse>(
      `/v1/chat/messages/${messageId}/feedback`,
    );
  } catch (error) {
    if (error instanceof ApiError && error.isNotFound) return null;
    throw error;
  }
}

/** `DELETE /v1/chat/messages/{id}/feedback` */
export async function deleteMessageFeedback(messageId: number): Promise<void> {
  await apiFetch<void>(`/v1/chat/messages/${messageId}/feedback`, {
    method: "DELETE",
  });
}
