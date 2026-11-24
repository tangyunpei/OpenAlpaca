/**
 * The chat stream's frames, this side (CONTRACT-SAMPLES-2; *Rev 107*'s
 * chat-stream item, (b)): every frame `GET /v1/chat/stream/{id}` sends —
 * one sample per SSE event name in `./contract/stream/`, each frame's
 * `data`, written by the daemon's tests through `routes/chat.rs`'s
 * `sse_frame` — parses as what `lib/chat-stream.ts`'s `attachChatStream`
 * reads it as, and every frame kind that reader listens for has its sample.
 * A frame kind the daemon drops fails the daemon's bump check, and once the
 * folder is rewritten, fails here.
 *
 * `done` and `confirmation_requested` are read as their exported types;
 * the other five as the one or two keys their listeners read, named here.
 */

import { describe, expect, it } from "vitest";

import type { ChatConfirmationRequest, ChatStreamDone } from "../chat-stream";
import { check, shape, type Node } from "./contract-shape";

const streamSamples = new Map(
  Object.entries(
    import.meta.glob("./contract/stream/*.json", {
      eager: true,
      import: "default",
    }) as Record<string, unknown>,
  ).map(([path, value]) => [
    path.replace(/^.*\//, "").replace(/\.json$/, ""),
    value,
  ]),
);

/** What the listeners read of the frames that have no exported type. */
interface ReasoningFrame {
  text: string;
}
interface DeltaFrame {
  content: string;
}
interface ErrorFrame {
  message: string;
}
interface ConfirmationResolvedFrame {
  request_id: string;
  outcome: string;
}
// `thinking` carries `{}`: its listener reads nothing.
type ThinkingFrame = Record<never, never>;

const FRAMES: Record<string, { type: string; node: Node<unknown> }> = {
  thinking: {
    type: "ThinkingFrame",
    node: shape<ThinkingFrame>({}, {}) as Node<unknown>,
  },
  reasoning: {
    type: "ReasoningFrame",
    node: shape<ReasoningFrame>({ text: "required" }, {}) as Node<unknown>,
  },
  delta: {
    type: "DeltaFrame",
    node: shape<DeltaFrame>({ content: "required" }, {}) as Node<unknown>,
  },
  error: {
    type: "ErrorFrame",
    node: shape<ErrorFrame>({ message: "required" }, {}) as Node<unknown>,
  },
  confirmation_resolved: {
    type: "ConfirmationResolvedFrame",
    node: shape<ConfirmationResolvedFrame>(
      { request_id: "required", outcome: "required" },
      {},
    ) as Node<unknown>,
  },
  done: {
    type: "ChatStreamDone",
    node: shape<ChatStreamDone>(
      {
        content: "required",
        model: "required",
        tokens_in: "required",
        tokens_out: "required",
        duration_ms: "required",
        attachments_used: "optional",
        attachments_skipped: "optional",
        memory_used: "optional",
        delegation: "optional",
        working_state_updated: "optional",
        continued_from: "optional",
      },
      {
        attachments_skipped: shape<
          NonNullable<ChatStreamDone["attachments_skipped"]>[number]
        >({ id: "required", reason: "required" }, {}),
        memory_used: shape<NonNullable<ChatStreamDone["memory_used"]>[number]>(
          { id: "required", revision: "required", reason: "required" },
          {},
        ),
        delegation: shape<NonNullable<ChatStreamDone["delegation"]>>(
          { task_id: "required", title: "required" },
          {},
        ),
        continued_from: shape<NonNullable<ChatStreamDone["continued_from"]>>(
          { session_id: "required", topic: "required" },
          {},
        ),
      },
    ) as Node<unknown>,
  },
  confirmation_requested: {
    type: "ChatConfirmationRequest",
    node: shape<ChatConfirmationRequest>(
      {
        request_id: "required",
        tool_name: "required",
        tool_arguments: "required",
        // The reader's own: the frame is this stream's, so it is never sent.
        stream_id: "may_omit",
        preview: "optional",
        per_call: "optional",
      },
      {},
    ) as Node<unknown>,
  },
};

function sorted(names: Iterable<string>): string[] {
  return [...names].toSorted();
}

describe("the chat stream's frame samples", () => {
  it("every frame sample is read here, and every frame read has its sample", () => {
    expect(sorted(streamSamples.keys())).toEqual(sorted(Object.keys(FRAMES)));
  });

  it.each(sorted(Object.keys(FRAMES)))(
    "stream/%s parses as the frame the stream reader reads",
    (name) => {
      const entry = FRAMES[name];
      const sample = streamSamples.get(name);
      expect(sample, `stream/${name}.json`).toBeDefined();
      if (entry === undefined) return;
      expect(
        check(sample, entry.node),
        `stream/${name}.json read as ${entry.type}`,
      ).toEqual([]);
    },
  );
});
