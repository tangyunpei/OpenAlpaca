/**
 * `AppShell` — the app frame (DESIGN_SPEC §2).
 *
 * The design is a fixed 1440×900 artboard, but this is a resizable desktop
 * window, so the frame is fluid where the artboard was fixed and exact
 * everywhere the artboard was exact:
 *
 *   * the root is `flex` at the viewport's full size, not 1440×900;
 *   * the nav rail keeps its literal 196px and never shrinks; the view section
 *     takes the remainder (`flex-1; min-width:0`) exactly as §2 specifies;
 *   * every internal dimension — 46px headers, 7px resizers, the 300/600,
 *     260/480 and 200/360 pane bounds, the 720/780 transcript column — stays a
 *     token;
 *   * the `min-width` is §8.7's "minimum sensible window", and it belongs to
 *     the **view on screen**, not to the app. Chat is the widest: rail 196 +
 *     conversations 200 + transcript ~500 + aside 300 = 1196, so 1200. It was
 *     1000 while the chat view had three columns; the conversation list (§5.7)
 *     made it four, and a floor that did not move with it would have left the
 *     transcript ~170px at the default aside width. Both of chat's side columns
 *     collapse, so the floor is what a window needs with everything open, not
 *     what it needs to be usable.
 *
 *     Work, Library and Settings never draw a fourth column — rail 196 + a
 *     260px list + a detail column, or a 220px nav over a 660px body — so the
 *     chat floor made every one of them scroll sideways in a window they fit
 *     in perfectly well. They keep the pre-§5.7 1000px, which is still the sum
 *     of what they do draw.
 *
 * `position:relative` is load-bearing: the toast (z-60) and the command palette
 * (z-50) are absolutely positioned siblings of the panes (§2.6).
 */

import { cn } from "@/lib/cn";
import type { View } from "@/stores/ui";

/** §8.7's floor per view — chat's four columns, everything else's three. */
export const MIN_WINDOW_CLASS: Record<View, string> = {
  chat: "min-w-[1200px]",
  work: "min-w-[1000px]",
  library: "min-w-[1000px]",
  settings: "min-w-[1000px]",
};

export interface AppShellProps {
  children: React.ReactNode;
  /**
   * The view on screen, which is what the minimum window is a property of.
   * Defaults to the widest floor, so a caller that does not say cannot end up
   * with panes narrower than the design allows.
   */
  view?: View;
  className?: string;
}

export function AppShell({
  children,
  view = "chat",
  className,
}: AppShellProps) {
  return (
    <div
      className={cn(
        "relative flex h-screen overflow-hidden bg-canvas font-sans text-ink",
        MIN_WINDOW_CLASS[view],
        className,
      )}
    >
      {children}
    </div>
  );
}
