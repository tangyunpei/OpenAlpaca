import { describe, expect, it } from "vitest";

import {
  chatStreamReducer,
  initialChatStreamState,
  type ChatStreamAction,
  type ChatStreamState,
} from "@/lib/chat-stream";
import type { ChatMessage } from "@/lib/api/types";

import {
  buildTranscript,
  parseUserContent,
  showsLiveTurn,
  showsPendingTurn,
  streamPhaseLabel,
  type PendingTurn,
  type TranscriptInput,
} from "./transcript-model";

/** Drive the real reducer so these assertions track the SSE contract. */
function drive(...actions: ChatStreamAction[]): ChatStreamState {
  return actions.reduce(chatStreamReducer, initialChatStreamState);
}

const OPEN: ChatStreamAction = {
  type: "open",
  streamId: "s1",
  laneKey: "user:gui",
};

function message(
  overrides: Partial<ChatMessage> & { id: number },
): ChatMessage {
  return {
    lane_key: "user:gui",
    role: "user",
    content: "",
    created_at: "2026-08-31T14:22:00Z",
    ...overrides,
  };
}

function input(overrides: Partial<TranscriptInput> = {}): TranscriptInput {
  return {
    history: [],
    reports: [],
    confirmations: [],
    resolutions: [],
    stream: initialChatStreamState,
    pending: null,
    ...overrides,
  };
}

describe("streamPhaseLabel — thinking → deltas → done", () => {
  it("reports thinking from the moment the stream opens", () => {
    expect(streamPhaseLabel(drive(OPEN))).toBe("thinking");
    expect(streamPhaseLabel(drive(OPEN, { type: "thinking" }))).toBe(
      "thinking",
    );
  });

  it("switches to streaming on the first delta", () => {
    const state = drive(
      OPEN,
      { type: "thinking" },
      {
        type: "delta",
        content: "Hel",
      },
    );
    expect(streamPhaseLabel(state)).toBe("streaming");
  });

  it("carries no phase once done lands", () => {
    const state = drive(OPEN, {
      type: "done",
      data: {
        content: "Hello",
        model: "claude-sonnet-4-6",
        tokens_in: 10,
        tokens_out: 4,
        duration_ms: 1200,
      },
    });
    expect(streamPhaseLabel(state)).toBeNull();
  });
});

describe("live-turn lifecycle", () => {
  const done = drive(OPEN, {
    type: "done",
    data: {
      content: "Hello",
      model: "claude-sonnet-4-6",
      tokens_in: 10,
      tokens_out: 4,
      duration_ms: 1200,
    },
  });

  it("shows the live turn while it streams", () => {
    expect(
      showsLiveTurn(drive(OPEN, { type: "delta", content: "He" }), []),
    ).toBe(true);
  });

  it("keeps showing a finished turn until history carries it", () => {
    expect(showsLiveTurn(done, [])).toBe(true);
    expect(
      showsLiveTurn(done, [
        message({ id: 2, role: "assistant", content: "Hello" }),
      ]),
    ).toBe(false);
  });

  it("never shows an idle or errored stream as a row", () => {
    expect(showsLiveTurn(initialChatStreamState, [])).toBe(false);
    expect(
      showsLiveTurn(drive(OPEN, { type: "server_error", message: "boom" }), []),
    ).toBe(false);
  });

  it("drops the optimistic user row once the persisted copy arrives", () => {
    const pending: PendingTurn = {
      text: "audit",
      sent: "/steer audit",
      at: "2026-08-31T14:22:10Z",
      steer: { mode: "steer", label: "connector audit" },
    };
    expect(showsPendingTurn(pending, [])).toBe(true);
    expect(
      showsPendingTurn(pending, [
        message({ id: 1, role: "user", content: "/steer audit" }),
      ]),
    ).toBe(false);
  });
});

describe("parseUserContent (GAP-02)", () => {
  it("recognises the `/steer ` prefix and strips it for display", () => {
    expect(parseUserContent("/steer keep going")).toEqual({
      text: "keep going",
      steered: true,
    });
    expect(parseUserContent("hello")).toEqual({
      text: "hello",
      steered: false,
    });
  });
});

describe("buildTranscript", () => {
  it("orders history by timestamp and puts the live turn last", () => {
    const items = buildTranscript(
      input({
        history: [
          message({
            id: 2,
            role: "assistant",
            content: "second",
            created_at: "2026-08-31T14:23:00Z",
          }),
          message({
            id: 1,
            role: "user",
            content: "first",
            created_at: "2026-08-31T14:22:00Z",
          }),
        ],
        stream: drive(OPEN, { type: "delta", content: "live" }),
      }),
    );
    expect(items.map((item) => item.key)).toEqual(["m1", "m2", "live-s1"]);
  });

  it("marks a stored `/steer` message with the steer pill", () => {
    const [item] = buildTranscript(
      input({
        history: [message({ id: 1, role: "user", content: "/steer faster" })],
        steerLabel: "connector audit",
      }),
    );
    expect(item).toMatchObject({
      kind: "user",
      text: "faster",
      steer: { mode: "steer", label: "connector audit" },
    });
  });

  it("carries the assistant meta straight off a stored message", () => {
    const [item] = buildTranscript(
      input({
        history: [
          message({
            id: 3,
            role: "assistant",
            content: "done",
            model: "claude-sonnet-4-6",
            tokens_in: 12,
            tokens_out: 3,
            duration_ms: 900,
          }),
        ],
      }),
    );
    expect(item).toMatchObject({
      kind: "assistant",
      meta: {
        model: "claude-sonnet-4-6",
        tokensIn: 12,
        tokensOut: 3,
        durationMs: 900,
      },
    });
  });

  it("skips system messages", () => {
    expect(
      buildTranscript(
        input({ history: [message({ id: 4, role: "system", content: "x" })] }),
      ),
    ).toHaveLength(0);
  });

  it("renders a terminal error as its own row", () => {
    const items = buildTranscript(
      input({
        stream: drive(OPEN, {
          type: "server_error",
          message: "model exploded",
        }),
      }),
    );
    expect(items).toEqual([
      { kind: "error", key: "e-s1", message: "model exploded" },
    ]);
  });

  it("places reports, confirmations and resolutions on the same clock", () => {
    const items = buildTranscript(
      input({
        reports: [
          {
            taskId: "b41c8e02",
            title: "Connector audit",
            status: "done",
            startedAt: "2026-08-31T14:20:00Z",
            endedAt: "2026-08-31T14:26:00Z",
            summary: null,
            artifactCount: 2,
          },
        ],
        confirmations: [
          {
            requestId: "req-1",
            toolName: "shell_execute",
            toolArguments: { command: "cargo tree" },
            agentName: "review_agent",
            at: "2026-08-31T14:27:00Z",
          },
        ],
        resolutions: [
          {
            requestId: "req-1",
            resolution: "approved",
            note: "shell_execute approved",
            at: "2026-08-31T14:28:00Z",
          },
        ],
      }),
    );
    expect(items.map((item) => item.kind)).toEqual([
      "report",
      "confirmation",
      "resolution",
    ]);
  });
});
