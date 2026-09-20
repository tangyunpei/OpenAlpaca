/**
 * The shell's one load-bearing number: the minimum window (§8.7) — and, since
 * U5, the floor under the composer's file drop.
 *
 * `AppShell`'s doc comment states the budget as an arithmetic sum of the
 * columns a view can show at once, and a column added anywhere in the app makes
 * that sentence false unless the floor moves with it. This is the test that
 * keeps the two in step — per view, because the budget is a property of the
 * view on screen and only chat draws four columns.
 */

import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { PANE_BOUNDS } from "@/stores/pane-widths";

import { AppShell } from "./AppShell";
import { MIN_TRANSCRIPT_WIDTH, RAIL_WIDTH } from "./pane-fit";

/** §2: the nav rail is a literal 196px and never shrinks. */
const RAIL = RAIL_WIDTH;
/** §8.7's "transcript ~500" — the narrowest column the design tolerates. */
const TRANSCRIPT = 500;

describe("AppShell", () => {
  /**
   * T2 turned this assertion around. The chat floor used to be the sum of all
   * four columns *open* (1200), which on a 1080-point window laid the row out
   * wider than the window and drew the aside's collapse button outside it.
   * Chat's two side columns give way instead, so its floor is what the view
   * needs with them closed.
   */
  it("holds chat to what it needs with its collapsible columns closed", () => {
    const { container } = render(
      <AppShell view="chat">
        <div />
      </AppShell>,
    );

    // rail 196 + the §2.2 26px gutters + a 440px transcript = 688.
    const collapsed = RAIL + 26 * 2 + MIN_TRANSCRIPT_WIDTH;
    expect(collapsed).toBeLessThanOrEqual(700);
    expect(container.firstElementChild).toHaveClass("min-w-[700px]");
    // And it is below the window the bug was found in, so nothing overflows.
    expect(700).toBeLessThan(1080);
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
      expect(container.firstElementChild).not.toHaveClass("min-w-[700px]");
    }

    // Work is the widest of the three: rail + list + a detail column.
    const widest = RAIL + PANE_BOUNDS.workListW.min + TRANSCRIPT;
    expect(widest).toBeLessThanOrEqual(1000);
  });

  /** A caller that names no view gets the chat floor. */
  it("defaults to the chat floor", () => {
    const { container } = render(
      <AppShell>
        <div />
      </AppShell>,
    );
    expect(container.firstElementChild).toHaveClass("min-w-[700px]");
  });

  /**
   * U5's floor. The window runs with `dragDropEnabled: false`, so the webview
   * handles drops itself and its default for a dropped file is to navigate to
   * it — replacing the app. A file dropped anywhere but the composer has to
   * die here.
   */
  it("swallows a file dropped outside the composer, and only a file", () => {
    const { container } = render(
      <AppShell>
        <div>body</div>
      </AppShell>,
    );
    const shell = container.firstElementChild as HTMLElement;

    const file = { types: ["Files"], files: [] };
    expect(fireEvent.dragOver(shell, { dataTransfer: file })).toBe(false);
    expect(fireEvent.drop(shell, { dataTransfer: file })).toBe(false);

    // A dragged selection is not ours: cancelling it would break dropping
    // text into the composer's textarea.
    const text = { types: ["text/plain"], files: [] };
    expect(fireEvent.dragOver(shell, { dataTransfer: text })).toBe(true);
    expect(fireEvent.drop(shell, { dataTransfer: text })).toBe(true);
  });
});
