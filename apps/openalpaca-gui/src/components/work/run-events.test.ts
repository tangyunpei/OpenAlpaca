import { describe, expect, it } from "vitest";

import type { ServerEvent } from "@/lib/events";

import { runEventsFromRing } from "./run-events";

const base = { ts: "2026-08-31T14:31:00Z", instance_id: "7f3a" };

const RING: ServerEvent[] = [
  {
    type: "workflow_started",
    task_id: "b41",
    lane_key: "l",
    title: "Audit",
    _id: 1,
    ...base,
  },
  {
    type: "workflow_progress",
    task_id: "b41",
    lane_key: "l",
    message: "read 12 files",
    _id: 2,
    ...base,
  },
  { type: "workflow_steered", task_id: "b41", lane_key: "l", _id: 3, ...base },
  {
    type: "task_status",
    task_id: "b41",
    title: "",
    status: "running",
    progress_current: 5,
    progress_total: 8,
    result_summary: null,
    _id: 4,
    ...base,
  },
  {
    type: "workflow_progress",
    task_id: "other",
    lane_key: "l",
    message: "not mine",
    _id: 5,
    ...base,
  },
  {
    type: "tool_executed",
    agent_id: "a1",
    tool_name: "shell_execute",
    success: true,
    duration_ms: 12,
    _id: 6,
    ...base,
  },
  { type: "heartbeat", _id: 7, ...base },
];

describe("runEventsFromRing", () => {
  const events = runEventsFromRing(RING, "b41");

  it("keeps only the events that carry this task id", () => {
    expect(events.map((event) => event.id)).toEqual([4, 3, 2, 1]);
  });

  it("drops tool events, which carry an agent id and no task id (GAP-10)", () => {
    expect(events.some((event) => event.tag === "tool")).toBe(false);
  });

  it("tags a steer as `steer` and everything else as `run`", () => {
    expect(events.find((event) => event.id === 3)?.tag).toBe("steer");
    expect(events.find((event) => event.id === 2)?.tag).toBe("run");
  });

  it("spells the progress narration out verbatim", () => {
    expect(events.find((event) => event.id === 2)?.text).toBe("read 12 files");
  });

  it("folds progress counters into a status line", () => {
    expect(events.find((event) => event.id === 4)?.text).toBe(
      "status running · 5/8",
    );
  });

  it("falls back to a plain sentence when the title is empty", () => {
    const [started] = runEventsFromRing(
      [
        {
          type: "workflow_started",
          task_id: "b41",
          lane_key: "l",
          title: "",
          _id: 9,
          ...base,
        },
      ],
      "b41",
    );
    expect(started?.text).toBe("workflow started");
  });

  it("caps the list the way the design does", () => {
    expect(runEventsFromRing(RING, "b41", 2)).toHaveLength(2);
  });

  it("returns nothing for a run with no matching frames", () => {
    expect(runEventsFromRing(RING, "unknown")).toEqual([]);
  });
});
