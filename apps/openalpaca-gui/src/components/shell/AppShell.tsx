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
 *     the **view on screen**, not to the app. Work, Library and Settings draw
 *     rail 196 + a 260px list + a detail column, or a 220px nav over a 660px
 *     body, and none of those columns collapses — so 1000px is the sum of what
 *     they really need, and it is their floor.
 *
 *     Chat's is **lower**, and that is T2's correction. Its floor was 1200 —
 *     the sum of all four columns with everything open — which is not a
 *     minimum window at all but a demand: on a 1080-point window the row was
 *     laid out at 1200 *inside* it, and the aside's right edge, its collapse
 *     button included, was drawn outside the window with no way to reach it.
 *     Chat's two side columns collapse, and `chatPaneFit` now collapses them
 *     when the window cannot hold them, so the floor is what the view needs
 *     with them closed: rail 196 + the §2.2 gutters + a 440px transcript, 700.
 *
 * `position:relative` is load-bearing: the toast (z-60) and the command palette
 * (z-50) are absolutely positioned siblings of the panes (§2.6).
 *
 * It also swallows every **file** drop that nothing else took (U5). The window
 * runs with `dragDropEnabled: false`, so the webview — not Tauri — handles
 * drops, and a webview's default for a dropped file is to navigate to it,
 * which would replace the running app with a PDF. The composer takes its own
 * drop; this is the floor under it. Drags carrying anything but files pass
 * through untouched, or dragging a selection into the textarea would stop
 * working.
 */

import { cn } from "@/lib/cn";
import { dragCarriesFiles } from "@/lib/drag";
import type { View } from "@/stores/ui";

/**
 * §8.7's floor per view — what each one needs once the columns that *can*
 * collapse have (T2), not what it needs with everything open.
 */
export const MIN_WINDOW_CLASS: Record<View, string> = {
  chat: "min-w-[700px]",
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
      onDragOver={(event) => {
        if (dragCarriesFiles(event.dataTransfer)) event.preventDefault();
      }}
      onDrop={(event) => {
        if (dragCarriesFiles(event.dataTransfer)) event.preventDefault();
      }}
    >
      {children}
    </div>
  );
}
