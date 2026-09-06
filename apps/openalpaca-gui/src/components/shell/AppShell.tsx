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
 *   * a `min-width` of 1200px reproduces §8.7's "minimum sensible window" for
 *     the widest a view gets: rail 196 + conversations 200 + transcript ~500 +
 *     aside 300 = 1196, so panes cannot be crushed below the widths the design
 *     assumes. It was 1000 while the chat view had three columns; the
 *     conversation list (§5.7) made it four, and a floor that did not move with
 *     it would have left the transcript ~170px at the default aside width.
 *     Both of the chat view's side columns collapse, so the floor is what a
 *     window needs with everything open, not what it needs to be usable.
 *
 * `position:relative` is load-bearing: the toast (z-60) and the command palette
 * (z-50) are absolutely positioned siblings of the panes (§2.6).
 */

import { cn } from "@/lib/cn";

export interface AppShellProps {
  children: React.ReactNode;
  className?: string;
}

export function AppShell({ children, className }: AppShellProps) {
  return (
    <div
      className={cn(
        "relative flex h-screen min-w-[1200px] overflow-hidden bg-canvas font-sans text-ink",
        className,
      )}
    >
      {children}
    </div>
  );
}
