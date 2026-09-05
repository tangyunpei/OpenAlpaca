import { describe, expect, it } from "vitest";

import {
  assistantMetaLine,
  formatClock,
  formatDurationMs,
  formatElapsed,
  formatToolArguments,
  shortModelId,
  shortRunId,
  shortTitle,
} from "./format";

describe("shortModelId (§3.10)", () => {
  it("strips the vendor prefix the design drops", () => {
    expect(shortModelId("claude-sonnet-4-6")).toBe("sonnet-4-6");
  });

  it("leaves an unrecognised id alone", () => {
    expect(shortModelId("llama3.2:8b")).toBe("llama3.2:8b");
  });

  it("keeps the last path segment of a namespaced id", () => {
    expect(shortModelId("openrouter/claude-sonnet-4-6")).toBe("sonnet-4-6");
  });
});

describe("formatDurationMs (§3.12)", () => {
  it("uses one decimal below a minute", () => {
    expect(formatDurationMs(3800)).toBe("3.8s");
  });

  it("zero-pads the seconds above a minute", () => {
    expect(formatDurationMs(372_000)).toBe("6m 12s");
    expect(formatDurationMs(664_000)).toBe("11m 04s");
  });
});

describe("assistantMetaLine — 1:1 with the SSE `done` payload", () => {
  it("renders model · duration · tokens", () => {
    expect(
      assistantMetaLine({
        model: "claude-sonnet-4-6",
        durationMs: 3800,
        tokensIn: 1284,
        tokensOut: 612,
      }),
    ).toBe("sonnet-4-6 · 3.8s · 1284/612 tok");
  });

  it("omits a segment the daemon did not send rather than zeroing it", () => {
    expect(assistantMetaLine({ model: "claude-sonnet-4-6" })).toBe(
      "sonnet-4-6",
    );
    expect(assistantMetaLine({})).toBeNull();
  });
});

describe("formatClock / formatElapsed", () => {
  it("formats 24-hour, zero-padded", () => {
    expect(formatClock(new Date(2026, 7, 31, 9, 4))).toBe("09:04");
  });

  it("returns null rather than guessing at an unusable stamp", () => {
    expect(formatClock("not-a-date")).toBeNull();
    expect(formatClock(null)).toBeNull();
    expect(formatElapsed(null, "2026-08-31T10:00:00Z")).toBeNull();
  });

  it("spans two stamps", () => {
    expect(formatElapsed("2026-08-31T10:00:00Z", "2026-08-31T10:06:12Z")).toBe(
      "6m 12s",
    );
  });
});

describe("shortRunId / shortTitle", () => {
  it("takes the 8-hex head of a task id", () => {
    expect(shortRunId("b41c8e02-4f1a-4a1e-9f0e-1c2d3e4f5061")).toBe("b41c8e02");
  });

  it("truncates a real title instead of inventing a short one", () => {
    expect(shortTitle("Audit every connector for stale credentials")).toBe(
      "Audit every connector…",
    );
    expect(shortTitle("Connector audit")).toBe("Connector audit");
  });
});

describe("formatToolArguments (§3.14)", () => {
  it("shows the literal command for a single-command argument", () => {
    expect(formatToolArguments({ command: "cargo tree -d" })).toBe(
      "cargo tree -d",
    );
  });

  it("pretty-prints anything else rather than flattening it", () => {
    expect(formatToolArguments({ path: "/tmp/a", mode: "w" })).toContain(
      '"path": "/tmp/a"',
    );
  });

  it("passes a bare string through", () => {
    expect(formatToolArguments("ls -la")).toBe("ls -la");
  });
});
