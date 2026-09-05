import { describe, expect, it, vi } from "vitest";

import {
  attachChatStream,
  chatStreamReducer,
  initialChatStreamState,
  isBlocked,
  type ChatStreamAction,
  type ChatStreamDone,
  type ChatStreamState,
  type EventSourceLike,
} from "./chat-stream";

function run(actions: ChatStreamAction[]): ChatStreamState {
  return actions.reduce(chatStreamReducer, initialChatStreamState);
}

const open: ChatStreamAction = {
  type: "open",
  streamId: "s1",
  laneKey: "local:gui",
};

const doneData: ChatStreamDone = {
  content: "the full authoritative reply",
  model: "claude-sonnet-4-6",
  tokens_in: 1284,
  tokens_out: 612,
  duration_ms: 3800,
};

describe("chatStreamReducer", () => {
  it("walks opening → thinking → streaming → done", () => {
    const state = run([
      open,
      { type: "thinking" },
      { type: "delta", content: "the " },
      { type: "delta", content: "full" },
      { type: "done", data: doneData },
    ]);

    expect(state.phase).toBe("done");
    expect(state.streamId).toBe("s1");
    expect(state.laneKey).toBe("local:gui");
    expect(state.deltaCount).toBe(2);
    expect(state.terminal).toBe(true);
  });

  it("prefers done.content over the accumulated deltas", () => {
    // The SSE bridge silently drops frames for a lagged client, so the buffer
    // can be short. `done` is the source of truth.
    const state = run([
      open,
      { type: "delta", content: "the " },
      { type: "done", data: doneData },
    ]);

    expect(state.buffer).toBe("the ");
    expect(state.content).toBe(doneData.content);
    expect(state.result).toEqual(doneData);
  });

  it("ignores deltas that arrive after the terminal frame", () => {
    const state = run([
      open,
      { type: "done", data: doneData },
      { type: "delta", content: " stray" },
    ]);

    expect(state.content).toBe(doneData.content);
    expect(state.deltaCount).toBe(0);
  });

  it("keeps the first terminal frame when error precedes done", () => {
    const state = run([
      open,
      { type: "delta", content: "partial" },
      { type: "server_error", message: "provider exploded" },
      { type: "done", data: doneData },
    ]);

    expect(state.phase).toBe("error");
    expect(state.error).toEqual({
      message: "provider exploded",
      transport: false,
    });
    expect(state.result).toBeNull();
  });

  it("ignores the transport error the browser fires after close()", () => {
    const state = run([
      open,
      { type: "done", data: doneData },
      { type: "transport_error", message: "Chat stream connection lost" },
    ]);

    expect(state.phase).toBe("done");
    expect(state.error).toBeNull();
  });

  it("records a transport error while the stream is live", () => {
    const state = run([
      open,
      { type: "transport_error", message: "socket died" },
    ]);

    expect(state.phase).toBe("error");
    expect(state.error).toEqual({ message: "socket died", transport: true });
  });

  it("accepts a confirmation before any delta without terminating", () => {
    const state = run([
      open,
      {
        type: "confirmation",
        request: {
          request_id: "r1",
          tool_name: "shell_execute",
          tool_arguments: { cmd: "ls" },
        },
      },
      { type: "delta", content: "resumed" },
    ]);

    expect(state.terminal).toBe(false);
    expect(state.phase).toBe("streaming");
    expect(isBlocked(state)).toBe(true);
    expect(state.pendingConfirmations).toHaveLength(1);
  });

  it("dedupes a confirmation delivered on both SSE and the WebSocket", () => {
    const request = {
      request_id: "r1",
      tool_name: "shell_execute",
      tool_arguments: null,
    };
    const state = run([
      open,
      { type: "confirmation", request },
      { type: "confirmation", request },
    ]);

    expect(state.pendingConfirmations).toHaveLength(1);
  });

  it("clears a resolved confirmation even after the stream finished", () => {
    const state = run([
      open,
      {
        type: "confirmation",
        request: {
          request_id: "r1",
          tool_name: "shell_execute",
          tool_arguments: null,
        },
      },
      { type: "done", data: doneData },
      { type: "confirmation_resolved", requestId: "r1" },
    ]);

    expect(state.pendingConfirmations).toHaveLength(0);
    expect(isBlocked(state)).toBe(false);
  });

  it("does not rewind the phase when `thinking` arrives out of order", () => {
    const state = run([
      open,
      { type: "delta", content: "hi" },
      { type: "thinking" },
    ]);
    expect(state.phase).toBe("streaming");
  });
});

// A minimal EventSource double: `emit` drives one named event.
function fakeSource() {
  const listeners = new Map<string, (event: { data?: unknown }) => void>();
  const close = vi.fn();
  const source: EventSourceLike = {
    addEventListener: (type, listener) => listeners.set(type, listener),
    close,
  };
  return {
    source,
    close,
    emit(type: string, data?: unknown) {
      listeners.get(type)?.(data === undefined ? {} : { data });
    },
  };
}

describe("attachChatStream", () => {
  it("closes the stream on done so EventSource cannot auto-reconnect into a 404", () => {
    const fake = fakeSource();
    const actions: ChatStreamAction[] = [];
    attachChatStream(fake.source, {
      streamId: "s1",
      laneKey: "local:gui",
      onAction: (a) => actions.push(a),
    });

    fake.emit("delta", JSON.stringify({ content: "hi" }));
    fake.emit("done", JSON.stringify(doneData));

    expect(fake.close).toHaveBeenCalledTimes(1);
    expect(actions.at(-1)).toEqual({ type: "done", data: doneData });
  });

  it("distinguishes a named server error from a transport failure", () => {
    const withData = fakeSource();
    const dataActions: ChatStreamAction[] = [];
    attachChatStream(withData.source, {
      streamId: "s1",
      laneKey: "l",
      onAction: (a) => dataActions.push(a),
    });
    withData.emit("error", JSON.stringify({ message: "CHAT_NOT_CONFIGURED" }));
    expect(dataActions.at(-1)).toEqual({
      type: "server_error",
      message: "CHAT_NOT_CONFIGURED",
    });

    const noData = fakeSource();
    const transportActions: ChatStreamAction[] = [];
    attachChatStream(noData.source, {
      streamId: "s1",
      laneKey: "l",
      onAction: (a) => transportActions.push(a),
    });
    noData.emit("error");
    expect(transportActions.at(-1)).toEqual({
      type: "transport_error",
      message: "Chat stream connection lost",
    });
  });

  it("does not close on a mid-stream confirmation", () => {
    const fake = fakeSource();
    const actions: ChatStreamAction[] = [];
    attachChatStream(fake.source, {
      streamId: "s1",
      laneKey: "l",
      onAction: (a) => actions.push(a),
    });

    fake.emit(
      "confirmation_requested",
      JSON.stringify({
        request_id: "r1",
        tool_name: "shell_execute",
        tool_arguments: {},
      }),
    );

    expect(fake.close).not.toHaveBeenCalled();
    expect(actions.at(-1)).toMatchObject({ type: "confirmation" });
  });

  it("reports a malformed done frame as a server error", () => {
    const fake = fakeSource();
    const actions: ChatStreamAction[] = [];
    attachChatStream(fake.source, {
      streamId: "s1",
      laneKey: "l",
      onAction: (a) => actions.push(a),
    });

    fake.emit("done", "not json");

    expect(actions.at(-1)).toMatchObject({ type: "server_error" });
    expect(fake.close).toHaveBeenCalled();
  });
});
