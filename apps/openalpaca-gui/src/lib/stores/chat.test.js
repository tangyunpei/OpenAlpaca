// @ts-nocheck
import { describe, expect, it } from "bun:test";
import { applyDoneDataToMessage } from "./chat";

function makeAssistantMessage() {
  return {
    id: 1,
    lane_key: "user:gui",
    role: "assistant",
    content: "Thinking...",
    created_at: "2026-02-25T00:00:00.000Z",
  };
}

describe("applyDoneDataToMessage", () => {
  it("writes citations and artifacts from done payload", () => {
    const base = makeAssistantMessage();
    const done = {
      content: "Final answer",
      model: "gpt-5-mini",
      tokens_in: 12,
      tokens_out: 34,
      duration_ms: 56,
      citations: [
        {
          source_file_id: "file-1",
          excerpt: "evidence text",
          page: 3,
        },
      ],
      artifacts: [
        {
          file_id: "artifact-1",
          label: "report.md",
          mime_type: "text/markdown",
        },
      ],
    };

    const updated = applyDoneDataToMessage(base, done);
    expect(updated.content).toBe("Final answer");
    expect(updated.model).toBe("gpt-5-mini");
    expect(updated.citations?.[0]?.source_file_id).toBe("file-1");
    expect(updated.artifacts?.[0]?.file_id).toBe("artifact-1");
  });

  it("keeps optional citation/artifact fields undefined when absent", () => {
    const base = makeAssistantMessage();
    const done = {
      content: "No sources",
      model: "gpt-5-mini",
      tokens_in: 1,
      tokens_out: 2,
      duration_ms: 3,
    };

    const updated = applyDoneDataToMessage(base, done);
    expect(updated.citations).toBeUndefined();
    expect(updated.artifacts).toBeUndefined();
  });
});
