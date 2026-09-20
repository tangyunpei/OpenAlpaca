/**
 * Which of the chat view's side columns fit in the window it is in (T2).
 *
 * DESIGN_SPEC §8.7 says there is no responsive design: the artboard is a fixed
 * 1440×900 desktop window and "all panes flex, but no breakpoints exist". That
 * held while the shell simply refused to be narrower than the sum of its
 * columns — and on a real 1080-point window (a portrait monitor, a half-screen
 * split) the refusal is what broke it: the four columns were laid out at their
 * full 1200px inside an 1080px window, so the Work pane's right edge, collapse
 * button and all, was drawn *outside* the window with no way to reach it.
 *
 * So the columns give way instead, in the order the design would: the
 * conversation list §5.7 added first — it is the newest column and the one
 * whose header already carries its own way back — then the aside. Both are
 * collapsible by hand today; this only decides when the window itself has
 * already made the choice.
 *
 * Pure, and free of the store, because the interesting half is the arithmetic
 * and jsdom cannot measure a layout.
 */

/** §2: the nav rail is a literal 196px and never shrinks. */
export const RAIL_WIDTH = 196;

/** §2.2: the grab strip between two panes. */
export const RESIZER_WIDTH = 7;

/**
 * The narrowest transcript worth keeping a column for.
 *
 * Not §8.7's "~500": that is the artboard's *comfortable* transcript, and the
 * question here is the opposite one — how little room can be left before a
 * side column has to go. 440 is a readable measure at the 15px body with the
 * §2.2 26px gutters, and it is what puts the give-way point for the
 * conversation list just above the 1080-point window that found this.
 */
export const MIN_TRANSCRIPT_WIDTH = 440;

export interface ChatPaneWidths {
  /** `chatSessionsW` — the conversation column. */
  sessions: number;
  /** `workW` — the Work pane / file panel. */
  aside: number;
}

export interface ChatPaneFit {
  /** The conversation column fits. */
  sessions: boolean;
  /** The aside fits. */
  aside: boolean;
}

/**
 * What the window can hold, with each column costing its resizer too.
 *
 * Collapse-only by design: a window that grows never re-opens a column. The
 * person may have collapsed it on purpose, and an expand nobody asked for is
 * worse than a column they have to click once to get back — which is exactly
 * why the caller applies this on a resize and not on every render, so an
 * explicit expand stands until the window changes again.
 */
export function chatPaneFit(
  windowWidth: number,
  widths: ChatPaneWidths,
): ChatPaneFit {
  const aside = RESIZER_WIDTH + widths.aside;
  const sessions = widths.sessions + RESIZER_WIDTH;
  const body = RAIL_WIDTH + MIN_TRANSCRIPT_WIDTH;

  if (body + sessions + aside <= windowWidth)
    return { sessions: true, aside: true };
  if (body + aside <= windowWidth) return { sessions: false, aside: true };
  return { sessions: false, aside: false };
}
