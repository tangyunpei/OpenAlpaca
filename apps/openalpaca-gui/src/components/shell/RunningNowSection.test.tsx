import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { RunningNowSection, type RailRun } from "./RunningNowSection";

const runs: RailRun[] = [
  { id: "b41c8e02", title: "Connector audit", status: "running" },
  { id: "9ab30f11", title: "Migration notes v34", status: "paused" },
];

describe("RunningNowSection (§3.4)", () => {
  it("renders one row per live run and focuses it on click", () => {
    const onFocusRun = vi.fn();
    render(<RunningNowSection runs={runs} onFocusRun={onFocusRun} />);

    screen.getByRole("button", { name: /Connector audit/ }).click();
    expect(onFocusRun).toHaveBeenCalledWith("b41c8e02");
  });

  it("marks only the run that holds the pending confirmation", () => {
    render(
      <RunningNowSection
        runs={runs}
        onFocusRun={vi.fn()}
        blockedRunId="b41c8e02"
      />,
    );
    expect(screen.getAllByText("wait")).toHaveLength(1);
  });

  it("hides itself rather than showing an empty section", () => {
    const { container } = render(
      <RunningNowSection runs={[]} onFocusRun={vi.fn()} />,
    );
    expect(container).toBeEmptyDOMElement();
  });
});
