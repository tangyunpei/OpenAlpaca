/**
 * SSE chat-stream state machine (API_MAP §4.1).
 *
 * The contract has several sharp edges, all encoded here:
 *
 *  * `POST /v1/chat` returns `{ stream_id, lane_key }` and the worker sleeps
 *    **100 ms** before the first event. Open the `EventSource` immediately on
 *    receiving the id — do not await anything else first.
 *  * `done` carries the **full** content, not the tail. It is the source of
 *    truth; accumulated deltas are only a live preview, and the SSE bridge
 *    silently drops frames for a lagged client. Since S1 the deltas are the
 *    provider's own tokens as they arrive — so they no longer necessarily
 *    concatenate to `done.content` (a multi-round turn streams the text
 *    written before a tool call), and replacing the content on `done` is not
 *    an optimisation but the contract.
 *  * `reasoning` (S2) carries the model thinking out loud, as `{ text }`. It
 *    is **not** the answer: it never enters `buffer`/`content`, the daemon
 *    never stores it, and history never replays it.
 *  * `confirmation_requested` can arrive at any point before `done`, including
 *    before any delta, and does **not** terminate the stream.
 *  * `EventSource` delivers both the server's *named* `error` event (JSON body)
 *    and its own *transport* error to listeners on `"error"`. Branch on whether
 *    `data` is present or you will swallow real server errors.
 *  * `EventSource` auto-reconnects, and the stream is GC'd 5 s after the
 *    terminal event, so a reopened stream 404s. Close inside the terminal
 *    handlers.
 */

import type { AttachmentRef, ChatSendResponse } from "./api/types";
import { ensureConnection, sseUrl } from "./connection";
import { apiFetch } from "./http";
import { workspaceHeader } from "./workspace-header";

// ── Payloads ────────────────────────────────────────────────────────────────

export interface DelegationInfo {
  task_id: string;
  title: string;
}

/**
 * One attachment the model that answered never received (U3).
 *
 * The turn still ran — the prompt carries a placeholder saying so — but the
 * bytes did not reach the model: no vision, no native document part, or an
 * extract the budget cut. `reason` is the daemon's own sentence.
 */
export interface SkippedAttachmentRef {
  id: string;
  reason: string;
}

/**
 * SSE `done`. `attachments_used`/`attachments_skipped`/`delegation` are
 * omitted when absent.
 *
 * Since U3 the two attachment lists are disjoint and `attachments_used` is
 * truthful: an attachment the adaptation withheld leaves it and appears in
 * `attachments_skipped` instead, so a client must not read "used" as "sent".
 */
export interface ChatStreamDone {
  content: string;
  model: string;
  tokens_in: number;
  tokens_out: number;
  duration_ms: number;
  attachments_used?: string[];
  attachments_skipped?: SkippedAttachmentRef[];
  delegation?: DelegationInfo;
}

/** SSE `confirmation_requested`. The WS twin adds `agent_id`/`lane_key`. */
export interface ChatConfirmationRequest {
  request_id: string;
  tool_name: string;
  tool_arguments: unknown;
  /**
   * The chat turn that raised it (R1) — this client's own stream when the SSE
   * frame drew the card, the frame's `stream_id` when the WebSocket twin did.
   *
   * `null`/absent means *unknown*, not "no stream": `GET /v1/chat/confirmations`
   * carries none, so a card seeded from the snapshot has no stream on it. A
   * consumer that retires cards on a turn's terminal frame must therefore act
   * on a match and never on the absence of one — the lane is shared, and
   * another client's main-loop prompt looks exactly like this one's.
   */
  stream_id?: string | null;
}

/**
 * How much live reasoning the state keeps (S2).
 *
 * The **tail**, not the head: a model's thinking runs forward, and what it is
 * working on now is what the indicator should be showing. It is a bound on
 * this client's memory of a stream that can run for minutes; the panel that
 * renders it caps its own height separately, so neither can push the
 * transcript around.
 */
export const REASONING_CAP = 4000;

/** The last `REASONING_CAP` characters of the reasoning so far. */
export function capReasoning(text: string): string {
  return text.length <= REASONING_CAP
    ? text
    : text.slice(text.length - REASONING_CAP);
}

// ── State machine ───────────────────────────────────────────────────────────

export type ChatStreamPhase =
  "idle" | "opening" | "thinking" | "streaming" | "done" | "error";

export interface ChatStreamError {
  message: string;
  /** `true` when the socket failed; `false` when the server sent `error`. */
  transport: boolean;
}

export interface ChatStreamState {
  phase: ChatStreamPhase;
  streamId: string | null;
  laneKey: string | null;
  /** Concatenated `delta.content` — a live preview only. */
  buffer: string;
  /** `done.content` once it lands, otherwise the buffer. Render this. */
  content: string;
  /**
   * The model's live reasoning, capped to its last {@link REASONING_CAP}
   * characters (S2).
   *
   * Never part of `content`, never persisted: the daemon keeps it out of the
   * stored message and `GET /v1/chat/history` never replays it, so this is the
   * only place it exists and it dies with the turn.
   */
  reasoning: string;
  result: ChatStreamDone | null;
  /**
   * Unresolved confirmations, deduped by `request_id`, in arrival order.
   *
   * Session-level, not turn-level: a run's prompt arrives after its turn is
   * terminal, and this list is what keeps it answerable (G1).
   */
  pendingConfirmations: ChatConfirmationRequest[];
  error: ChatStreamError | null;
  deltaCount: number;
  /** Once terminal, every later frame of the *answer* is ignored. */
  terminal: boolean;
}

export type ChatStreamAction =
  | { type: "open"; streamId: string; laneKey: string }
  | { type: "thinking" }
  | { type: "delta"; content: string }
  /** One `reasoning` frame — the model thinking out loud (S2). */
  | { type: "reasoning"; text: string }
  | { type: "confirmation"; request: ChatConfirmationRequest }
  /** Answered, timed out, or its run finished — drop the card. */
  | { type: "confirmation_resolved"; requestId: string }
  | { type: "done"; data: ChatStreamDone }
  | { type: "server_error"; message: string }
  | { type: "transport_error"; message: string }
  | { type: "reset" };

export const initialChatStreamState: ChatStreamState = {
  phase: "idle",
  streamId: null,
  laneKey: null,
  buffer: "",
  content: "",
  reasoning: "",
  result: null,
  pendingConfirmations: [],
  error: null,
  deltaCount: 0,
  terminal: false,
};

/**
 * Pure reducer. Terminal states absorb every frame of the **answer** —
 * deltas, another `done`, a late error — but not the confirmations, which are
 * not part of it.
 *
 * A confirmation is a question addressed to the person, and the run that asks
 * it usually outlives the turn that started it: a workflow's
 * `artifact_write` prompt arrives minutes after that turn went `done`, and
 * dropping it here left the daemon waiting on a card nobody ever saw, for the
 * whole 300 s timeout (G1). So `confirmation` and `confirmation_resolved` both
 * stay live past `terminal`; `reset` — a new conversation — is what clears
 * them.
 */
export function chatStreamReducer(
  state: ChatStreamState,
  action: ChatStreamAction,
): ChatStreamState {
  switch (action.type) {
    case "reset":
      return initialChatStreamState;

    case "open":
      return {
        ...initialChatStreamState,
        phase: "opening",
        streamId: action.streamId,
        laneKey: action.laneKey,
      };

    case "confirmation_resolved": {
      const remaining = state.pendingConfirmations.filter(
        (c) => c.request_id !== action.requestId,
      );
      if (remaining.length === state.pendingConfirmations.length) return state;
      return { ...state, pendingConfirmations: remaining };
    }

    case "thinking":
      if (state.terminal) return state;
      // A late `thinking` after deltas have started must not rewind the phase.
      return state.deltaCount > 0 ? state : { ...state, phase: "thinking" };

    case "reasoning": {
      // Not `buffer`, and no phase change: reasoning is not an answer, and a
      // turn that only thought out loud still owes the person one. The
      // `thinking` frame the daemon sends first is what moved the phase.
      if (state.terminal || action.text === "") return state;
      return {
        ...state,
        reasoning: capReasoning(state.reasoning + action.text),
      };
    }

    case "delta": {
      if (state.terminal) return state;
      const buffer = state.buffer + action.content;
      return {
        ...state,
        phase: "streaming",
        buffer,
        content: buffer,
        deltaCount: state.deltaCount + 1,
      };
    }

    case "confirmation": {
      // Deliberately not gated on `terminal`: see the note above.
      //
      // The first sighting wins, `stream_id` included — so a card seeded from
      // the snapshot keeps its unknown stream even when a live frame follows.
      // That is the conservative side of R1: a consumer acts on a stream it
      // knows, never on one it does not.
      const seen = state.pendingConfirmations.some(
        (c) => c.request_id === action.request.request_id,
      );
      if (seen) return state;
      return {
        ...state,
        pendingConfirmations: [...state.pendingConfirmations, action.request],
      };
    }

    case "done":
      if (state.terminal) return state;
      return {
        ...state,
        phase: "done",
        // `done.content` is authoritative — deltas may have been dropped.
        content: action.data.content,
        result: action.data,
        error: null,
        terminal: true,
      };

    case "server_error":
      if (state.terminal) return state;
      return {
        ...state,
        phase: "error",
        error: { message: action.message, transport: false },
        terminal: true,
      };

    case "transport_error":
      // The browser always fires a transport error after `close()`; ignoring it
      // once terminal is what keeps a completed stream from flipping to error.
      if (state.terminal) return state;
      return {
        ...state,
        phase: "error",
        error: { message: action.message, transport: true },
        terminal: true,
      };
  }
}

/** A confirmation is blocking the transcript's composer. */
export function isBlocked(state: ChatStreamState): boolean {
  return state.pendingConfirmations.length > 0;
}

export function isStreamActive(state: ChatStreamState): boolean {
  return (
    state.phase === "opening" ||
    state.phase === "thinking" ||
    state.phase === "streaming"
  );
}

// ── Driver ──────────────────────────────────────────────────────────────────

/** The slice of `EventSource` the driver uses, so tests can supply a double. */
export interface EventSourceLike {
  addEventListener(
    type: string,
    listener: (event: { data?: unknown }) => void,
  ): void;
  close(): void;
}

export interface ChatStreamHandle {
  readonly streamId: string;
  readonly laneKey: string;
  /** Close the transport. Safe to call more than once. */
  close(): void;
}

export interface StartChatStreamOptions {
  streamId: string;
  laneKey: string;
  onAction: (action: ChatStreamAction) => void;
  /** Defaults to a real `EventSource` against `sseUrl(...)`. */
  createEventSource?: (url: string) => EventSourceLike;
  /** Pre-resolved URL; supplied by tests instead of a live connection. */
  url?: string;
}

function safeParse(data: unknown): Record<string, unknown> | null {
  if (typeof data !== "string") return null;
  try {
    const parsed: unknown = JSON.parse(data);
    return typeof parsed === "object" && parsed !== null
      ? (parsed as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

/**
 * Attach the SSE listeners for one chat stream.
 *
 * Synchronous by design: the caller already has `stream_id`, and the server is
 * only 100 ms from its first frame.
 */
export function attachChatStream(
  source: EventSourceLike,
  options: Pick<StartChatStreamOptions, "streamId" | "laneKey" | "onAction">,
): ChatStreamHandle {
  const { streamId, laneKey, onAction } = options;
  let closed = false;

  const close = (): void => {
    if (closed) return;
    closed = true;
    source.close();
  };

  onAction({ type: "open", streamId, laneKey });

  source.addEventListener("thinking", () => {
    onAction({ type: "thinking" });
  });

  source.addEventListener("delta", (event) => {
    const payload = safeParse(event.data);
    if (payload === null || typeof payload.content !== "string") return;
    onAction({ type: "delta", content: payload.content });
  });

  // S2. The field is `text`, not `content`, deliberately: a client must never
  // append reasoning to the answer.
  source.addEventListener("reasoning", (event) => {
    const payload = safeParse(event.data);
    if (payload === null || typeof payload.text !== "string") return;
    onAction({ type: "reasoning", text: payload.text });
  });

  source.addEventListener("confirmation_requested", (event) => {
    const payload = safeParse(event.data);
    if (payload === null || typeof payload.request_id !== "string") return;
    onAction({
      type: "confirmation",
      request: {
        request_id: payload.request_id,
        tool_name:
          typeof payload.tool_name === "string"
            ? payload.tool_name
            : "unknown_tool",
        tool_arguments: payload.tool_arguments ?? null,
        // The frame does not name a stream and does not have to: it *is* this
        // one (R1).
        stream_id: streamId,
      },
    });
    // Deliberately does NOT close — the same stream continues after the
    // confirmation is answered.
  });

  // T1. A prompt stops being pending for four reasons and only one of them is
  // an answer this client posted; this frame carries all four. Like its twin
  // it does not close the stream.
  source.addEventListener("confirmation_resolved", (event) => {
    const payload = safeParse(event.data);
    if (payload === null || typeof payload.request_id !== "string") return;
    onAction({
      type: "confirmation_resolved",
      requestId: payload.request_id,
    });
  });

  source.addEventListener("done", (event) => {
    const payload = safeParse(event.data);
    if (payload === null || typeof payload.content !== "string") {
      onAction({
        type: "server_error",
        message: "Malformed `done` frame from the daemon",
      });
      close();
      return;
    }
    onAction({ type: "done", data: payload as unknown as ChatStreamDone });
    // Close before the browser can auto-reconnect into the 5 s GC window.
    close();
  });

  source.addEventListener("error", (event) => {
    // Named server error → carries a JSON body. Transport failure → no data.
    const payload = safeParse(event.data);
    if (payload !== null) {
      const message =
        typeof payload.message === "string"
          ? payload.message
          : "The daemon reported a stream error";
      onAction({ type: "server_error", message });
    } else {
      onAction({
        type: "transport_error",
        message: "Chat stream connection lost",
      });
    }
    close();
  });

  return { streamId, laneKey, close };
}

/** Open the SSE stream for an already-created `stream_id`. */
export async function startChatStream(
  options: StartChatStreamOptions,
): Promise<ChatStreamHandle> {
  const create =
    options.createEventSource ??
    ((url: string) => new EventSource(url) as unknown as EventSourceLike);

  const url = options.url ?? sseUrl(await ensureConnection(), options.streamId);
  return attachChatStream(create(url), options);
}

export interface SendChatOptions {
  content: string;
  attachments?: AttachmentRef[];
  /**
   * Run this turn on a named model (GAP-13, closed). The daemon validates it
   * against the registry and refuses an id it cannot serve, so what the
   * composer's picker says is what answers.
   */
  model?: string;
  /** Optional `x-workspace-path` header. */
  workspacePath?: string;
  /**
   * The conversation this turn belongs to (§5.7). Omitted — the default — the
   * turn lands in the lane's active session, which is what lets the daemon
   * open a new one when the window's project changed (R48).
   *
   * Sent, the turn addresses that conversation and **takes its project**,
   * overriding the header (R49): resuming is a change of scope, not just of
   * transcript. An archived target is `409 SESSION_ARCHIVED` unless the caller
   * also asked to re-open it, which this client does not do implicitly —
   * re-opening archives whatever is live on the lane, and that is the user's
   * call, made by clicking the row.
   */
  sessionId?: string;
  signal?: AbortSignal;
}

/** `POST /v1/chat` — creates the stream server-side and returns its id. */
export async function sendChatMessage(
  options: SendChatOptions,
): Promise<ChatSendResponse> {
  return await apiFetch<ChatSendResponse>("/v1/chat", {
    method: "POST",
    body: {
      content: options.content,
      attachments: options.attachments ?? [],
      ...(options.model === undefined ? {} : { model: options.model }),
      ...(options.sessionId === undefined
        ? {}
        : { session_id: options.sessionId }),
    },
    headers: workspaceHeader(options.workspacePath),
    signal: options.signal,
  });
}
