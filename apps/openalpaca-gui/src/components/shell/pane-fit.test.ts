/**
 * T2's arithmetic: which of chat's side columns a window can hold.
 *
 * jsdom measures no layout, so this is where the rule is provable — the view
 * test only has to show that it is applied on mount and on resize.
 */

import { describe, expect, it } from "vitest";

import { PANE_DEFAULTS } from "@/stores/pane-widths";

import {
  chatPaneFit,
  MIN_TRANSCRIPT_WIDTH,
  RAIL_WIDTH,
  RESIZER_WIDTH,
} from "./pane-fit";

const DEFAULTS = {
  sessions: PANE_DEFAULTS.chatSessionsW,
  aside: PANE_DEFAULTS.workW,
};

describe("chatPaneFit (T2)", () => {
  it("keeps both columns in the window the design was drawn for", () => {
    expect(chatPaneFit(1440, DEFAULTS)).toEqual({
      sessions: true,
      aside: true,
    });
    expect(chatPaneFit(1920, DEFAULTS)).toEqual({
      sessions: true,
      aside: true,
    });
  });

  /**
   * The window this was found on: a portrait monitor, or a half-screen split.
   * Collapsing the conversation list by hand was what made it usable, so that
   * is the column that gives way first.
   */
  it("gives up the conversation column on a 1080-point window", () => {
    expect(chatPaneFit(1080, DEFAULTS)).toEqual({
      sessions: false,
      aside: true,
    });
  });

  it("gives up the aside too when even that will not fit", () => {
    expect(chatPaneFit(800, DEFAULTS)).toEqual({
      sessions: false,
      aside: false,
    });
    expect(chatPaneFit(640, DEFAULTS)).toEqual({
      sessions: false,
      aside: false,
    });
  });

  /** The order is fixed: the aside never goes while the list stays. */
  it("never drops the aside while keeping the conversation column", () => {
    for (let width = 400; width <= 2000; width += 13) {
      const fit = chatPaneFit(width, DEFAULTS);
      if (fit.sessions) expect(fit.aside).toBe(true);
    }
  });

  /** Each column costs its own resizer, and the widths are the live ones. */
  it("counts the resizer beside each column, at the width it really is", () => {
    const widths = { sessions: 360, aside: 600 };
    const both =
      RAIL_WIDTH +
      MIN_TRANSCRIPT_WIDTH +
      widths.sessions +
      RESIZER_WIDTH +
      widths.aside +
      RESIZER_WIDTH;

    expect(chatPaneFit(both, widths).sessions).toBe(true);
    expect(chatPaneFit(both - 1, widths).sessions).toBe(false);
    // …and the same boundary one column down.
    const asideOnly =
      RAIL_WIDTH + MIN_TRANSCRIPT_WIDTH + widths.aside + RESIZER_WIDTH;
    expect(chatPaneFit(asideOnly, widths).aside).toBe(true);
    expect(chatPaneFit(asideOnly - 1, widths).aside).toBe(false);
  });
});
