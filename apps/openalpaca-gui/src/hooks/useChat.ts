/**
 * Chat: history, streaming send, confirmations.
 *
 * The streaming hook owns the SSE state machine. Two rules from API_MAP §4.1
 * are load-bearing and easy to lose in a refactor:
 *   1. open the `EventSource` immediately after `POST /v1/chat` (the daemon
 *      sleeps only 100 ms before the first frame);
 *   2. treat `done.content` as the truth — the bridge silently drops deltas for
 *      a lagged client.
 *
 * There are no feedback or clear-history hooks here. The daemon serves those
 * four routes and `lib/api/chat.ts` is their client, but this window draws no
 * control for either — no thumb on a message, no "clear this lane" — and the
 * GUI manual documents none, so wrapper hooks with no caller were deleted
 * rather than kept as a surface nothing reaches.
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
  getChatHistory,
  listPendingConfirmations,
  respondToConfirmation,
  type ChatHistoryQuery,
  type RespondToConfirmationInput,
} from "@/lib/api/chat";
import type { ChatHistoryResponse, PendingConfirmation } from "@/lib/api/types";
import {
  chatStreamReducer,
  initialChatStreamState,
  isBlocked,
  isStreamActive,
  sendChatMessage,
  startChatStream,
  type ChatConfirmationRequest,
  type ChatStreamAction,
  type ChatStreamHandle,
  type ChatStreamState,
  type SendChatOptions,
} from "@/lib/chat-stream";
import {
  isConfirmationOver,
  noteConfirmationOver,
} from "@/lib/confirmations-over";
import { daemonEvents, type ServerEvent } from "@/lib/events";
import { ApiError } from "@/lib/http";
import { qk } from "@/lib/query-keys";

/**
 * `GET /v1/chat/history`. Omitting `laneKey` lets the daemon answer for its
 * default lane and echo the key back, so this stays the one-round-trip path
 * for the chat view itself; `GET /v1/me` (GAP-16, closed) is there for a
 * caller that needs the default lane before it has any chat history to ask.
 *
 * `enabled` is there for the transcript's two-call tail read: the second call
 * has no offset to ask for until the first has answered with a `total`, and a
 * query that cannot be formed yet must not be issued.
 */
export function useChatHistory(
  query: ChatHistoryQuery = {},
  options: { enabled?: boolean } = {},
): UseQueryResult<ChatHistoryResponse> {
  return useQuery({
    queryKey: qk.chat.history(query),
    queryFn: ({ signal }) => getChatHistory(query, signal),
    enabled: options.enabled ?? true,
  });
}

/**
 * `GET /v1/chat/confirmations` (S9) — what is waiting *now*, not what was
 * announced while this window happened to be listening.
 *
 * The prompts themselves only ever arrive as live frames, so a window that
 * opened after one was raised — a reload, a second window, a restart of this
 * app — showed no card at all and the run sat on the prompt until it timed
 * out. This query is the seed for that, on load and, because a resync
 * invalidates the whole cache, on every reconnect.
 *
 * Polled as well, because this list is a **second opinion** on a modal state:
 * a card that is no longer in it is one nobody has to answer any more, however
 * that came about, and never trusting one signal for the thing that pauses the
 * composer is the rule T1 wrote. The cadence follows the stake — every 10 s
 * while a card is up and the composer is paused, every 30 s otherwise, when it
 * is only a seed for a prompt this window has not heard about.
 *
 * Read on **every** mount, however fresh the cache (G1, Round G). The chat
 * view is swapped out whenever another view opens, and a cached list younger
 * than `staleTime` used to be handed to the remount as if it were current: a
 * prompt answered just before the trip came back as a card, and one raised
 * while chat was away stayed invisible until the next 30 s poll. The caller
 * seeds only from a read made during its own mount (`useChatSession`).
 */
export const CONFIRMATIONS_POLL_MS = 30_000;
export const CONFIRMATIONS_POLL_BLOCKED_MS = 10_000;

export function usePendingConfirmations(
  /** A card is on screen — poll at the faster cadence. */
  blocked = false,
): UseQueryResult<PendingConfirmation[]> {
  return useQuery({
    queryKey: qk.chat.confirmations(),
    queryFn: ({ signal }) => listPendingConfirmations(signal),
    refetchInterval: blocked
      ? CONFIRMATIONS_POLL_BLOCKED_MS
      : CONFIRMATIONS_POLL_MS,
    refetchOnMount: "always",
  });
}

/**
 * The answer route's `404`: the daemon holds no such prompt — it was answered
 * already (here, elsewhere, from the CLI), it timed out, it was withdrawn, or
 * the daemon restarted (`ConfirmationBroker::respond`, both arms). It is a
 * resolution, not a failure to retry (G1, Round G).
 */
export function isNoLongerPending(error: unknown): boolean {
  return error instanceof ApiError && error.isNotFound;
}

/** The daemon-wide frame a pending confirmation arrives on. */
export type ToolConfirmationEvent = Extract<
  ServerEvent,
  { type: "tool_confirmation_requested" }
>;

export interface ChatStreamOptions {
  /**
   * Whether a confirmation arriving on the **daemon-wide** socket belongs to
   * this view.
   *
   * `/v1/events` forwards every frame to every client with no lane filter, and
   * a confirmation accepted here blocks the composer and arms Enter-approve —
   * so answering one raised by a scheduled skill, a connector or a wake turn
   * would widen a foreign agent's run from this window. Omitted, every frame
   * is accepted, which is the pre-filter behaviour and is what a caller with
   * no lane of its own should get.
   */
  accepts?: (event: ToolConfirmationEvent, streamId: string | null) => boolean;
}

export interface ChatStreamController {
  state: ChatStreamState;
  /** A confirmation is pending — the composer is replaced by the banner. */
  blocked: boolean;
  active: boolean;
  send: (options: SendChatOptions) => Promise<void>;
  /**
   * Approve/deny the oldest pending confirmation, or a named one.
   *
   * Both of the daemon's answers retire the card: `200` (it took this answer)
   * and `404` (it holds no such prompt — {@link isNoLongerPending}), which
   * also re-reads the pending list, since the card was drawn from a view of
   * it that was wrong.
   */
  respond: UseMutationResult<void, Error, RespondToConfirmationInput>;
  /**
   * Take on a confirmation this client learned about off-stream (S9) — from
   * `GET /v1/chat/confirmations`, which is the only way a window that was not
   * listening when it was raised can know it exists.
   *
   * Deduped by `request_id` in the reducer, so seeding what is already on
   * screen changes nothing; and a prompt the daemon has said is over is never
   * adopted again (`lib/confirmations-over`).
   */
  adoptConfirmation: (request: ChatConfirmationRequest) => void;
  /**
   * Drop a pending confirmation this client is not going to answer (G1).
   *
   * The daemon publishes no "this prompt expired" frame, so a prompt whose run
   * has finished — it timed out, or somebody else answered it — would sit on
   * screen for ever. The caller that knows the run is over says so.
   */
  dismissConfirmation: (requestId: string) => void;
  /** Drop the local stream state (new lane, cleared transcript). */
  reset: () => void;
}

/**
 * Drives one chat stream at a time — matching the design, which has a single
 * composer and a single in-flight assistant turn.
 */
export function useChatStream(
  options: ChatStreamOptions = {},
): ChatStreamController {
  const [state, dispatchToReducer] = useReducer(
    chatStreamReducer,
    initialChatStreamState,
  );
  /**
   * Every action reaches the reducer through here, and this is the one place a
   * card is adopted: the SSE frame, its WebSocket twin and the snapshot seed
   * all arrive as `confirmation`. A prompt the daemon has already said is over
   * — a frame or a read that left before the answer and lands after it — is
   * dropped rather than drawn again (G1, Round G). The reducer stays pure: it
   * still dedupes against the cards on screen only.
   */
  const dispatch = useCallback((action: ChatStreamAction) => {
    if (
      action.type === "confirmation" &&
      isConfirmationOver(action.request.request_id)
    )
      return;
    dispatchToReducer(action);
  }, []);
  const handleRef = useRef<ChatStreamHandle | null>(null);
  const client = useQueryClient();

  // The WS subscription is opened once and never re-subscribes, so both of the
  // things its filter needs — the caller's predicate and this stream's own id —
  // are read through refs refreshed on every render.
  const acceptsRef = useRef(options.accepts);
  acceptsRef.current = options.accepts;
  const streamIdRef = useRef(state.streamId);
  streamIdRef.current = state.streamId;

  useEffect(
    () => () => {
      handleRef.current?.close();
      handleRef.current = null;
    },
    [],
  );

  // The same confirmation arrives on the WS with more context. The reducer
  // dedupes by `request_id`, so subscribing here only adds robustness for the
  // case where the SSE frame was dropped — but the socket carries *every*
  // lane's confirmations, so what is robustness for this stream's own frame is
  // a foreign run's prompt for anything else, and `accepts` is what tells them
  // apart.
  useEffect(
    () =>
      daemonEvents.onEvent((event) => {
        // The daemon's word that a prompt is over, whosever it was: recorded
        // before any filter, because a snapshot that left before this frame
        // may still list it (G1, Round G). Retiring the card on screen is the
        // chat session's business.
        if (event.type === "tool_confirmation_resolved") {
          noteConfirmationOver(event.request_id);
          return;
        }
        if (event.type !== "tool_confirmation_requested") return;
        const accepts = acceptsRef.current;
        if (accepts !== undefined && !accepts(event, streamIdRef.current))
          return;
        dispatch({
          type: "confirmation",
          request: {
            request_id: event.request_id,
            tool_name: event.tool_name,
            tool_arguments: event.tool_arguments,
            // Which turn raised it, so a terminal frame can retire this
            // window's own prompts and leave another client's alone (R1).
            stream_id: event.stream_id,
            // Design 09 §4.4: the daemon's own text for the card, and
            // whether the approval is good for this call only.
            preview: event.preview ?? null,
            per_call: event.per_call === true,
          },
        });
      }),
    [dispatch],
  );

  // A finished turn changes the transcript, and may have started a workflow.
  // `qk.chat.all()` covers the pending-confirmation snapshot too, which is the
  // re-read T1 wants on a turn's terminal frame: the turn is over, so whatever
  // it was blocked on is over with it.
  useEffect(() => {
    if (state.phase !== "done") return;
    void client.invalidateQueries({ queryKey: qk.chat.all() });
    if (state.result?.delegation) {
      void client.invalidateQueries({ queryKey: qk.tasks.all() });
    }
    // Design 11 §5: the turn compacted, and its working state is on the
    // session row — the aside's card reads it from the session list.
    if (state.result?.working_state_updated === true) {
      void client.invalidateQueries({ queryKey: qk.sessions.all() });
    }
  }, [state.phase, state.result, client]);

  // The same re-read for the *other* terminal frame (T1). A failed turn
  // invalidates no transcript — there is nothing new to show — but a prompt it
  // raised is just as finished as one a successful turn raised.
  useEffect(() => {
    if (state.phase !== "error") return;
    void client.invalidateQueries({ queryKey: qk.chat.confirmations() });
  }, [state.phase, client]);

  const send = useCallback(
    async (options: SendChatOptions) => {
      handleRef.current?.close();
      handleRef.current = null;

      const { stream_id, lane_key } = await sendChatMessage(options);
      // Immediately — the worker sleeps 100 ms and there is no replay.
      handleRef.current = await startChatStream({
        streamId: stream_id,
        laneKey: lane_key,
        onAction: (action) => {
          // On the stream this action comes only from the daemon's own
          // `confirmation_resolved` frame (G1, Round G).
          if (action.type === "confirmation_resolved")
            noteConfirmationOver(action.requestId);
          dispatch(action);
        },
      });
    },
    [dispatch],
  );

  const respond = useMutation<void, Error, RespondToConfirmationInput>({
    mutationFn: (input) => respondToConfirmation(input),
    onSuccess: (_data, input) => {
      noteConfirmationOver(input.requestId);
      dispatch({ type: "confirmation_resolved", requestId: input.requestId });
    },
    onError: (error, input) => {
      if (!isNoLongerPending(error)) return;
      // A card nobody can answer any more must not stay up for a second
      // click to repeat the 404 — and the list it was drawn from was wrong,
      // so it is read again (G1, Round G).
      noteConfirmationOver(input.requestId);
      dispatch({ type: "confirmation_resolved", requestId: input.requestId });
      void client.invalidateQueries({ queryKey: qk.chat.confirmations() });
    },
  });

  const adoptConfirmation = useCallback(
    (request: ChatConfirmationRequest) => {
      dispatch({ type: "confirmation", request });
    },
    [dispatch],
  );

  // An inference, not the daemon's word — the turn ended, the run finished,
  // the snapshot stopped listing it — so it is never recorded as over (T18).
  const dismissConfirmation = useCallback(
    (requestId: string) => {
      dispatch({ type: "confirmation_resolved", requestId });
    },
    [dispatch],
  );

  const reset = useCallback(() => {
    handleRef.current?.close();
    handleRef.current = null;
    dispatch({ type: "reset" });
  }, [dispatch]);

  return {
    state,
    blocked: isBlocked(state),
    active: isStreamActive(state),
    send,
    respond,
    adoptConfirmation,
    dismissConfirmation,
    reset,
  };
}
