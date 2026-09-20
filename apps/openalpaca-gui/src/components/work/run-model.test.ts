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
  steerDisabledReason,
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

  it("joins duration and steps, and never invents a cost when none is served", () => {
    expect(runMeta(task(), now)).toBe("11m 04s · 5/8 steps");
    expect(runMeta(task(), now)).not.toContain("$");
  });

  it("appends the cost segment when the list route serves cost_usd (GAP-08b)", () => {
    expect(runMeta(task({ cost_usd: 0.41 }), now)).toBe(
      "11m 04s · 5/8 steps · $0.41",
    );
  });

  it("still renders an explicit zero cost rather than omitting it", () => {
    expect(runMeta(task({ cost_usd: 0 }), now)).toBe(
      "11m 04s · 5/8 steps · $0.00",
    );
  });

  it("drops the step segment when no total is known", () => {
    expect(runMeta(task({ progress_total: null }), now)).toBe("11m 04s");
  });

  /**
   * R38 — the per-run agent signal the list route lost with P8, back as
   * `subagent_count` (one grouped `subagent_span` query per page). It sits
   * before the cost so the design's cost tail stays last.
   */
  it("names how many agents the run spawned", () => {
    expect(runMeta(task({ subagent_count: 3 }), now)).toBe(
      "11m 04s · 5/8 steps · 3 agents",
    );
    expect(runMeta(task({ subagent_count: 3, cost_usd: 0.41 }), now)).toBe(
      "11m 04s · 5/8 steps · 3 agents · $0.41",
    );
  });

  it("says 1 agent in the singular", () => {
    expect(runMeta(task({ subagent_count: 1 }), now)).toBe(
      "11m 04s · 5/8 steps · 1 agent",
    );
  });

  /**
   * Unlike the cost, zero is not a fact worth a segment: a run the lead
   * handled alone has no agents to count, and the detail route does not serve
   * the field at all.
   */
  it("omits the agent segment for a run that spawned none", () => {
    expect(runMeta(task({ subagent_count: 0 }), now)).toBe(
      "11m 04s · 5/8 steps",
    );
    expect(runMeta(task(), now)).toBe("11m 04s · 5/8 steps");
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

  it("carries cost_usd through as costUsd, null when the route omits it", () => {
    expect(toRun(task()).costUsd).toBeNull();
    expect(toRun(task({ cost_usd: 1.25 })).costUsd).toBe(1.25);
  });

  it("carries subagent_count through, null when the route omits it", () => {
    expect(toRun(task()).subagentCount).toBeNull();
    expect(toRun(task({ subagent_count: 0 })).subagentCount).toBe(0);
    expect(toRun(task({ subagent_count: 4 })).subagentCount).toBe(4);
  });

  it("carries the daemon's `steerable` hint, and trusts a row without it", () => {
    expect(toRun(task({ steerable: true })).steerable).toBe(true);
    expect(toRun(task({ steerable: false })).steerable).toBe(false);
    // A daemon too old to serve the field: leave the control alone rather
    // than disabling a button that works.
    expect(toRun(task()).steerable).toBe(true);
  });

  it("reads a connector run off `created_by`", () => {
    expect(toRun(task()).startedElsewhere).toBe(false);
    expect(toRun(task({ created_by: "telegram:4242" })).startedElsewhere).toBe(
      true,
    );
  });
});

describe("steerDisabledReason", () => {
  it("is null for a run the daemon says it would take a message for", () => {
    expect(steerDisabledReason(toRun(task({ steerable: true })))).toBeNull();
  });

  it("names the channel a run it cannot steer came from", () => {
    const run = toRun(task({ steerable: false, created_by: "telegram:4242" }));
    expect(steerDisabledReason(run)).toMatch(/another channel/i);
  });

  it("says a finished run is finished", () => {
    const run = toRun(task({ steerable: false, status: "completed" }));
    expect(steerDisabledReason(run)).toMatch(/finished/i);
  });

  it("does not guess a reason it cannot know", () => {
    const reason = steerDisabledReason(
      toRun(task({ steerable: false, status: "queued" })),
    );
    expect(reason).not.toBeNull();
    expect(reason).not.toMatch(/another channel|finished/i);
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
