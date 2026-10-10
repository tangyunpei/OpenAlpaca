/**
 * The honest-degradation contract for the three work-detail cards: each one
 * renders its real component, and where the daemon cannot answer it says which
 * route is missing instead of drawing rows that look like data.
 */

import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { OutcomeArtifact } from "@/components/work/run-model";
import type { TaskTimeline } from "@/lib/api/tasks";
import type { RunEvent } from "@/lib/api/run-events";

import {
  EventLogSection,
  EVENT_LOG_EMPTY,
  EVENT_LOG_ERROR,
} from "./EventLogSection";
import {
  OutputSection,
  OUTPUT_EMPTY,
  OUTPUT_EMPTY_BESIDE_PROJECT,
  PROJECT_FILES_TITLE,
  PROJECT_FILES_UNREADABLE,
} from "./OutputSection";
import {
  TimelineSection,
  TIMELINE_EMPTY,
  TIMELINE_ERROR,
} from "./TimelineSection";

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

  // Fix round 1, finding 4 — a failed read is not evidence the run spawned
  // nothing; `TimelineSection` must say the read failed, not draw the empty
  // sentence a 500 or a dropped connection has no business producing.
  it("says the read failed rather than claiming no steps have run", () => {
    render(<TimelineSection timeline={null} error="database is locked" />);
    expect(screen.getByText(TIMELINE_ERROR)).toBeInTheDocument();
    expect(screen.getByText(/database is locked/)).toBeInTheDocument();
    expect(screen.queryByText(TIMELINE_EMPTY)).not.toBeInTheDocument();
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

  it("lists the files a run wrote into its project under their own title", () => {
    render(
      <OutputSection
        artifacts={[artifact({ id: "art-1" })]}
        count={1}
        onOpen={vi.fn()}
        projectFiles={[
          artifact({ id: "p-1", name: "src/main.rs", kind: "code" }),
        ]}
        projectTotal={1}
      />,
    );
    expect(screen.getByText(PROJECT_FILES_TITLE)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /src\/main\.rs/ }),
    ).toBeInTheDocument();
    expect(screen.getByText("1 file")).toBeInTheDocument();
  });

  it("says Output is empty because the run's files are in its project", () => {
    render(
      <OutputSection
        artifacts={[]}
        count={0}
        projectFiles={[artifact({ id: "p-1", name: "src/main.rs" })]}
      />,
    );
    expect(screen.getByText(OUTPUT_EMPTY_BESIDE_PROJECT)).toBeInTheDocument();
    expect(screen.queryByText(OUTPUT_EMPTY)).not.toBeInTheDocument();
  });

  it("draws no project card for a run that wrote nothing there", () => {
    render(<OutputSection artifacts={[]} count={0} />);
    expect(screen.queryByText(PROJECT_FILES_TITLE)).not.toBeInTheDocument();
  });

  it("says the project files could not be listed rather than dropping them", () => {
    render(
      <OutputSection
        artifacts={[]}
        count={0}
        projectNote="The files written in the project could not be listed — database is locked"
      />,
    );
    expect(screen.getByText(PROJECT_FILES_UNREADABLE)).toBeInTheDocument();
    expect(screen.getByText(/database is locked/)).toBeInTheDocument();
  });

  // The path is the agent's: a bidi override or a zero-width space in it is
  // shown as `<U+XXXX>`, with the count under the row (lib/hidden-chars).
  it("shows the hidden characters an agent put in a project file's path, with their count", () => {
    render(
      <OutputSection
        artifacts={[]}
        count={0}
        onOpen={vi.fn()}
        projectFiles={[
          artifact({
            id: "p-1",
            name: "scripts/\u{202E}release\u{200B}.sh",
            kind: "code",
          }),
        ]}
      />,
    );
    const row = screen.getByRole("button", { name: /scripts\// });
    expect(
      Array.from(row.querySelectorAll("[data-hidden-char]")).map((span) =>
        span.getAttribute("data-hidden-char"),
      ),
    ).toEqual(["U+202E", "U+200B"]);
    const name = within(row).getByText(/^scripts\//);
    expect(name.textContent).toBe("scripts/<U+202E>release<U+200B>.sh");
    expect(name.className).toContain("wrap-anywhere");
    expect(name.className).not.toContain("truncate");
    expect(
      within(row).getByText(
        "Note: this request contains 2 hidden characters, shown as <U+…>.",
      ),
    ).toBeInTheDocument();
  });

  it("draws no hidden-character note for a plain path", () => {
    render(
      <OutputSection
        artifacts={[artifact({ id: "art-1" })]}
        count={1}
        onOpen={vi.fn()}
        projectFiles={[
          artifact({ id: "p-1", name: "src/main.rs", kind: "code" }),
        ]}
      />,
    );
    const name = screen.getByText("src/main.rs");
    expect(name.className).toContain("truncate");
    expect(screen.queryByText(/hidden character/)).not.toBeInTheDocument();
  });
});

describe("EventLogSection", () => {
  const event: RunEvent = {
    id: 4,
    task_id: "b41",
    tag: "steer",
    text: "steering message delivered",
    at: "2026-08-31T14:31:00Z",
  };

  it("shows the design's own empty copy for a run with nothing logged", () => {
    render(<EventLogSection events={[]} />);
    expect(screen.getByText(EVENT_LOG_EMPTY)).toBeInTheDocument();
  });

  // The gap note is gone with GAP-10: the card no longer shows this session
  // only, so a footer saying it would be false.
  it("renders the run's rows with no missing-API note", () => {
    render(<EventLogSection events={[event]} />);
    expect(screen.getByText("steering message delivered")).toBeInTheDocument();
    expect(screen.getByText("steer")).toBeInTheDocument();
    expect(screen.queryByText(/not yet available/)).not.toBeInTheDocument();
    expect(screen.queryByText(/task_id=/)).not.toBeInTheDocument();
  });

  // A failed read is not "nothing happened" — the T32 precedent from
  // `TimelineSection`, applied to the card below it.
  it("says the read failed rather than claiming the run is quiet", () => {
    render(<EventLogSection events={[]} error="500 Internal Server Error" />);
    expect(screen.getByText(EVENT_LOG_ERROR)).toBeInTheDocument();
    expect(screen.getByText(/500 Internal Server Error/)).toBeInTheDocument();
    expect(screen.queryByText(EVENT_LOG_EMPTY)).not.toBeInTheDocument();
  });
});
