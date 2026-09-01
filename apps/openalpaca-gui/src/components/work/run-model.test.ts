import { describe, expect, it } from "vitest";

import type { Task } from "@/lib/api/types";

import {
  formatClock,
  formatDuration,
  isRaisedRun,
  isTerminalRun,
  kindFromName,
  parseOutcomeArtifacts,
  parseTimestamp,
  partitionRuns,
  runMeta,
  toRun,
} from "./run-model";

const BASE: Task = {
  id: "b41c8e02-0000-0000-0000-000000000000",
  title: "Audit the connector surface",
  description: null,
  status: "running",
  priority: 0,
  progress_current: 5,
  progress_total: 8,
  result_summary: null,
  created_by: "user",
  source_lane: "local:gui",
  created_at: "2026-08-31T14:22:41Z",
  updated_at: "2026-08-31T14:33:45Z",
  completed_at: null,
  state_version: 1,
};

const task = (patch: Partial<Task> = {}): Task => ({ ...BASE, ...patch });

describe("parseTimestamp", () => {
  it("reads RFC 3339", () => {
    expect(parseTimestamp("2026-08-31T14:22:41Z")?.getTime()).toBe(
      Date.parse("2026-08-31T14:22:41Z"),
    );
  });

  it("treats a space-separated stamp with no zone as UTC", () => {
    expect(parseTimestamp("2026-08-31 14:22:41")?.getTime()).toBe(
      Date.parse("2026-08-31T14:22:41Z"),
    );
  });

  it("rejects empty and unparseable values", () => {
    expect(parseTimestamp(null)).toBeNull();
    expect(parseTimestamp("")).toBeNull();
    expect(parseTimestamp("not a date")).toBeNull();
  });
});

describe("formatDuration", () => {
  it("uses the design's three shapes", () => {
    expect(formatDuration(41_000)).toBe("41s");
    expect(formatDuration(664_000)).toBe("11m 04s");
    expect(formatDuration(14_520_000)).toBe("4h 02m");
  });

  it("never renders a negative duration", () => {
    expect(formatDuration(-5_000)).toBe("0s");
  });
});

describe("formatClock", () => {
  it("renders local hours, with and without seconds", () => {
    const date = new Date("2026-08-31T14:22:41Z");
    const hh = String(date.getHours()).padStart(2, "0");
    const mm = String(date.getMinutes()).padStart(2, "0");
    expect(formatClock(BASE.created_at)).toBe(`${hh}:${mm}`);
    expect(formatClock(BASE.created_at, true)).toBe(`${hh}:${mm}:41`);
  });
});

describe("runMeta", () => {
  const now = new Date("2026-08-31T14:33:45Z");

  it("joins duration and steps, and never invents a cost (GAP-08)", () => {
    expect(runMeta(task(), now)).toBe("11m 04s · 5/8 steps");
    expect(runMeta(task(), now)).not.toContain("$");
  });

  it("drops the step segment when no total is known", () => {
    expect(runMeta(task({ progress_total: null }), now)).toBe("11m 04s");
  });

  it("measures a terminal run to its completion, not to now", () => {
    const finished = task({
      status: "completed",
      completed_at: "2026-08-31T14:25:41Z",
    });
    expect(runMeta(finished, now)).toBe("3m 00s · 5/8 steps");
  });
});

describe("toRun", () => {
  it("maps `completed` onto the design's `done`", () => {
    expect(toRun(task({ status: "completed" })).status).toBe("done");
  });

  it("carries `failed` through rather than painting it as done", () => {
    expect(toRun(task({ status: "failed" })).status).toBe("failed");
  });

  it("takes the note from the daemon's own summary, first line only", () => {
    const run = toRun(
      task({ result_summary: "two files written\nplus detail" }),
    );
    expect(run.note).toBe("two files written");
  });

  it("has no note when the daemon supplied none", () => {
    expect(toRun(task()).note).toBeNull();
  });
});

describe("status predicates", () => {
  it("raises only running and paused cards (§3.19)", () => {
    expect(isRaisedRun("running")).toBe(true);
    expect(isRaisedRun("paused")).toBe(true);
    expect(isRaisedRun("queued")).toBe(false);
    expect(isRaisedRun("done")).toBe(false);
  });

  it("treats done, cancelled and failed as terminal", () => {
    expect(isTerminalRun("done")).toBe(true);
    expect(isTerminalRun("cancelled")).toBe(true);
    expect(isTerminalRun("failed")).toBe(true);
    expect(isTerminalRun("queued")).toBe(false);
  });
});

describe("parseOutcomeArtifacts", () => {
  it("reads names, ids and kinds out of free-form entries", () => {
    const parsed = parseOutcomeArtifacts([
      { id: "a1", name: "findings.md" },
      { filename: "audit.csv" },
      "notes.txt",
    ]);
    expect(parsed).toEqual([
      { id: "a1", name: "findings.md", kind: "md", stamp: null },
      { id: null, name: "audit.csv", kind: "table", stamp: null },
      { id: null, name: "notes.txt", kind: "term", stamp: null },
    ]);
  });

  it("prefers a declared kind over the extension", () => {
    expect(
      parseOutcomeArtifacts([{ name: "plan.md", kind: "plan" }])[0]?.kind,
    ).toBe("plan");
  });

  it("drops entries with no readable name rather than rendering a blank row", () => {
    expect(parseOutcomeArtifacts([{ size: 12 }, null, 7, ""])).toEqual([]);
  });

  it("returns nothing for a task with no outcome", () => {
    expect(parseOutcomeArtifacts(undefined)).toEqual([]);
  });
});

describe("kindFromName", () => {
  it("maps the design's seven kinds off the extension", () => {
    expect(kindFromName("a.md")).toBe("md");
    expect(kindFromName("a.rs")).toBe("code");
    expect(kindFromName("a.csv")).toBe("table");
    expect(kindFromName("a.html")).toBe("html");
    expect(kindFromName("a.png")).toBe("image");
    expect(kindFromName("a.log")).toBe("term");
    expect(kindFromName("Makefile")).toBe("term");
  });
});

describe("partitionRuns", () => {
  const now = new Date("2026-08-31T15:00:00Z");

  it("keeps cancelled and failed runs in the live list, per §4.2", () => {
    const runs = [
      toRun(task({ id: "r1", status: "running" }), now),
      toRun(task({ id: "r2", status: "cancelled" }), now),
      toRun(task({ id: "r3", status: "failed" }), now),
      toRun(
        task({
          id: "r4",
          status: "completed",
          completed_at: "2026-08-31T14:40:00Z",
        }),
        now,
      ),
    ];
    const partition = partitionRuns(runs, now);
    expect(partition.live.map((run) => run.id)).toEqual(["r1", "r2", "r3"]);
    expect(partition.completedToday.map((run) => run.id)).toEqual(["r4"]);
  });

  it("counts only running/queued/paused as active", () => {
    const runs = [
      toRun(task({ id: "r1", status: "running" }), now),
      toRun(task({ id: "r2", status: "queued" }), now),
      toRun(task({ id: "r3", status: "paused" }), now),
      toRun(task({ id: "r4", status: "cancelled" }), now),
    ];
    expect(partitionRuns(runs, now).activeCount).toBe(3);
  });

  it("excludes runs that finished on an earlier day", () => {
    const runs = [
      toRun(
        task({
          id: "old",
          status: "completed",
          completed_at: "2026-08-29T09:00:00Z",
        }),
        now,
      ),
    ];
    const partition = partitionRuns(runs, now);
    expect(partition.completedToday).toEqual([]);
    expect(partition.doneCount).toBe(1);
  });
});
