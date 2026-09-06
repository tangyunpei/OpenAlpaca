/**
 * The honest-degradation contract for the three work-detail cards: each one
 * renders its real component, and where the daemon cannot answer it says which
 * route is missing instead of drawing rows that look like data.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { OutcomeArtifact } from "@/components/work/run-model";
import type { TaskTimeline } from "@/lib/api/tasks";
import type { RunEvent } from "@/lib/api/unbacked";

import { EventLogSection, EVENT_LOG_EMPTY } from "./EventLogSection";
import { OutputSection, OUTPUT_EMPTY } from "./OutputSection";
import { TimelineSection, TIMELINE_EMPTY } from "./TimelineSection";

const timeline = (lanes: TaskTimeline["lanes"]): TaskTimeline => ({
  task_id: "b41",
  started_at: "2026-08-31T14:22:41Z",
  now: "2026-08-31T14:32:41Z",
  completed_at: null,
  lanes,
});

describe("TimelineSection", () => {
  it("shows the design's empty copy while the timeline is loading", () => {
    render(<TimelineSection timeline={null} />);
    expect(screen.getByText("Timeline")).toBeInTheDocument();
    expect(screen.getByText(TIMELINE_EMPTY)).toBeInTheDocument();
  });

  it("shows the same copy for a run that spawned nothing", () => {
    render(<TimelineSection timeline={timeline([])} />);
    expect(screen.getByText(TIMELINE_EMPTY)).toBeInTheDocument();
  });

  it("draws the real swimlanes, in-flight lane included", () => {
    render(
      <TimelineSection
        timeline={timeline([
          {
            lane_id: "l1",
            label: "lead·1",
            template_id: "lead_agent",
            agent_instance_id: "a1",
            started_at: "2026-08-31T14:22:41Z",
            ended_at: null,
            state: "running",
            detail: null,
          },
          {
            lane_id: "l2",
            label: "review·1",
            template_id: "review_agent",
            agent_instance_id: "a2",
            started_at: "2026-08-31T14:24:00Z",
            ended_at: null,
            state: "blocked",
            detail: "waiting on shell_execute",
          },
        ])}
        blocked
      />,
    );
    expect(screen.getByText("lead·1")).toBeInTheDocument();
    expect(screen.getByText("review·1")).toBeInTheDocument();
    expect(screen.getByText("waiting on shell_execute")).toBeInTheDocument();
    expect(screen.queryByText(TIMELINE_EMPTY)).not.toBeInTheDocument();
    expect(screen.queryByText(/not yet available/)).not.toBeInTheDocument();
  });
});

describe("OutputSection", () => {
  const artifact = (patch: Partial<OutcomeArtifact> = {}): OutcomeArtifact => ({
    id: null,
    name: "findings.md",
    kind: "md",
    stamp: "14:31",
    ...patch,
  });

  it("uses the design's empty copy when the run produced nothing", () => {
    render(<OutputSection artifacts={[]} count={0} />);
    expect(screen.getByText(OUTPUT_EMPTY)).toBeInTheDocument();
  });

  it("says so when a run counted files the list did not return", () => {
    render(
      <OutputSection
        artifacts={[]}
        count={3}
        note="The run's files could not be listed — database is locked"
      />,
    );
    expect(screen.getByText(/reported 3 files/)).toBeInTheDocument();
    expect(screen.getByText(/database is locked/)).toBeInTheDocument();
  });

  it("lists what the outcome did report, unclickable without an id", () => {
    render(
      <OutputSection artifacts={[artifact()]} count={1} onOpen={vi.fn()} />,
    );
    expect(screen.getByText("findings.md")).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("becomes a button the moment an artifact has an id", () => {
    render(
      <OutputSection
        artifacts={[artifact({ id: "art-1" })]}
        count={1}
        onOpen={vi.fn()}
      />,
    );
    expect(
      screen.getByRole("button", { name: /findings\.md/ }),
    ).toBeInTheDocument();
  });
});

describe("EventLogSection (GAP-10)", () => {
  const event: RunEvent = {
    id: 4,
    task_id: "b41",
    tag: "steer",
    text: "steering message delivered",
    at: "2026-08-31T14:31:00Z",
  };

  it("shows the design's empty copy and names the proposed filter", () => {
    render(<EventLogSection events={[]} />);
    expect(screen.getByText(EVENT_LOG_EMPTY)).toBeInTheDocument();
    expect(screen.getByText(/task_id=/)).toBeInTheDocument();
  });

  it("renders the live rows it does have, and still states the limitation", () => {
    render(<EventLogSection events={[event]} />);
    expect(screen.getByText("steering message delivered")).toBeInTheDocument();
    expect(screen.getByText("steer")).toBeInTheDocument();
    expect(
      screen.getByText(/Live events from this session only/),
    ).toBeInTheDocument();
  });
});
