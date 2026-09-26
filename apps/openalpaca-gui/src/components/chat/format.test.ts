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

  /**
   * G4 — every assistant row in the transcript read `0/0 tok`. The counts are
   * served as an explicit `null` for a row that has none, and `null ?? 0`
   * turned "not measured" into "measured zero" under every answer on screen.
   */
  it("prints a dash for counts the daemon says it does not have", () => {
    expect(
      assistantMetaLine({
        model: "qwen3:8b",
        durationMs: 4200,
        tokensIn: null,
        tokensOut: null,
      }),
    ).toBe("qwen3:8b · 4.2s · — tok");

    // A real count is still a count, zero included: a turn that really used
    // no input tokens is not the same fact as one nobody counted.
    expect(assistantMetaLine({ tokensIn: 0, tokensOut: 0 })).toBe("0/0 tok");
    expect(assistantMetaLine({ tokensIn: 2279, tokensOut: null })).toBe(
      "2279/0 tok",
    );
    // A daemon that serves no such field at all still gets no segment.
    expect(assistantMetaLine({ model: "qwen3:8b" })).toBe("qwen3:8b");
  });

  it("does not throw on `model: null` (every row before GAP-13, and every template answer) — omits the segment", () => {
    expect(
      assistantMetaLine({
        model: null,
        durationMs: 3800,
        tokensIn: 1284,
        tokensOut: 612,
      }),
    ).toBe("3.8s · 1284/612 tok");
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

  /**
   * A value's own line break would reach the box as layout, where it looks
   * exactly like a soft wrap: `echo … wait` then `rm -rf ~` on the next line
   * reads as one long echo while the shell runs two commands. A string with
   * any control character JSON escapes is shown as JSON — `\n`, `\t`, `\r`
   * written out — as every other approval surface shows it.
   */
  it("shows a value holding a line break, a tab or another control character as JSON", () => {
    const command = "echo please wait\nrm -rf ~";
    expect(formatToolArguments({ command })).toBe(
      '{\n  "command": "echo please wait\\nrm -rf ~"\n}',
    );
    expect(formatToolArguments({ path: "/srv/allowed\tfile" })).toBe(
      '{\n  "path": "/srv/allowed\\tfile"\n}',
    );
    expect(formatToolArguments("done\r x\u{1B}[2J")).toBe(
      '"done\\r x\\u001b[2J"',
    );
    // Any C0 control, not only the three above: an ESC alone steps aside
    // too, as JSON and every other approval surface write it out.
    expect(formatToolArguments({ command: "a\u{1B}b" })).toBe(
      '{\n  "command": "a\\u001bb"\n}',
    );
  });

  /**
   * Bare, a value has no mark where it ends: a trailing space is not seen,
   * a leading one is a slight indent, and an empty value draws no box at all.
   * `std::fs` does not trim, so `/srv/allowed ` is another file. JSON's quotes
   * show the value's extent, as the CLI and the chat apps show it.
   */
  it("shows a value that is empty or starts or ends with a space as JSON", () => {
    expect(formatToolArguments({ path: "/srv/allowed " })).toBe(
      '{\n  "path": "/srv/allowed "\n}',
    );
    expect(formatToolArguments({ command: " /etc/passwd" })).toBe(
      '{\n  "command": " /etc/passwd"\n}',
    );
    expect(formatToolArguments({ path: "" })).toBe('{\n  "path": ""\n}');
    expect(formatToolArguments("ls -la ")).toBe('"ls -la "');
    expect(formatToolArguments("")).toBe('""');
    // A space inside the value keeps the bare form.
    expect(formatToolArguments({ command: "ls  -la /tmp" })).toBe(
      "ls  -la /tmp",
    );
  });
});
