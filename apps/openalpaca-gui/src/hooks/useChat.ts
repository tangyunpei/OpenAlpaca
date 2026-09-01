/**
 * Chat: history, streaming send, confirmations, feedback.
 *
 * The streaming hook owns the SSE state machine. Two rules from API_MAP §4.1
 * are load-bearing and easy to lose in a refactor:
 *   1. open the `EventSource` immediately after `POST /v1/chat` (the daemon
 *      sleeps only 100 ms before the first frame);
 *   2. treat `done.content` as the truth — the bridge silently drops deltas for
 *      a lagged client.
 */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";
import { useCallback, useEffect, useReducer, useRef } from "react";

import {
  clearChatHistory,
  deleteMessageFeedback,
  getChatHistory,
  getMessageFeedback,
  respondToConfirmation,
  setMessageFeedback,
  type ChatHistoryQuery,
  type RespondToConfirmationInput,
} from "@/lib/api/chat";
import type {
  ChatHistoryResponse,
  FeedbackResponse,
  FeedbackValue,
} from "@/lib/api/types";
import {
  chatStreamReducer,
  initialChatStreamState,
  isBlocked,
  isStreamActive,
  sendChatMessage,
  startChatStream,
  type ChatStreamHandle,
  type ChatStreamState,
  type SendChatOptions,
} from "@/lib/chat-stream";
import { daemonEvents } from "@/lib/events";
import { qk } from "@/lib/query-keys";

/**
 * `GET /v1/chat/history`. Omitting `laneKey` lets the daemon answer for its
 * default lane and echo the key back — the only way to learn it (GAP-16).
 */
export function useChatHistory(
  query: ChatHistoryQuery = {},
): UseQueryResult<ChatHistoryResponse> {
  return useQuery({
    queryKey: qk.chat.history(query),
    queryFn: ({ signal }) => getChatHistory(query, signal),
  });
}

export function useClearChatHistory(): UseMutationResult<
  { deleted: number },
  Error,
  string | undefined
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (laneKey?: string) => clearChatHistory(laneKey),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: qk.chat.all() });
      void client.invalidateQueries({ queryKey: qk.conversations.all() });
    },
  });
}

export function useMessageFeedback(
  messageId: number,
): UseQueryResult<FeedbackResponse | null> {
  return useQuery({
    queryKey: qk.chat.feedback(messageId),
    queryFn: () => getMessageFeedback(messageId),
  });
}

export interface SetFeedbackInput {
  messageId: number;
  feedback: FeedbackValue;
  comment?: string;
}

export function useSetMessageFeedback(): UseMutationResult<
  FeedbackResponse,
  Error,
  SetFeedbackInput
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: SetFeedbackInput) =>
      setMessageFeedback(input.messageId, input.feedback, input.comment),
    onSuccess: (_data, input) => {
      void client.invalidateQueries({
        queryKey: qk.chat.feedback(input.messageId),
      });
    },
  });
}

export function useDeleteMessageFeedback(): UseMutationResult<
  void,
  Error,
  number
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (messageId: number) => deleteMessageFeedback(messageId),
    onSuccess: (_data, messageId) => {
      void client.invalidateQueries({ queryKey: qk.chat.feedback(messageId) });
    },
  });
}

export interface ChatStreamController {
  state: ChatStreamState;
  /** A confirmation is pending — the composer is replaced by the banner. */
  blocked: boolean;
  active: boolean;
  send: (options: SendChatOptions) => Promise<void>;
  /** Approve/deny the oldest pending confirmation, or a named one. */
  respond: UseMutationResult<void, Error, RespondToConfirmationInput>;
  /** Drop the local stream state (new lane, cleared transcript). */
  reset: () => void;
}

/**
 * Drives one chat stream at a time — matching the design, which has a single
 * composer and a single in-flight assistant turn.
 */
export function useChatStream(): ChatStreamController {
  const [state, dispatch] = useReducer(
    chatStreamReducer,
    initialChatStreamState,
  );
  const handleRef = useRef<ChatStreamHandle | null>(null);
  const client = useQueryClient();

  useEffect(
    () => () => {
      handleRef.current?.close();
      handleRef.current = null;
    },
    [],
  );

  // The same confirmation arrives on the WS with more context. The reducer
  // dedupes by `request_id`, so subscribing here only adds robustness for the
  // case where the SSE frame was dropped.
  useEffect(
    () =>
      daemonEvents.onEvent((event) => {
        if (event.type !== "tool_confirmation_requested") return;
        dispatch({
          type: "confirmation",
          request: {
            request_id: event.request_id,
            tool_name: event.tool_name,
            tool_arguments: event.tool_arguments,
          },
        });
      }),
    [],
  );

  // A finished turn changes the transcript, and may have started a workflow.
  useEffect(() => {
    if (state.phase !== "done") return;
    void client.invalidateQueries({ queryKey: qk.chat.all() });
    if (state.result?.delegation) {
      void client.invalidateQueries({ queryKey: qk.tasks.all() });
    }
  }, [state.phase, state.result, client]);

  const send = useCallback(async (options: SendChatOptions) => {
    handleRef.current?.close();
    handleRef.current = null;

    const { stream_id, lane_key } = await sendChatMessage(options);
    // Immediately — the worker sleeps 100 ms and there is no replay.
    handleRef.current = await startChatStream({
      streamId: stream_id,
      laneKey: lane_key,
      onAction: dispatch,
    });
  }, []);

  const respond = useMutation<void, Error, RespondToConfirmationInput>({
    mutationFn: (input) => respondToConfirmation(input),
    onSuccess: (_data, input) => {
      dispatch({ type: "confirmation_resolved", requestId: input.requestId });
    },
  });

  const reset = useCallback(() => {
    handleRef.current?.close();
    handleRef.current = null;
    dispatch({ type: "reset" });
  }, []);

  return {
    state,
    blocked: isBlocked(state),
    active: isStreamActive(state),
    send,
    respond,
    reset,
  };
}
