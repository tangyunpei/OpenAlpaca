/**
 * The honest-degradation contract for the three work-detail cards: each one
 * renders its real component, and where the daemon cannot answer it says which
 * route is missing instead of drawing rows that look like data.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { TaskAgentAssignment } from "@/lib/api/types";
import type { OutcomeArtifact } from "@/components/work/run-model";
import type { RunEvent } from "@/lib/api/unbacked";
import { available, unavailable } from "@/lib/unavailable";

import { EventLogSection, EVENT_LOG_EMPTY } from "./EventLogSection";
import { OutputSection, OUTPUT_EMPTY } from "./OutputSection";
import { TimelineSection, TIMELINE_EMPTY } from "./TimelineSection";

const assignment = (
  patch: Partial<TaskAgentAssignment> = {},
): TaskAgentAssignment => ({
  id: "h1",
  task_id: "b41",
  agent_id: "explore_agent_1",
  role: "explore",
  status: "completed",
  runtime_seconds: 42,
  completed_at: "2026-08-31T14:27:41Z",
  ...patch,
});

describe("TimelineSection (GAP-09)", () => {
  it("shows the design's empty copy and names the proposed route", () => {
    render(
      <TimelineSection timeline={unavailable("GAP-09")} assignments={[]} />,
    );
    expect(screen.getByText("Timeline")).toBeInTheDocument();
    expect(screen.getByText(TIMELINE_EMPTY)).toBeInTheDocument();
    expect(
      screen.getByText(/\/v1\/tasks\/\{id\}\/timeline/),
    ).toBeInTheDocument();
  });

  it("falls back to the agent runs the daemon does serve, clearly labelled", () => {
    render(
      <TimelineSection
        timeline={unavailable("GAP-09")}
        assignments={[assignment()]}
      />,
    );
    expect(screen.getByText("explore")).toBeInTheDocument();
    expect(screen.getByText("explore_agent_1")).toBeInTheDocument();
    expect(screen.getByText(/42s · completed/)).toBeInTheDocument();
    expect(screen.getByText(/Agent runs, not a timeline/)).toBeInTheDocument();
  });

  it("draws the real swimlanes once a timeline exists", () => {
    render(
      <TimelineSection
        timeline={available({
          task_id: "b41",
          started_at: "2026-08-31T14:22:41Z",
          now: "2026-08-31T14:32:41Z",
          completed_at: null,
          lanes: [
            {
              lane_id: "l1",
              label: "lead",
              template_id: "lead_agent",
              agent_instance_id: "a1",
              started_at: "2026-08-31T14:22:41Z",
              ended_at: null,
              state: "running",
              detail: null,
            },
          ],
        })}
        assignments={[]}
      />,
    );
    expect(screen.getByText("lead")).toBeInTheDocument();
    expect(screen.queryByText(/not yet available/)).not.toBeInTheDocument();
  });
});

describe("OutputSection (GAP-04)", () => {
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

  it("says so when a run counted files it cannot list, and names the route", () => {
    render(<OutputSection artifacts={[]} count={3} />);
    expect(screen.getByText(/reported 3 files/)).toBeInTheDocument();
    expect(screen.getByText(/\/v1\/artifacts/)).toBeInTheDocument();
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
