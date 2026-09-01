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
 *   * every internal dimension — 46px headers, 7px resizers, the 300/600 and
 *     260/480 pane bounds, the 720/780 transcript column — stays a token;
 *   * a `min-width` of 1000px reproduces §8.7's "minimum sensible window"
 *     (rail 196 + transcript ~500 + aside 300) so panes cannot be crushed
 *     below the widths the design assumes.
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
        "relative flex h-screen min-w-[1000px] overflow-hidden bg-canvas font-sans text-ink",
        className,
      )}
    >
      {children}
    </div>
  );
}
