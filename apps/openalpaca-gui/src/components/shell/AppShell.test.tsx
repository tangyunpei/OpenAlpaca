/**
 * The shell's one load-bearing number: the minimum window (§8.7).
 *
 * `AppShell`'s doc comment states the budget as an arithmetic sum of the
 * columns a view can show at once, and a column added anywhere in the app makes
 * that sentence false unless the floor moves with it. This is the test that
 * keeps the two in step.
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
  it("is never narrower than the sum of the columns a view can show", () => {
    const { container } = render(
      <AppShell>
        <div />
      </AppShell>,
    );

    // rail 196 + conversations 200 + transcript 500 + chat aside 300 = 1196.
    const budget =
      RAIL + PANE_BOUNDS.chatSessionsW.min + TRANSCRIPT + PANE_BOUNDS.workW.min;
    expect(budget).toBeLessThanOrEqual(1200);
    expect(container.firstElementChild).toHaveClass("min-w-[1200px]");
  });
});
