import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import {
  LaneBar,
  laneColor,
  laneGeometry,
  showsPending,
  type Lane,
} from "./LaneBar";

const blockLane: Lane = {
  label: "review·3",
  start: 40,
  end: 74,
  state: "block",
  detail: "awaiting you",
};

describe("LaneBar", () => {
  it("colours lanes by state", () => {
    expect(laneColor("done", false)).toBe("var(--color-green)");
    expect(laneColor("run", false)).toBe("var(--color-line-hover)");
  });

  it("couples the blocked lane's red→green flip to the confirmation", () => {
    // §4.4: resolving the confirmation turns the same lane green and drops the
    // hatch — one state change, three visual consequences.
    expect(laneColor("block", true)).toBe("var(--color-red)");
    expect(showsPending("block", true)).toBe(true);

    expect(laneColor("block", false)).toBe("var(--color-green)");
    expect(showsPending("block", false)).toBe(false);
  });

  it("never hatches a lane that is not blocked", () => {
    expect(showsPending("done", true)).toBe(false);
    expect(showsPending("run", true)).toBe(false);
  });

  it("clamps percentages into the track", () => {
    expect(laneGeometry({ ...blockLane, start: -10, end: 130 })).toEqual({
      left: 0,
      width: 100,
    });
    // An inverted span collapses instead of painting backwards.
    expect(laneGeometry({ ...blockLane, start: 70, end: 20 })).toEqual({
      left: 70,
      width: 0,
    });
    expect(laneGeometry({ ...blockLane, start: Number.NaN, end: 50 })).toEqual({
      left: 0,
      width: 50,
    });
  });

  it("renders the trailing detail only at the wide size", () => {
    const { rerender } = render(<LaneBar lane={blockLane} />);
    expect(screen.queryByText("awaiting you")).toBeNull();

    rerender(<LaneBar lane={blockLane} size="wide" />);
    expect(screen.getByText("awaiting you")).toBeInTheDocument();
  });
});
