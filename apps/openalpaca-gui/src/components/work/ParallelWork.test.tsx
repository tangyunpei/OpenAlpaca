import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { laneColor, showsPending } from "@/components/ui";
import type { TaskTimeline, TimelineLane } from "@/lib/api/unbacked";
import { available, unavailable } from "@/lib/unavailable";

import {
  axisLabels,
  lanesFromTimeline,
  ParallelWorkBlock,
} from "./ParallelWork";

const lane = (patch: Partial<TimelineLane> = {}): TimelineLane => ({
  lane_id: "l1",
  label: "explore·1",
  template_id: "explore_agent",
  agent_instance_id: "a1",
  started_at: "2026-08-31T14:22:41Z",
  ended_at: "2026-08-31T14:27:41Z",
  state: "done",
  detail: "12 files read",
  ...patch,
});

const timeline = (lanes: TimelineLane[]): TaskTimeline => ({
  task_id: "b41c8e02",
  started_at: "2026-08-31T14:22:41Z",
  now: "2026-08-31T14:32:41Z",
  completed_at: null,
  lanes,
});

describe("lanesFromTimeline", () => {
  it("places a span as a percentage of the run's wall clock", () => {
    const [bar] = lanesFromTimeline(timeline([lane()]));
    expect(bar?.start).toBe(0);
    expect(bar?.end).toBe(50);
  });

  it("runs an unfinished span out to now", () => {
    const [bar] = lanesFromTimeline(
      timeline([lane({ ended_at: null, state: "running" })]),
    );
    expect(bar?.end).toBe(100);
    expect(bar?.state).toBe("run");
  });

  it("maps a blocked span onto the design's `block` state", () => {
    const [bar] = lanesFromTimeline(timeline([lane({ state: "blocked" })]));
    expect(bar?.state).toBe("block");
  });

  it("draws a failed span neutral, not green, and says so in the detail", () => {
    const [bar] = lanesFromTimeline(
      timeline([lane({ state: "failed", detail: null })]),
    );
    // §3.21 has no `failed` colour; grey plus the word beats claiming success.
    expect(bar?.state).toBe("run");
    expect(bar?.detail).toBe("failed");
  });

  it("skips a span with no readable start rather than pinning it at zero", () => {
    expect(lanesFromTimeline(timeline([lane({ started_at: "" })]))).toEqual([]);
  });

  it("derives steps for the trailing detail when the daemon sends none", () => {
    const [bar] = lanesFromTimeline(
      timeline([lane({ detail: null, steps_current: 3, steps_total: 8 })]),
    );
    expect(bar?.detail).toBe("3/8 steps");
  });
});

describe("blocked coupling (§4.4)", () => {
  it("turns a block lane red with a pending hatch only while blocked", () => {
    expect(laneColor("block", true)).toBe("var(--color-red)");
    expect(showsPending("block", true)).toBe(true);
  });

  it("turns the same lane green and drops the hatch once resolved", () => {
    expect(laneColor("block", false)).toBe("var(--color-green)");
    expect(showsPending("block", false)).toBe(false);
  });

  it("leaves the other two lane states untouched by `blocked`", () => {
    expect(laneColor("done", true)).toBe(laneColor("done", false));
    expect(laneColor("run", true)).toBe(laneColor("run", false));
    expect(showsPending("run", true)).toBe(false);
  });
});

describe("axisLabels", () => {
  it("marks the live end of the axis with `now`", () => {
    const labels = axisLabels(timeline([lane()]));
    expect(labels?.[2]).toContain("now");
  });

  it("uses the completion time once the run has finished", () => {
    const labels = axisLabels({
      ...timeline([lane()]),
      completed_at: "2026-08-31T14:30:00Z",
    });
    expect(labels?.[2]).not.toContain("now");
  });
});

describe("ParallelWorkBlock", () => {
  it("names the missing route instead of drawing empty lanes (GAP-09)", () => {
    render(<ParallelWorkBlock timeline={unavailable("GAP-09")} />);
    expect(screen.getByText("Parallel work")).toBeInTheDocument();
    expect(
      screen.getByText(/Subagent timeline not yet available/i),
    ).toBeInTheDocument();
  });

  it("renders one labelled lane per span when the route exists", () => {
    render(
      <ParallelWorkBlock
        timeline={available(
          timeline([
            lane(),
            lane({ lane_id: "l2", label: "review·3", state: "blocked" }),
          ]),
        )}
      />,
    );
    expect(screen.getByText("explore·1")).toBeInTheDocument();
    expect(screen.getByText("review·3")).toBeInTheDocument();
    expect(screen.queryByText(/not yet available/i)).not.toBeInTheDocument();
  });

  it("says so plainly when the run has no spans at all", () => {
    render(<ParallelWorkBlock timeline={available(timeline([]))} />);
    expect(screen.getByText("No subagent spans yet.")).toBeInTheDocument();
  });
});
