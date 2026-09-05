import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { UiStatus } from "@/components/ui";

import type { Run } from "./run-model";
import { CompletedRow, WorkListRow } from "./WorkListRow";

const run = (patch: Partial<Run> = {}): Run => ({
  id: "b41c8e02",
  title: "Audit the connector surface",
  status: "running",
  meta: "11m 04s · 5/8 steps",
  started: "14:22:41",
  stamp: "14:33",
  note: null,
  laneKey: "local:gui",
  artifactCount: 0,
  artifacts: [],
  finishedAt: null,
  costUsd: null,
  ...patch,
});

describe("WorkListRow status → visual (§3.20, §3.24)", () => {
  const cases: Array<[UiStatus, string]> = [
    ["running", "RUNNING"],
    ["queued", "QUEUED"],
    ["paused", "PAUSED"],
    ["cancelled", "CANCELLED"],
    ["failed", "FAILED"],
  ];

  it.each(cases)("labels %s as %s", (status, label) => {
    render(
      <WorkListRow run={run({ status })} selected={false} onSelect={vi.fn()} />,
    );
    expect(screen.getByText(label)).toBeInTheDocument();
  });

  it("pulses only the running dot", () => {
    const { container, unmount } = render(
      <WorkListRow
        run={run({ status: "running" })}
        selected={false}
        onSelect={vi.fn()}
      />,
    );
    expect(container.querySelector(".animate-pulse-oa")).not.toBeNull();
    unmount();

    const queued = render(
      <WorkListRow
        run={run({ status: "queued" })}
        selected={false}
        onSelect={vi.fn()}
      />,
    );
    expect(queued.container.querySelector(".animate-pulse-oa")).toBeNull();
  });

  it("marks the selected row and raises it", () => {
    const { container } = render(
      <WorkListRow run={run()} selected onSelect={vi.fn()} />,
    );
    const button = container.querySelector("button");
    expect(button).toHaveAttribute("aria-current", "true");
    expect(button?.className).toContain("bg-raised");
  });

  it("leaves an unselected row transparent", () => {
    const { container } = render(
      <WorkListRow run={run()} selected={false} onSelect={vi.fn()} />,
    );
    const button = container.querySelector("button");
    expect(button).not.toHaveAttribute("aria-current");
    expect(button?.className).toContain("border-transparent");
  });

  it("reports the run id on click", async () => {
    const onSelect = vi.fn();
    render(<WorkListRow run={run()} selected={false} onSelect={onSelect} />);
    await userEvent.click(screen.getByRole("button"));
    expect(onSelect).toHaveBeenCalledWith("b41c8e02");
  });

  it("renders the meta the daemon can actually supply, with no cost", () => {
    render(<WorkListRow run={run()} selected={false} onSelect={vi.fn()} />);
    const meta = screen.getByText("11m 04s · 5/8 steps");
    expect(meta).toBeInTheDocument();
    expect(meta.textContent).not.toContain("$");
  });
});

describe("CompletedRow", () => {
  it("shows the title and its finishing stamp", () => {
    render(
      <CompletedRow
        run={run({ status: "done", stamp: "13:41" })}
        selected={false}
        onSelect={vi.fn()}
      />,
    );
    expect(screen.getByText("Audit the connector surface")).toBeInTheDocument();
    expect(screen.getByText("13:41")).toBeInTheDocument();
  });

  it("omits the stamp when the daemon recorded no time", () => {
    render(
      <CompletedRow
        run={run({ status: "done", stamp: null })}
        selected={false}
        onSelect={vi.fn()}
      />,
    );
    expect(screen.queryByText("13:41")).not.toBeInTheDocument();
  });
});
