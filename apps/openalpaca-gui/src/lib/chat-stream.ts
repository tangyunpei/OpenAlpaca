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
 *    silently drops frames for a lagged client.
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

// ── Payloads ────────────────────────────────────────────────────────────────

export interface DelegationInfo {
  task_id: string;
  title: string;
}

/** SSE `done`. `attachments_used`/`delegation` are omitted when absent. */
export interface ChatStreamDone {
  content: string;
  model: string;
  tokens_in: number;
  tokens_out: number;
  duration_ms: number;
  attachments_used?: string[];
  delegation?: DelegationInfo;
}

/** SSE `confirmation_requested`. The WS twin adds `agent_id`/`stream_id`/`lane_key`. */
export interface ChatConfirmationRequest {
  request_id: string;
  tool_name: string;
  tool_arguments: unknown;
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
  result: ChatStreamDone | null;
  /** Unresolved confirmations, deduped by `request_id`, in arrival order. */
  pendingConfirmations: ChatConfirmationRequest[];
  error: ChatStreamError | null;
  deltaCount: number;
  /** Once terminal, every later frame is ignored. */
  terminal: boolean;
}

export type ChatStreamAction =
  | { type: "open"; streamId: string; laneKey: string }
  | { type: "thinking" }
  | { type: "delta"; content: string }
  | { type: "confirmation"; request: ChatConfirmationRequest }
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
  result: null,
  pendingConfirmations: [],
  error: null,
  deltaCount: 0,
  terminal: false,
};

/**
 * Pure reducer. Terminal states absorb every frame except confirmation
 * resolutions, which stay live so a card can be dismissed after `done`.
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
      if (state.terminal) return state;
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
      },
    });
    // Deliberately does NOT close — the same stream continues after the
    // confirmation is answered.
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
  /** GAP-13: the daemon ignores this today; sending it keeps the UI honest. */
  model?: string;
  /** Optional `x-workspace-path` header. */
  workspacePath?: string;
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
    },
    headers: options.workspacePath
      ? { "x-workspace-path": options.workspacePath }
      : undefined,
    signal: options.signal,
  });
}
