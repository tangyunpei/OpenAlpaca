/**
 * The shell's one load-bearing number: the minimum window (§8.7).
 *
 * `AppShell`'s doc comment states the budget as an arithmetic sum of the
 * columns a view can show at once, and a column added anywhere in the app makes
 * that sentence false unless the floor moves with it. This is the test that
 * keeps the two in step — per view, because the budget is a property of the
 * view on screen and only chat draws four columns.
 */

import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { PANE_BOUNDS } from "@/stores/pane-widths";

import { AppShell } from "./AppShell";

/** §2: the nav rail is a literal 196px and never shrinks. */
const RAIL = 196;
/** §8.7's "transcript ~500" — the narrowest column the design tolerates. */
const TRANSCRIPT = 500;

describe("AppShell", () => {
  it("is never narrower than the sum of the columns chat can show", () => {
    const { container } = render(
      <AppShell view="chat">
        <div />
      </AppShell>,
    );

    // rail 196 + conversations 200 + transcript 500 + chat aside 300 = 1196.
    const budget =
      RAIL + PANE_BOUNDS.chatSessionsW.min + TRANSCRIPT + PANE_BOUNDS.workW.min;
    expect(budget).toBeLessThanOrEqual(1200);
    expect(container.firstElementChild).toHaveClass("min-w-[1200px]");
  });

  /**
   * The conversation column is chat's alone. Work and Library draw a list and a
   * detail beside the rail, Settings a 220px nav over a 660px body — so the
   * chat floor made all three scroll sideways in a window they fit in.
   */
  it("holds the other three views to the floor they actually need", () => {
    for (const view of ["work", "library", "settings"] as const) {
      const { container } = render(
        <AppShell view={view}>
          <div />
        </AppShell>,
      );
      expect(container.firstElementChild).toHaveClass("min-w-[1000px]");
      expect(container.firstElementChild).not.toHaveClass("min-w-[1200px]");
    }

    // Work is the widest of the three: rail + list + a detail column.
    const widest = RAIL + PANE_BOUNDS.workListW.min + TRANSCRIPT;
    expect(widest).toBeLessThanOrEqual(1000);
  });

  /** A caller that names no view gets the widest floor, never the narrowest. */
  it("defaults to the chat floor", () => {
    const { container } = render(
      <AppShell>
        <div />
      </AppShell>,
    );
    expect(container.firstElementChild).toHaveClass("min-w-[1200px]");
  });
});
