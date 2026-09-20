/**
 * The projection from persisted rows onto the run detail's log rows (GAP-10).
 *
 * This replaces the socket-ring projection the card used before the daemon
 * could answer per run: the inputs are `event_log` rows, so the tool events
 * that previously had to be dropped for lack of attribution are now first-class
 * — and everything the daemon writes gets a row, named by its own
 * `event_type` when this build has no sentence for it.
 */

import { describe, expect, it } from "vitest";

import type { EventLogRecord } from "./types";
import { runEventsFromLog } from "./run-events";

function row(
  id: number,
  event_type: string,
  detail: Record<string, unknown> | null = null,
  task_id: string | null = "b41",
): EventLogRecord {
  return {
    id,
    timestamp: "2026-08-31T14:31:00Z",
    agent_id: null,
    task_id,
    event_type,
    detail,
  };
}

function one(record: EventLogRecord) {
  return runEventsFromLog([record], "b41")[0];
}

describe("runEventsFromLog — tags", () => {
  it("tags every tool-shaped event as `tool`", () => {
    for (const type of [
      "tool_executed",
      "tool_confirmation_requested",
      "tool_auto_approved",
      "security_violation",
      "circuit_breaker_tripped",
    ]) {
      expect(one(row(1, type, { tool_name: "shell_execute" }))?.tag).toBe(
        "tool",
      );
    }
  });

  it("tags the steer, the artifact, the span and the run frames", () => {
    expect(one(row(1, "workflow_steered"))?.tag).toBe("steer");
    expect(one(row(2, "artifact_written", { name: "a.md" }))?.tag).toBe(
      "artifact",
    );
    expect(
      one(row(3, "subagent_span", { label: "review·1", state: "done" }))?.tag,
    ).toBe("spawn");
    expect(one(row(4, "task_status", { status: "running" }))?.tag).toBe("run");
    expect(one(row(5, "workflow_progress", { message: "x" }))?.tag).toBe("run");
  });

  it("falls back to `run` for an event type this build does not know", () => {
    const event = one(row(6, "some_future_event", {}));
    expect(event?.tag).toBe("run");
    expect(event?.text).toBe("some_future_event");
  });
});

describe("runEventsFromLog — sentences", () => {
  it("folds progress counters into the status line", () => {
    expect(
      one(
        row(1, "task_status", {
          status: "running",
          progress_current: 5,
          progress_total: 8,
        }),
      )?.text,
    ).toBe("status running · 5/8");
  });

  it("omits the counters when there is no total to count against", () => {
    expect(
      one(row(2, "task_status", { status: "running", progress_total: 0 }))
        ?.text,
    ).toBe("status running");
  });

  it("falls back to a plain sentence when the title is empty", () => {
    expect(one(row(3, "workflow_started", { title: "" }))?.text).toBe(
      "workflow started",
    );
    expect(one(row(4, "workflow_started", { title: "Audit" }))?.text).toBe(
      "started · Audit",
    );
  });

  it("spells the progress narration out verbatim", () => {
    expect(
      one(row(5, "workflow_progress", { message: "read 12 files" }))?.text,
    ).toBe("read 12 files");
  });

  it("names an artifact with its version and a lane with its state", () => {
    expect(
      one(row(6, "artifact_written", { name: "report.md", version: 2 }))?.text,
    ).toBe("report.md · v2");
    expect(
      one(row(7, "subagent_span", { label: "review·1", state: "done" }))?.text,
    ).toBe("review·1 · done");
  });

  it("marks a failed tool call and leaves a successful one bare", () => {
    expect(
      one(
        row(8, "tool_executed", { tool_name: "shell_execute", success: true }),
      )?.text,
    ).toBe("shell_execute");
    expect(
      one(
        row(9, "tool_executed", { tool_name: "shell_execute", success: false }),
      )?.text,
    ).toBe("shell_execute · failed");
  });

  it("gives a refusal its reason and a trip its failure count", () => {
    expect(
      one(
        row(10, "security_violation", {
          tool_name: "shell_execute",
          reason: "not in allow list",
        }),
      )?.text,
    ).toBe("shell_execute · not in allow list");
    expect(
      one(
        row(11, "circuit_breaker_tripped", {
          tool_name: "web_search",
          consecutive_failures: 3,
        }),
      )?.text,
    ).toBe("web_search · circuit open after 3 failures");
  });

  /** Never a plausible-looking phrase built out of nothing. */
  it("falls back to the raw event type when the blob cannot fill a sentence", () => {
    expect(one(row(12, "task_status", {}))?.text).toBe("task_status");
    expect(one(row(13, "artifact_written", null))?.text).toBe(
      "artifact_written",
    );
    expect(one(row(14, "tool_executed", { success: true }))?.text).toBe(
      "tool_executed",
    );
  });
});

describe("runEventsFromLog — what it drops", () => {
  // `dag_node_status` fires for the same moment as `subagent_span` (P9 /
  // Phase 8 deletes the emitter); rendering both would fill the card with
  // pairs of the same transition.
  it("produces no row from `dag_node_status`", () => {
    expect(runEventsFromLog([row(1, "dag_node_status", {})], "b41")).toEqual(
      [],
    );
  });

  it("keeps a full run of spans undoubled by their `dag_node_status` mirrors", () => {
    const rows = ["review", "writing", "audit"].flatMap((label, i) => [
      row(100 + i, "subagent_span", { label: `${label}·1`, state: "running" }),
      row(200 + i, "subagent_span", { label: `${label}·1`, state: "done" }),
      row(300 + i, "dag_node_status", { node_id: `node-${i}` }),
      row(400 + i, "dag_node_status", { node_id: `node-${i}` }),
    ]);
    const events = runEventsFromLog(rows, "b41");
    expect(events.filter((event) => event.tag === "spawn")).toHaveLength(6);
    expect(events).toHaveLength(6);
  });

  it("guards against a page that is not this run's, rather than trusting it", () => {
    const events = runEventsFromLog(
      [
        row(1, "task_status", { status: "running" }),
        row(2, "task_status", { status: "running" }, "other"),
        row(3, "task_status", { status: "running" }, null),
      ],
      "b41",
    );
    expect(events.map((event) => event.id)).toEqual([1]);
  });

  it("preserves the server's order and stamps", () => {
    const events = runEventsFromLog(
      [row(3, "workflow_steered"), row(2, "workflow_steered")],
      "b41",
    );
    expect(events.map((event) => event.id)).toEqual([3, 2]);
    expect(events[0]?.at).toBe("2026-08-31T14:31:00Z");
  });
});
