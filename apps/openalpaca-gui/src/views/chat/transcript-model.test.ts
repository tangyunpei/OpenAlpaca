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
    artifacts: [],
    confirmations: [],
    resolutions: [],
    steers: [],
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

// The GUI stopped sending the prefix when GAP-02 closed, but the CLI and
// Telegram still do — and stored history keeps what was sent before.
describe("parseUserContent — the chat prefix in stored history", () => {
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

describe("buildTranscript — steers sent to a run's own route", () => {
  it("shows a steer as a user row carrying the run's pill, in time order", () => {
    const items = buildTranscript(
      input({
        history: [
          message({
            id: 1,
            role: "user",
            content: "audit the connectors",
            created_at: "2026-09-05T14:22:00Z",
          }),
        ],
        steers: [
          {
            id: "run-1-0",
            text: "check telegram first",
            mode: "steer",
            label: "connector audit",
            at: "2026-09-05T14:22:30Z",
          },
        ],
      }),
    );

    expect(items.map((item) => item.kind)).toEqual(["user", "user"]);
    const steered = items[1];
    if (steered?.kind !== "user") throw new Error("expected a user row");
    expect(steered.text).toBe("check telegram first");
    expect(steered.steer).toEqual({ mode: "steer", label: "connector audit" });
    expect(steered.key).toBe("srun-1-0");
  });

  // A queued follow-up rides the same row with the other pill (GAP-03): it is
  // not a chat turn either, so nothing is stored for it and the transcript is
  // the only place it is visible in the moment it was sent.
  it("shows a queued follow-up with the `follow-up` pill, not the steer one", () => {
    const items = buildTranscript(
      input({
        steers: [
          {
            id: "run-1-q0",
            text: "then write it up",
            mode: "queue",
            label: "connector audit",
            at: "2026-09-05T14:23:00Z",
          },
        ],
      }),
    );

    const queued = items[0];
    if (queued?.kind !== "user") throw new Error("expected a user row");
    expect(queued.text).toBe("then write it up");
    expect(queued.steer).toEqual({ mode: "queue", label: "connector audit" });
  });
});

describe("buildTranscript — written artifacts", () => {
  const written = {
    artifactId: "art-1",
    name: "findings.md",
    kind: "markdown",
    version: 2,
    taskId: "task-1",
    at: "2026-08-31T14:22:30Z",
  };

  it("places the card at the moment the file was written", () => {
    const items = buildTranscript(
      input({
        history: [
          message({
            id: 1,
            role: "user",
            content: "audit it",
            created_at: "2026-08-31T14:22:00Z",
          }),
          message({
            id: 2,
            role: "assistant",
            content: "done",
            created_at: "2026-08-31T14:23:00Z",
          }),
        ],
        artifacts: [written],
      }),
    );
    expect(items.map((item) => item.key)).toEqual(["m1", "a-art-1-2", "m2"]);
    expect(items[1]).toMatchObject({ kind: "artifact", entry: written });
  });

  it("keys a new version separately, so a supersede is its own card", () => {
    const items = buildTranscript(
      input({ artifacts: [written, { ...written, version: 3 }] }),
    );
    expect(items.map((item) => item.key)).toEqual(["a-art-1-2", "a-art-1-3"]);
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

describe("the run link and artifact chips (GAP-23)", () => {
  it("carries the run a stored assistant turn started", () => {
    const items = buildTranscript(
      input({
        history: [
          message({ id: 1, role: "user", content: "do the thing" }),
          message({
            id: 2,
            role: "assistant",
            content: "Starting that now.",
            task_id: "b41c8e02-9f3a-4c11-8f52-2b7d5e6a1c30",
          }),
        ],
      }),
    );

    const [user, assistant] = items;
    expect(user?.kind).toBe("user");
    expect(assistant).toMatchObject({
      kind: "assistant",
      runId: "b41c8e02-9f3a-4c11-8f52-2b7d5e6a1c30",
      artifacts: [],
    });
  });

  it("turns a completion report's links into chips beside its attachments", () => {
    const items = buildTranscript(
      input({
        history: [
          message({
            id: 3,
            role: "assistant",
            content: "Done — two files written.",
            task_id: "b41c8e02",
            attachments: [
              {
                file_id: "upload-1",
                filename: "spec.pdf",
                mime_type: "application/pdf",
                size_bytes: 12,
              },
            ],
            artifacts: [
              { id: "produced-1", name: "notes.md", kind: "markdown" },
              { id: "produced-2", name: "run.log", kind: null },
            ],
          }),
        ],
      }),
    );

    expect(items[0]).toMatchObject({
      kind: "assistant",
      runId: "b41c8e02",
      // The upload the turn carried stays where it was…
      attachments: [
        {
          fileId: "upload-1",
          filename: "spec.pdf",
          mimeType: "application/pdf",
          kind: null,
        },
      ],
      // …and the run's own output is a separate list, kind included.
      artifacts: [
        {
          fileId: "produced-1",
          filename: "notes.md",
          mimeType: null,
          kind: "markdown",
        },
        {
          fileId: "produced-2",
          filename: "run.log",
          mimeType: null,
          kind: null,
        },
      ],
    });
  });

  it("leaves an ordinary chat turn with no run and no chips", () => {
    const items = buildTranscript(
      input({
        history: [message({ id: 4, role: "assistant", content: "Sure." })],
      }),
    );
    expect(items[0]).toMatchObject({
      kind: "assistant",
      runId: null,
      artifacts: [],
    });
  });

  it("gives the live turn no run link — the delegation is only stored once", () => {
    const items = buildTranscript(
      input({
        stream: drive(OPEN, { type: "delta", content: "Wor" }),
      }),
    );
    expect(items[0]).toMatchObject({ kind: "assistant", runId: null });
  });
});
